//! Picture buffers and per-macroblock state.

use std::sync::{Arc, OnceLock};

/// Storage type of decoded samples: `u8` for 8-bit pictures (half the memory and cache traffic
/// of 16-bit storage in prediction reads, filters and output), `u16` for higher bit depths.
/// Values are always within `0..1 << bit_depth`.
pub trait Sample: Copy + Default + PartialEq + std::fmt::Debug + Send + Sync + 'static {
    type Packed: Copy + Default;
    fn pack8(samples: [Self; 8]) -> Self::Packed;
    fn unpack8(word: Self::Packed) -> [Self; 8];
    fn lane(word: Self::Packed, k: usize) -> i16;
    fn pack_lanes<const N: usize>(samples: &[[i16; N]; 8], i: usize) -> Self::Packed;
    fn to_i32(self) -> i32;
    fn to_i16(self) -> i16;
    fn to_u16(self) -> u16;
    /// `v` (a sample value, already clipped to the bit depth) in the storage type.
    fn from_i32(v: i32) -> Self;
    /// `v` (a sample value, within the bit depth) in the storage type.
    fn from_u16(v: u16) -> Self;
    /// The rows of `f` if they are stored with this type.
    fn rows(f: &Frame) -> Option<&[OnceLock<FrameRow<Self>>]>;
    fn copy_pcm(dst: &mut [Self], src: &[u16]);
    fn allocator() -> fn(usize) -> Vec<Self>;
    fn wrap_plane(samples: Vec<Self>) -> crate::Plane;
    fn plane_samples(plane: crate::Plane) -> Option<Vec<Self>>;
    fn wrap_rows(rows: Box<[OnceLock<FrameRow<Self>>]>) -> Rows;
}

impl Sample for u8 {
    type Packed = u64;
    #[inline(always)]
    fn pack8(samples: [Self; 8]) -> Self::Packed {
        u64::from_le_bytes(samples)
    }
    #[inline(always)]
    fn unpack8(word: Self::Packed) -> [Self; 8] {
        word.to_le_bytes()
    }
    #[inline(always)]
    fn lane(word: Self::Packed, k: usize) -> i16 {
        ((word >> (8 * k)) & 0xff) as i16
    }
    #[inline(always)]
    fn pack_lanes<const N: usize>(samples: &[[i16; N]; 8], i: usize) -> Self::Packed {
        (0..8).fold(0u64, |acc, k| acc | ((samples[k][i] as u8 as u64) << (8 * k)))
    }
    #[inline(always)]
    fn to_i32(self) -> i32 {
        self as i32
    }
    #[inline(always)]
    fn to_i16(self) -> i16 {
        self as i16
    }
    #[inline(always)]
    fn to_u16(self) -> u16 {
        self as u16
    }
    #[inline(always)]
    fn from_i32(v: i32) -> Self {
        v as u8
    }
    #[inline(always)]
    fn from_u16(v: u16) -> Self {
        v as u8
    }
    #[inline(always)]
    fn rows(f: &Frame) -> Option<&[OnceLock<FrameRow<u8>>]> {
        match &f.rows {
            Rows::U8(r) => Some(r),
            Rows::U16(_) => None,
        }
    }
    #[inline(always)]
    fn copy_pcm(dst: &mut [Self], src: &[u16]) {
        for (d, &v) in dst.iter_mut().zip(src) {
            *d = v as u8;
        }
    }
    fn allocator() -> fn(usize) -> Vec<Self> {
        plane_allocator()
    }
    fn wrap_plane(samples: Vec<Self>) -> crate::Plane {
        crate::Plane::U8(samples)
    }
    fn plane_samples(plane: crate::Plane) -> Option<Vec<Self>> {
        match plane {
            crate::Plane::U8(samples) => Some(samples),
            crate::Plane::U16(_) => None,
        }
    }
    fn wrap_rows(rows: Box<[OnceLock<FrameRow<u8>>]>) -> Rows {
        Rows::U8(rows)
    }
}

impl Sample for u16 {
    type Packed = u128;
    #[inline(always)]
    fn pack8(samples: [Self; 8]) -> Self::Packed {
        u128::from_le_bytes(std::array::from_fn(|i| samples[i / 2].to_le_bytes()[i & 1]))
    }
    #[inline(always)]
    fn unpack8(word: Self::Packed) -> [Self; 8] {
        std::array::from_fn(|k| (word >> (16 * k)) as u16)
    }
    #[inline(always)]
    fn lane(word: Self::Packed, k: usize) -> i16 {
        ((word >> (16 * k)) & 0xffff) as i16
    }
    #[inline(always)]
    fn pack_lanes<const N: usize>(samples: &[[i16; N]; 8], i: usize) -> Self::Packed {
        (0..8).fold(0u128, |acc, k| acc | ((samples[k][i] as u16 as u128) << (16 * k)))
    }
    #[inline(always)]
    fn to_i32(self) -> i32 {
        self as i32
    }
    #[inline(always)]
    fn to_i16(self) -> i16 {
        self as i16
    }
    #[inline(always)]
    fn to_u16(self) -> u16 {
        self
    }
    #[inline(always)]
    fn from_i32(v: i32) -> Self {
        v as u16
    }
    #[inline(always)]
    fn from_u16(v: u16) -> Self {
        v
    }
    #[inline(always)]
    fn rows(f: &Frame) -> Option<&[OnceLock<FrameRow<u16>>]> {
        match &f.rows {
            Rows::U16(r) => Some(r),
            Rows::U8(_) => None,
        }
    }
    #[inline(always)]
    fn copy_pcm(dst: &mut [Self], src: &[u16]) {
        dst.copy_from_slice(src);
    }
    fn allocator() -> fn(usize) -> Vec<Self> {
        Vec::with_capacity
    }
    fn wrap_plane(samples: Vec<Self>) -> crate::Plane {
        crate::Plane::U16(samples)
    }
    fn plane_samples(plane: crate::Plane) -> Option<Vec<Self>> {
        match plane {
            crate::Plane::U16(samples) => Some(samples),
            crate::Plane::U8(_) => None,
        }
    }
    fn wrap_rows(rows: Box<[OnceLock<FrameRow<u16>>]>) -> Rows {
        Rows::U16(rows)
    }
}

/// Whether pictures with these bit depths are stored as `u8` (both 8-bit), else as `u16`.
pub fn stores_u8(bit_depth: u32, bit_depth_c: u32) -> bool {
    bit_depth <= 8 && bit_depth_c <= 8
}

/// Planar sample buffers (MB-aligned dimensions, stride == width).
#[derive(Clone)]
pub struct Planes<S: Sample> {
    pub y: Vec<S>,
    pub cb: Vec<S>,
    pub cr: Vec<S>,
    pub width: usize,
    pub height: usize,
    pub cwidth: usize,
    pub mb_height_c: usize,
}

impl<S: Sample> Planes<S> {
    pub fn new(width: usize, height: usize, bit_depth: u32, chroma_format_idc: u8) -> Self {
        let mb_height_c = if chroma_format_idc == 2 { 16 } else { 8 };
        let (cw, ch) = (width / 2, height * mb_height_c / 16);
        Planes {
            y: vec![S::default(); width * height],
            cb: vec![S::from_i32(1 << (bit_depth - 1)); cw * ch],
            cr: vec![S::from_i32(1 << (bit_depth - 1)); cw * ch],
            width,
            height,
            cwidth: cw,
            mb_height_c,
        }
    }
    pub fn gray(width: usize, height: usize, bit_depth: u32, chroma_format_idc: u8) -> Self {
        let mut p = Self::new(width, height, bit_depth, chroma_format_idc);
        p.y.fill(S::from_i32(1 << (bit_depth - 1)));
        p
    }
}

/// Macroblock coding category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MbKind {
    #[default]
    None,
    I4x4,
    I8x8,
    I16x16,
    IPcm,
    PSkip,
    BSkip,
    BDirect16x16,
    /// Any other P or B inter macroblock.
    Inter,
}

impl MbKind {
    #[inline]
    pub fn is_intra(self) -> bool {
        matches!(self, MbKind::I4x4 | MbKind::I8x8 | MbKind::I16x16 | MbKind::IPcm)
    }
}

/// Per-macroblock state kept for the whole picture (neighbour derivations, deblocking, co-located data).
#[derive(Clone, Copy)]
pub struct MbState {
    /// Slice number within the picture; `u32::MAX` = not (yet) decoded.
    pub slice_num: u32,
    pub kind: MbKind,
    pub transform_8x8: bool,
    /// coded_block_pattern: bits 0..3 luma, bits 4..5 chroma.
    pub cbp: u8,
    /// Unprimed QPY (7.4.5), including negative values at higher bit depths.
    pub qp: i8,
    /// QPc for Cb / Cr (used by deblocking).
    pub qpc: [i8; 2],
    pub intra_chroma_mode: u8,
    /// Intra4x4PredMode / Intra8x8PredMode per 4x4 block (raster order).
    pub intra_modes: [u8; 16],
    /// CAVLC: TotalCoeff per luma 4x4 block (raster). CABAC: coded_block_flag.
    pub nnz: [u8; 16],
    /// Same for chroma AC blocks [Cb, Cr][raster 2 wide by up to 4 high].
    pub nnz_c: [[u8; 8]; 2],
    /// CABAC coded_block_flag of DC blocks: bit0 luma (I16x16), bit1 Cb, bit2 Cr.
    pub cbf_dc: u8,
    /// Luma 4x4 blocks (raster bit) with non-zero coefficients, for deblocking bS = 2.
    pub nz_mask: u16,
    pub ref_idx: [[i8; 4]; 2],
    /// Motion vectors per 4x4 block (raster order).
    pub mv: [[[i16; 2]; 16]; 2],
    /// |mvd| per 4x4 block (raster), saturated, for CABAC context selection.
    pub mvd: [[[u8; 2]; 16]; 2],
    /// Bit per 8x8 block predicted in direct mode.
    pub direct8x8: u8,
}

impl MbState {
    #[inline]
    pub fn kind_is_skip(&self) -> bool {
        matches!(self.kind, MbKind::PSkip | MbKind::BSkip)
    }
}

impl Default for MbState {
    fn default() -> Self {
        MbState {
            slice_num: u32::MAX,
            kind: MbKind::None,
            transform_8x8: false,
            cbp: 0,
            qp: 0,
            qpc: [0; 2],
            intra_chroma_mode: 0,
            intra_modes: [2; 16],
            nnz: [0; 16],
            nnz_c: [[0; 8]; 2],
            cbf_dc: 0,
            nz_mask: 0,
            ref_idx: [[-1; 4]; 2],
            mv: [[[0; 2]; 16]; 2],
            mvd: [[[0; 2]; 16]; 2],
            direct8x8: 0,
        }
    }
}

/// One published macroblock row of a decoded frame: final (deblocked) samples plus the motion data
/// needed as co-located information for direct prediction.
pub struct FrameRow<S: Sample> {
    /// 16 luma lines.
    pub y: Box<[S]>,
    /// 8 or 16 lines per chroma component.
    pub cb: Box<[S]>,
    pub cr: Box<[S]>,
    /// Per 4x4 block: index mb_x * 16 + raster.
    pub mv: [Box<[[i16; 2]]>; 2],
    /// Per 8x8 block: index mb_x * 4 + b8.
    pub ref_idx: [Box<[i8]>; 2],
    /// Unique id of the referenced picture per 8x8 block (u32::MAX = none).
    pub ref_id: [Box<[u32]>; 2],
    pub intra: Box<[bool]>,
}

/// Published macroblock rows in the frame's sample storage type.
pub enum Rows {
    U8(Box<[OnceLock<FrameRow<u8>>]>),
    U16(Box<[OnceLock<FrameRow<u16>>]>),
}

/// A decoded (or in-progress) frame as stored in the DPB and referenced by later pictures.
///
/// Rows are published in order by the thread decoding the picture; readers block (per row) until the
/// data they need is available, which lets several pictures decode concurrently.
pub struct Frame {
    pub id: u32,
    pub poc: i32,
    pub width: usize,
    pub height: usize,
    pub cwidth: usize,
    pub cheight: usize,
    pub mb_height_c: usize,
    pub chroma_format_idc: u8,
    pub mb_w: usize,
    pub bit_depth: u32,
    rows: Rows,
}

/// The value of a published row (waiting for it with the `threads` feature).
#[inline(always)]
fn wait<T>(cell: &OnceLock<T>) -> Option<&T> {
    #[cfg(feature = "threads")]
    {
        Some(cell.wait())
    }
    #[cfg(not(feature = "threads"))]
    {
        cell.get()
    }
}

impl Frame {
    pub fn new(id: u32, poc: i32, mb_w: usize, mb_h: usize, bit_depth: u32, chroma_format_idc: u8) -> Self {
        fn cells<S: Sample>(n: usize) -> Rows {
            S::wrap_rows((0..n).map(|_| OnceLock::new()).collect())
        }
        let rows = if stores_u8(bit_depth, bit_depth) { cells::<u8>(mb_h) } else { cells::<u16>(mb_h) };
        let mb_height_c = if chroma_format_idc == 2 { 16 } else { 8 };
        Frame { id, poc, width: mb_w * 16, height: mb_h * 16, cwidth: mb_w * 8, cheight: mb_h * mb_height_c, mb_height_c, chroma_format_idc, mb_w, bit_depth, rows }
    }

    pub fn mb_h(&self) -> usize {
        match &self.rows {
            Rows::U8(rows) => rows.len(),
            Rows::U16(rows) => rows.len(),
        }
    }

    /// Publish MB row `r` (ignored if already published or stored with another sample type).
    pub fn publish<S: Sample>(&self, r: usize, row: FrameRow<S>) {
        if let Some(cell) = S::rows(self).and_then(|rows| rows.get(r)) {
            let _ = cell.set(row);
        }
    }

    pub fn is_published(&self, r: usize) -> bool {
        match &self.rows {
            Rows::U8(rows) => rows.get(r).is_some_and(|cell| cell.get().is_some()),
            Rows::U16(rows) => rows.get(r).is_some_and(|cell| cell.get().is_some()),
        }
    }

    /// MB row `r`, waiting until it has been published; None for a different storage type.
    #[inline]
    pub fn row<S: Sample>(&self, r: usize) -> Option<&FrameRow<S>> {
        wait(S::rows(self)?.get(r)?)
    }

    /// Wait until the whole frame is available.
    pub fn wait_complete(&self) {
        match &self.rows {
            Rows::U8(rows) => {
                for row in rows.iter() {
                    let _ = wait(row);
                }
            }
            Rows::U16(rows) => {
                for row in rows.iter() {
                    let _ = wait(row);
                }
            }
        }
    }

    /// Build a published row from a contiguous plane set (rows of MB row `r`) and motion data.
    pub fn make_row<S: Sample>(planes: &Planes<S>, r: usize, mbs: &[MbState], ref_ids: &impl Fn(&MbState, usize, i8) -> u32) -> FrameRow<S> {
        let w = planes.width;
        let cw = planes.cwidth;
        let mb_w = w / 16;
        let y = planes.y[r * 16 * w..(r + 1) * 16 * w].into();
        let hc = planes.mb_height_c;
        let cb = planes.cb[r * hc * cw..(r + 1) * hc * cw].into();
        let cr = planes.cr[r * hc * cw..(r + 1) * hc * cw].into();
        let row_mbs = &mbs[r * mb_w..(r + 1) * mb_w];
        let mut mv = [vec![[0i16; 2]; mb_w * 16], vec![[0i16; 2]; mb_w * 16]];
        let mut ref_idx = [vec![-1i8; mb_w * 4], vec![-1i8; mb_w * 4]];
        let mut ref_id = [vec![u32::MAX; mb_w * 4], vec![u32::MAX; mb_w * 4]];
        let mut intra = vec![true; mb_w];
        for (x, st) in row_mbs.iter().enumerate() {
            if st.slice_num == u32::MAX {
                continue;
            }
            intra[x] = st.kind.is_intra();
            for l in 0..2 {
                mv[l][x * 16..x * 16 + 16].copy_from_slice(&st.mv[l]);
                for b in 0..4 {
                    let ri = st.ref_idx[l][b];
                    ref_idx[l][x * 4 + b] = ri;
                    if ri >= 0 {
                        ref_id[l][x * 4 + b] = ref_ids(st, l, ri);
                    }
                }
            }
        }
        let [mv0, mv1] = mv;
        let [ri0, ri1] = ref_idx;
        let [id0, id1] = ref_id;
        FrameRow { y, cb, cr, mv: [mv0.into(), mv1.into()], ref_idx: [ri0.into(), ri1.into()], ref_id: [id0.into(), id1.into()], intra: intra.into() }
    }

    /// A frame with every row published from `planes` and no motion (intra), e.g. "non-existing"
    /// frames inferred for frame_num gaps.
    pub fn from_planes<S: Sample>(id: u32, poc: i32, planes: &Planes<S>, bit_depth: u32) -> Self {
        let (mb_w, mb_h) = (planes.width / 16, planes.height / 16);
        let f = Frame::new(id, poc, mb_w, mb_h, bit_depth, if planes.mb_height_c == 16 { 2 } else { 1 });
        let blank = vec![MbState::default(); mb_w * mb_h];
        for r in 0..mb_h {
            f.publish(r, Frame::make_row(planes, r, &blank, &|_, _, _| u32::MAX));
        }
        f
    }

    /// Copy the w x h luma window at (x0, y0) (clamped to the picture) into `out` (stride `os`).
    #[inline]
    pub fn luma_window<S: Sample>(&self, (x0, y0): (i32, i32), w: usize, h: usize, out: &mut [S], os: usize) {
        let (width, height) = (self.width, self.height);
        for r in 0..h {
            let yy = (y0 + r as i32).clamp(0, height as i32 - 1) as usize;
            let Some(row) = self.row::<S>(yy >> 4) else { return };
            let line = &row.y[(yy & 15) * width..(yy & 15) * width + width];
            copy_line::<S, 24>(line, x0, &mut out[r * os..], w);
        }
    }

    /// Copy the w x h window of chroma component `c` at (x0, y0) (clamped) into `out`.
    #[inline]
    pub fn chroma_window<S: Sample>(&self, c: usize, (x0, y0): (i32, i32), w: usize, h: usize, out: &mut [S], os: usize) {
        let (cw, ch) = (self.cwidth, self.cheight);
        for r in 0..h {
            let yy = (y0 + r as i32).clamp(0, ch as i32 - 1) as usize;
            let Some(row) = self.row::<S>(yy >> self.mb_height_c.trailing_zeros()) else { return };
            let plane = if c == 0 { &row.cb } else { &row.cr };
            let line = &plane[(yy & (self.mb_height_c - 1)) * cw..(yy & (self.mb_height_c - 1)) * cw + cw];
            copy_line::<S, 16>(line, x0, &mut out[r * os..], w);
        }
    }

    /// Crop and copy the frame into planar buffers (waits for completion).
    pub fn copy_cropped(&self, crop: (usize, usize, usize, usize)) -> (crate::Plane, crate::Plane, crate::Plane) {
        match &self.rows {
            Rows::U8(_) => self.copy_cropped_as::<u8>(crop),
            Rows::U16(_) => self.copy_cropped_as::<u16>(crop),
        }
    }

    fn copy_cropped_as<S: Sample>(&self, crop: (usize, usize, usize, usize)) -> (crate::Plane, crate::Plane, crate::Plane) {
        let (cx, cy, cw, ch) = crop;
        let alloc = S::allocator();
        let mut y = alloc(cw * ch);
        for r in cy..cy + ch {
            let Some(row) = self.row::<S>(r >> 4) else { continue };
            y.extend_from_slice(&row.y[(r & 15) * self.width + cx..(r & 15) * self.width + cx + cw]);
        }
        let sub_h = 16 / self.mb_height_c;
        let (ccx, ccy, ccw, cch) = (cx / 2, cy / sub_h, cw.div_ceil(2), ch.div_ceil(sub_h));
        let mut u = alloc(ccw * cch);
        let mut v = alloc(ccw * cch);
        for r in ccy..ccy + cch {
            let Some(row) = self.row::<S>(r >> self.mb_height_c.trailing_zeros()) else { continue };
            let o = (r & (self.mb_height_c - 1)) * self.cwidth + ccx;
            u.extend_from_slice(&row.cb[o..o + ccw]);
            v.extend_from_slice(&row.cr[o..o + ccw]);
        }
        (S::wrap_plane(y), S::wrap_plane(u), S::wrap_plane(v))
    }
}

static PLANE_ALLOCATOR: std::sync::OnceLock<fn(usize) -> Vec<u8>> = std::sync::OnceLock::new();

/// Where output planes come from: an empty buffer with room for the given number of samples.
/// The host sets it once to recycle the planes of pictures it is done with; the first call wins.
pub fn set_plane_allocator(alloc: fn(usize) -> Vec<u8>) {
    let _ = PLANE_ALLOCATOR.set(alloc);
}

fn plane_allocator() -> fn(usize) -> Vec<u8> {
    PLANE_ALLOCATOR.get().copied().unwrap_or(Vec::with_capacity)
}

/// Copy `w` samples of `line` starting at x0 into `out`; when possible a fixed-size block of `N`
/// samples is copied instead (cheaper than a variable-length copy; `out` must then have room for `N`).
#[inline(always)]
fn copy_line<S: Sample, const N: usize>(line: &[S], x0: i32, out: &mut [S], w: usize) {
    if w <= N
        && x0 >= 0
        && let Some(src) = line.get(x0 as usize..).and_then(|l| l.first_chunk::<N>())
        && let Some(dst) = out.first_chunk_mut::<N>()
    {
        *dst = *src;
    } else {
        copy_clamped(line, x0, &mut out[..w]);
    }
}

/// Copy `out.len()` samples of `line` starting at x0, replicating edge samples outside the line.
#[inline(always)]
fn copy_clamped<S: Sample>(line: &[S], x0: i32, out: &mut [S]) {
    let w = out.len();
    if x0 >= 0 && x0 as usize + w <= line.len() {
        out.copy_from_slice(&line[x0 as usize..x0 as usize + w]);
    } else {
        let max = line.len() as i32 - 1;
        for (c, o) in out.iter_mut().enumerate() {
            *o = line[(x0 + c as i32).clamp(0, max) as usize];
        }
    }
}

pub type FrameRef = Arc<Frame>;

/// An entry of RefPicList0/1 for the current slice.
#[derive(Clone)]
pub struct RefPic {
    pub frame: FrameRef,
    pub long_term: bool,
}

impl RefPic {
    pub fn poc(&self) -> i32 {
        self.frame.poc
    }
    pub fn id(&self) -> u32 {
        self.frame.id
    }
}
