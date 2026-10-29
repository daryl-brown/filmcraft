//! Slice scheduling must preserve native samples and output metadata.

use crate::common;
use filmcraft_h264::{Decoder, Picture};

fn hash(pictures: &[Picture]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for picture in pictures {
        for plane in [&picture.y, &picture.u, &picture.v] {
            for byte in plane.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    hash
}

fn equal(reference: &[Picture], actual: &[Picture], context: &str) {
    let (want, got) = (hash(reference), hash(actual));
    let diagnostic = format!("{context}: FNV-1a reference {want:016x}, actual {got:016x}");
    assert_eq!(reference.len(), actual.len(), "{diagnostic}");
    for (k, (a, b)) in reference.iter().zip(actual).enumerate() {
        assert_eq!((&a.y, &a.u, &a.v), (&b.y, &b.u, &b.v), "picture {k}: {diagnostic}");
        assert_eq!((a.pts, a.poc, a.key), (b.pts, b.poc, b.key), "picture {k}: {diagnostic}");
        assert_eq!((a.width, a.height, a.bit_depth, a.chroma_format_idc), (b.width, b.height, b.bit_depth, b.chroma_format_idc), "{diagnostic}");
        assert_eq!((a.chroma_width, a.chroma_height, a.y_stride, a.uv_stride), (b.chroma_width, b.chroma_height, b.y_stride, b.uv_stride), "{diagnostic}");
        assert_eq!((a.color, a.sar, a.draft), (b.color, b.sar, b.draft), "{diagnostic}");
    }
    assert_eq!(want, got, "{diagnostic}");
}

#[test]
fn parallel_matches_sequential_on_all_fixtures() {
    for fixture in common::FIXTURES {
        let Some((path, _)) = common::ensure(fixture) else { continue };
        let (reference, stats) = common::decode_file_with(&path, 1, false, false).unwrap();
        assert_eq!(stats.slice_parallel_pictures, 0, "{}", fixture.name);
        for threads in [1, 2, 3, 4, 8, 0] {
            let (actual, stats) = common::decode_file_with(&path, threads, true, false).unwrap();
            if threads == 1 {
                assert_eq!(stats.slice_parallel_pictures, 0, "{}", fixture.name);
            }
            equal(&reference, &actual, &format!("{} threads {threads}", fixture.name));
        }
        let (actual, _) = common::decode_file_decoder(&path, Decoder::with_shared_pool(), true, false).unwrap();
        equal(&reference, &actual, &format!("{} shared pool", fixture.name));
    }
}

#[test]
fn parallel_path_actually_runs() {
    for name in ["h422_10_slices", "h10_slices", "slices4", "slices4_cavlc"] {
        let Some((path, _)) = common::ensure(common::fixture(name)) else { continue };
        let (reference, off) = common::decode_file_with(&path, 4, false, false).unwrap();
        let (actual, on) = common::decode_file_with(&path, 4, true, false).unwrap();
        equal(&reference, &actual, name);
        assert_eq!(off.slice_parallel_pictures, 0, "{name}");
        if cfg!(all(feature = "threads", not(target_arch = "wasm32"))) && name.starts_with('h') {
            assert!(on.slice_parallel_pictures > 0, "{name}: {on:?}");
            assert_eq!(on.slice_parallel_pictures, actual.len() as u64, "{name}");
        } else {
            // These long-GOP fixtures use four slices for each I picture.
            assert!(on.slice_parallel_pictures <= on.slices_i / 4, "{name}: {on:?}");
        }
        if !cfg!(all(feature = "threads", not(target_arch = "wasm32"))) {
            assert_eq!(on.slice_parallel_pictures, 0, "{name}");
        }
    }
}

#[test]
fn parallel_matches_sequential_in_draft_mode() {
    for name in ["h422_10_slices", "h10_slices", "slices4", "slices4_cavlc"] {
        let Some((path, _)) = common::ensure(common::fixture(name)) else { continue };
        let (reference, _) = common::decode_file_with(&path, 1, false, true).unwrap();
        let (actual, _) = common::decode_file_with(&path, 4, true, true).unwrap();
        equal(&reference, &actual, &format!("{name} draft"));
    }
}

const INTRA_S8: &[&str] = &["h422_10_s8_cabac", "h422_10_s8_cavlc", "h10_s8_cabac"];

#[test]
fn intra_fixtures_are_detected_and_inter_ones_are_not() {
    for &name in INTRA_S8.iter().chain(["high_720p", "keyint10", "cavlc_b", "baseline_qcif"].iter()) {
        let Some((path, _)) = common::ensure(common::fixture(name)) else { continue };
        let data = std::fs::read(path).unwrap();
        let aus = common::split_access_units(&data);
        assert!(!aus.is_empty(), "{name}");
        for (i, au) in aus.into_iter().enumerate() {
            assert_eq!(filmcraft_h264::stream_is_intra_annexb(au), INTRA_S8.contains(&name), "{name} access unit {i}");
        }
    }
}

#[test]
fn intra_only_decoder_rejects_inter_pictures() {
    let Some((path, _)) = common::ensure(common::fixture("keyint10")) else { return };
    let data = std::fs::read(path).unwrap();
    let aus = common::split_access_units(&data);
    assert!(aus.len() > 1);
    let mut decoder = Decoder::with_threads(1);
    decoder.set_intra_only(true);
    decoder.decode(aus[0], 0).unwrap();
    assert!(matches!(decoder.decode(aus[1], 1), Err(filmcraft_h264::Error::Invalid(_))));
}

#[test]
fn shared_pool_decoders_match_default_on_intra_fixtures() {
    for &name in INTRA_S8 {
        let fixture = common::fixture(name);
        let Some((path, yuv)) = common::ensure(fixture) else { continue };
        let data = std::fs::read(&path).unwrap();
        let mut pictures = Vec::new();
        let mut parallel = 0;
        for (i, au) in common::split_access_units(&data).into_iter().enumerate() {
            let mut decoder = Decoder::with_shared_pool();
            decoder.set_intra_only(true);
            pictures.extend(decoder.decode(au, i as i64).unwrap());
            pictures.extend(decoder.flush());
            assert!(decoder.take_error().is_none(), "{name} access unit {i}");
            parallel += decoder.stats().slice_parallel_pictures;
        }
        common::compare(&pictures, &std::fs::read(yuv).unwrap(), fixture.width as usize, fixture.height as usize).unwrap();
        let (default, _) = common::decode_file_with(&path, 1, false, false).unwrap();
        equal(&default, &pictures, name);
        if filmcraft_h264::default_threads() > 1 {
            assert!(parallel > 0, "{name}");
        } else {
            assert_eq!(parallel, 0, "{name}: only one worker available");
        }
    }
}

#[test]
fn mid_row_slices_use_sequential_path() {
    let Some((path, _)) = common::ensure(common::fixture("h422_10_slicemax")) else { return };
    let (reference, _) = common::decode_file_with(&path, 1, false, false).unwrap();
    let (actual, stats) = common::decode_file_with(&path, 4, true, false).unwrap();
    equal(&reference, &actual, "mid-row slices");
    assert_eq!(stats.slice_parallel_pictures, 0);
}
