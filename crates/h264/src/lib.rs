//! Clean-room pure-Rust H.264 / AVC decoder, implemented from ITU-T Rec. H.264 (ISO/IEC 14496-10).
//!
//! Supports 8 to 10-bit 4:2:0 and intra 4:2:2 with native byte or word sample storage.
//!
//! Supported: progressive (frame) coding, 8 to 10-bit 4:2:0, Baseline / Main / High / High 10
//! and High 4:2:2 / High 4:2:2 Intra profiles (4:2:2 intra pictures only), CAVLC and CABAC,
//! I/P/B slices, 8x8 transform, custom scaling matrices, weighted prediction, spatial/temporal direct,
//! multiple slices, long-term references and all MMCOs, deblocking.
//!
//! ```no_run
//! let mut dec = filmcraft_h264::Decoder::new();
//! let stream = std::fs::read("video.h264").unwrap();
//! for pic in dec.decode(&stream, 0).unwrap() {
//!     println!("{}x{} poc {}", pic.width, pic.height, pic.poc);
//! }
//! for pic in dec.flush() {
//!     let _ = pic;
//! }
//! ```

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

mod cabac;
mod cabac_mb;
#[rustfmt::skip]
mod cabac_tables;
mod cavlc;
#[rustfmt::skip]
mod cavlc_tables;
mod deblock;
mod decoder;
pub mod dpb;
mod error;
mod inter;
mod intra;
mod mbtypes;
pub mod params;
mod picture;
pub use picture::{set_plane_allocator, set_plane_allocator_u16};
pub mod slice;
mod slicedec;
#[cfg(test)]
mod synth_tests;
mod tables;
mod transform;

pub use decoder::{DecodeStats, Decoder, default_threads, stream_is_intra_annexb};
pub use error::{Error, Result};

/// Colour description from the VUI (ITU-T H.273 code points).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorInfo {
    /// `video_full_range_flag`.
    pub full_range: bool,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
}

/// One decoded sample plane in its native storage type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plane {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

impl Plane {
    pub fn len(&self) -> usize {
        match self {
            Plane::U8(v) => v.len(),
            Plane::U16(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn as_u8(&self) -> Option<&[u8]> {
        match self {
            Plane::U8(v) => Some(v),
            Plane::U16(_) => None,
        }
    }
    pub fn as_u16(&self) -> Option<&[u16]> {
        match self {
            Plane::U8(_) => None,
            Plane::U16(v) => Some(v),
        }
    }
    /// Sample `i` widened to u16.
    pub fn get(&self, i: usize) -> u16 {
        match self {
            Plane::U8(v) => v[i] as u16,
            Plane::U16(v) => v[i],
        }
    }
    /// Raw bytes as written by ffmpeg `rawvideo` (8-bit, or 16-bit little endian).
    pub fn to_le_bytes(&self) -> Vec<u8> {
        match self {
            Plane::U8(v) => v.clone(),
            Plane::U16(v) => v.iter().flat_map(|s| s.to_le_bytes()).collect(),
        }
    }
}

/// A decoded, cropped picture in planar 4:2:0 or 4:2:2.
#[derive(Clone, Debug)]
pub struct Picture {
    /// Cropped luma width.
    pub width: u32,
    /// Cropped luma height.
    pub height: u32,
    /// 1 for 4:2:0, 2 for 4:2:2 (intra pictures only).
    pub chroma_format_idc: u8,
    pub chroma_width: u32,
    pub chroma_height: u32,
    /// Luma bit depth (chroma uses the same storage type).
    pub bit_depth: u32,
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
    /// Strides in samples.
    pub y_stride: usize,
    pub uv_stride: usize,
    /// Presentation timestamp passed to [`Decoder::decode`] with the access unit of this picture.
    pub pts: i64,
    /// Picture order count.
    pub poc: i32,
    /// IDR picture.
    pub key: bool,
    pub color: ColorInfo,
    /// Sample aspect ratio (0, 0 when unspecified).
    pub sar: (u16, u16),
    /// Decoded in draft mode ([`Decoder::set_draft`]): a non-reference picture whose deblocking
    /// filter was skipped. Its samples are approximate (not the conforming output); no other
    /// picture is affected.
    pub draft: bool,
}
