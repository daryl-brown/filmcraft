//! Independent H.264 samples, container probing and permanent fallback against ffmpeg.

#[path = "it/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};

use common::{bytes, ffmpeg_frames, fixture, max_diff, planes, raw_planes};
use filmcraft_codecs::video::{H264Decoder, VideoDecoder};
use filmcraft_media::FrameRequest;

const SCRAMBLED: &[usize] = &[17, 2, 21, 5, 14, 0, 23, 8, 19, 3, 12, 6];

fn encode(ff: &Path, name: &str, pix_fmt: &str, profile: &str, params: &str, muxer: Option<&str>) -> Option<PathBuf> {
    let mut args = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=352x288:rate=25",
        "-frames:v",
        "24",
        "-c:v",
        "libx264",
        "-pix_fmt",
        pix_fmt,
        "-profile:v",
        profile,
        "-x264-params",
        params,
    ];
    if let Some(muxer) = muxer {
        args.extend_from_slice(&["-f", muxer]);
    }
    fixture(ff, name, &args)
}

fn check_source(ff: &Path, file: &Path, oracle: &Path, pix_fmt: &str, intra: bool, all_first: bool) {
    let source = filmcraft_codecs::open_bytes(file.file_name().unwrap().to_str().unwrap(), bytes(file)).expect("open H.264 source");
    let rate = source.info().frame_rate();
    let raw = ffmpeg_frames(ff, oracle, pix_fmt);
    let bps = if pix_fmt.ends_with("10le") { 2 } else { 1 };
    let ch = if pix_fmt.starts_with("yuv422") { 288 } else { 144 };
    let frame_size = (352 * 288 + 2 * 176 * ch) * bps;
    assert_eq!(raw.len(), 24 * frame_size, "oracle frame count");
    let check = |i: usize| {
        let frame = source.video_frame(FrameRequest::full(rate.tick_of(i as i64))).expect("decode frame");
        let reference = raw_planes(&raw[i * frame_size..(i + 1) * frame_size], 352, 288, 176, ch, bps);
        assert_eq!(max_diff(&planes(&frame), &reference), 0, "{} frame {i}", file.display());
    };
    if all_first {
        let before = filmcraft_codecs::gop_stats();
        check(0);
        let first = filmcraft_codecs::gop_stats() - before;
        assert_eq!(first.misses, 1);
        assert_eq!(first.decoded, 1, "patched SPS selects independent decoding for the IDR");
        let before = filmcraft_codecs::gop_stats();
        check(1);
        let fallback = filmcraft_codecs::gop_stats() - before;
        assert_eq!(fallback.misses, 1);
        assert!(fallback.decoded > 1, "the first P picture restarts sequential decoding");
        for i in 2..24 {
            check(i);
        }
    }
    let before = filmcraft_codecs::gop_stats();
    for &i in SCRAMBLED {
        check(i);
    }
    let growth = filmcraft_codecs::gop_stats() - before;
    if intra {
        assert_eq!(growth.misses, SCRAMBLED.len() as u64, "distinct cold requests");
        assert_eq!(growth.decoded, growth.misses, "one sample per intra cache miss");
    } else if !all_first {
        assert!(growth.misses > 0, "cold requests must decode");
        assert!(growth.decoded >= growth.misses, "long-GOP cache decodes at least one sample per miss");
    }
    // Four workers request two distinct frames each after the measured requests.
    let start = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for worker in 0..4 {
            let (check, start) = (&check, &start);
            scope.spawn(move || {
                start.wait();
                for i in [1, 4, 7, 9, 10, 11, 13, 15].into_iter().skip(worker * 2).take(2) {
                    check(i);
                }
            });
        }
    });
}

/// Keep SPS/PPS and metadata with the first picture, stopping before the next picture.
fn first_access_unit(data: &[u8]) -> &[u8] {
    let mut seen_slice = false;
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i..].starts_with(&[0, 0, 1]) {
            let start = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            let payload = i + 3;
            let kind = data[payload] & 0x1f;
            let slice = matches!(kind, 1 | 5);
            let first_mb_zero = slice && data.get(payload + 1).is_some_and(|byte| byte & 0x80 != 0);
            if seen_slice && (matches!(kind, 6..=9) || first_mb_zero) {
                return &data[..start];
            }
            seen_slice |= slice;
            i += 3;
        } else {
            i += 1;
        }
    }
    assert!(seen_slice, "raw fixture contains a slice");
    data
}

#[test]
fn intra_containers_probes_and_fallback_are_bit_exact() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let intra = encode(&ff, "h264_intra_422.mp4", "yuv422p10le", "high422", "keyint=1:slices=8", None).expect("generate intra MP4");
    check_source(&ff, &intra, &intra, "yuv422p10le", true, false);

    if let Some(mxf) = encode(&ff, "h264_intra_422.mxf", "yuv422p10le", "high422", "keyint=1:slices=8", Some("mxf")) {
        check_source(&ff, &mxf, &mxf, "yuv422p10le", true, false);
    } else if let Some(mxf) = encode(&ff, "h264_intra_422_8.mxf", "yuv422p", "high422", "keyint=1:slices=8", Some("mxf")) {
        eprintln!("MXF 10-bit muxing unavailable; testing 8-bit 4:2:2");
        check_source(&ff, &mxf, &mxf, "yuv422p", true, false);
    } else {
        eprintln!("skipped intra MXF: ffmpeg cannot mux either 10-bit or 8-bit 4:2:2 H.264");
    }

    let long = encode(&ff, "h264_longgop.mp4", "yuv420p", "high", "bframes=2:keyint=12", None).expect("generate long-GOP MP4");
    check_source(&ff, &long, &long, "yuv420p", false, false);

    let liar = encode(&ff, "h264_liar.mp4", "yuv420p", "high", "bframes=0:keyint=8", None).expect("generate fallback MP4");
    let mut patched = std::fs::read(&liar).unwrap();
    // Restrict the SPS search to the avcC payload, excluding encoded slice bytes.
    let avcc = patched.windows(4).position(|v| v == b"avcC").expect("avcC box");
    assert!(avcc >= 4);
    let size = u32::from_be_bytes(patched[avcc - 4..avcc].try_into().unwrap()) as usize;
    let end = (avcc - 4).checked_add(size).expect("avcC size");
    let payload = patched.get(avcc + 4..end).expect("complete avcC box");
    let sps = payload.windows(3).position(|v| v == [0x67, 0x64, 0x00]).expect("High SPS with clear constraint flags");
    patched[avcc + 4 + sps + 2] = 0x10;
    let patched_path = common::dir().join("h264_liar_intra.mp4");
    let patched_path = filmcraft_testkit::fixtures::generate(&patched_path, |tmp| {
        std::fs::write(tmp, &patched).expect("write patched MP4");
        true
    })
    .expect("publish patched MP4");
    assert_eq!(std::fs::read(&patched_path).unwrap(), patched, "cached fallback fixture matches its source");
    check_source(&ff, &patched_path, &liar, "yuv420p", false, true);

    for (name, pix_fmt, profile, params, expected) in [
        ("h264_intra_422.h264", "yuv422p10le", "high422", "keyint=1:slices=8", true),
        ("h264_longgop.h264", "yuv420p", "high", "bframes=2:keyint=12", false),
    ] {
        let raw = encode(&ff, name, pix_fmt, profile, params, Some("h264")).expect("generate probe stream");
        let data = std::fs::read(raw).unwrap();
        let sample = first_access_unit(&data);
        let mut decoder = H264Decoder::annexb();
        assert!(!decoder.intra_only());
        assert_eq!(decoder.detect_intra_only(sample), expected, "{name}");
        assert_eq!(decoder.intra_only(), expected, "{name}");
    }
}
