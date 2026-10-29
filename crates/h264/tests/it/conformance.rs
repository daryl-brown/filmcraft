//! Bit-exactness against ffmpeg's decoder on libx264-generated fixtures (skipped when ffmpeg is absent).

use crate::common;

macro_rules! fixture_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                match common::check_fixture(stringify!($name)) {
                    Ok(true) => {}
                    Ok(false) => eprintln!("skipped {}", stringify!($name)),
                    Err(e) => panic!("{e}"),
                }
            }
        )*
    };
}

fixture_tests!(
    h422_10_s8_cavlc,
    h422_10_s8_cabac,
    h422_10_s8_cavlc_nodeblock,
    h422_10_s8_cabac_nodeblock,
    h422_10_s8_cabac_deblock,
    h422_8_s8_cabac,
    h10_s8_cabac,
    h422_10_slicemax,

    h422_8_cabac0_t0,
    h422_8_cabac0_t1,
    h422_8_cabac1_t0,
    h422_8_cabac1_t1,
    h422_10_cabac0_t0,
    h422_10_cabac0_t1,
    h422_10_cabac1_t0,
    h422_10_cabac1_t1,
    h422_10_cqm,
    h422_10_qp1,
    h422_10_qp50,
    h422_10_nodeblock,
    h422_10_deblock,
    h422_10_slices,
    h422_10_chroma_m6,
    h422_10_chroma_p6,
    h422_10_constrained,
    h422_10_crop,
    h422_10_large,

    h10_intra_cabac0_t0,
    h10_intra_cabac0_t1,
    h10_intra_cabac1_t0,
    h10_intra_cabac1_t1,
    h10_cqm,
    h10_qp1,
    h10_qp50,
    h10_nodeblock,
    h10_deblock,
    h10_slices,
    h10_chroma_m6,
    h10_chroma_p6,
    h10_crop,
    h10_bpyramid,
    h10_weighted,
    h10_temporal,
    intra_cavlc,
    intra_cavlc_noise,
    intra_cavlc_8x8,
    intra_cavlc_cqm,
    p_cavlc_nodeblock,
    baseline_qcif,
    baseline_cif_noise,
    cavlc_b,
    cavlc_b_temporal,
    cavlc_weightp,
    slices4_cavlc,
    qp1_cavlc,
    main_cabac_b,
    high_720p,
    high_1080p,
    crop_1918x1078,
    bpyramid,
    weightp2,
    weightb,
    direct_temporal,
    direct_spatial,
    ref4,
    no_deblock,
    deblock_m2_2,
    cqm_jvt,
    slices4,
    keyint10,
    open_gop,
    constrained_intra,
    no8x8dct,
    qp50,
    qp1,
    vt_high,
    vt_main,
    vt_baseline,
    vt_high_1080p,
);

/// Draft mode (reduced-resolution playback) skips deblocking of non-reference pictures only:
/// every other picture stays bit-exact with ffmpeg, single- and frame-threaded, the skipped ones
/// are flagged, and they are the only ones that differ.
#[test]
fn draft_mode_changes_only_flagged_non_reference_pictures() {
    let mut checked = 0;
    for name in ["main_cabac_b", "bpyramid", "cavlc_b", "direct_temporal", "weightb", "h10_bpyramid", "h10_weighted"] {
        let f = common::fixture(name);
        let Some((h264, yuv)) = common::ensure(f) else {
            eprintln!("skipped {name}");
            continue;
        };
        let reference = std::fs::read(&yuv).unwrap();
        let (w, h) = (f.width as usize, f.height as usize);
        let fsize = (w * h + 2 * w.div_ceil(2) * h.div_ceil(2)) * if f.pix_fmt == "yuv420p" { 1 } else { 2 };
        for threads in [1, 0] {
            let pics = common::decode_file_opts(&h264, threads, true).unwrap_or_else(|(au, e, _)| panic!("{name}: error at {au}: {e}"));
            common::check_pts(&pics).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(pics.len() * fsize, reference.len(), "{name}: picture count");
            let (mut draft, mut changed) = (0, 0);
            for (i, p) in pics.iter().enumerate() {
                let r = &reference[i * fsize..(i + 1) * fsize];
                let same = common::compare(std::slice::from_ref(p), r, w, h).is_ok();
                if p.draft {
                    draft += 1;
                    changed += !same as usize;
                } else {
                    assert!(same, "{name} (threads {threads}): picture {i} (not draft) differs from the reference");
                }
            }
            assert!(draft > 0, "{name}: has non-reference pictures");
            if name != "no_deblock" {
                assert!(changed > 0, "{name}: skipping deblocking changes some draft picture");
            }
            checked += 1;
        }
    }
    if checked == 0 {
        eprintln!("skipped: no fixtures");
    }
}

/// Print which coding tools each fixture exercises (run with `-- --ignored --nocapture`).
#[test]
#[ignore]
fn coverage_report() {
    for f in common::FIXTURES {
        let Some((h264, _)) = common::ensure(f) else { return };
        let data = std::fs::read(h264).unwrap();
        let mut dec = filmcraft_h264::Decoder::new();
        for au in common::split_access_units(&data) {
            dec.decode(au, 0).unwrap();
        }
        dec.flush();
        let s = dec.stats();
        println!(
            "{:<20} pics {:>3} cavlc/cabac {:>3}/{:<3} I/P/B {:>3}/{:>3}/{:>3} i4 {:>6} i8 {:>6} i16 {:>6} pcm {:>4} pskip {:>6} bskip {:>6} bdirect {:>5} inter {:>6} t8 {:>6} mmco {:>3} lt {} gaps {} wp {} tdirect {}",
            f.name,
            s.pictures,
            s.slices_cavlc,
            s.slices_cabac,
            s.slices_i,
            s.slices_p,
            s.slices_b,
            s.mb_i4x4,
            s.mb_i8x8,
            s.mb_i16x16,
            s.mb_pcm,
            s.mb_p_skip,
            s.mb_b_skip,
            s.mb_b_direct16x16,
            s.mb_inter,
            s.mb_inter_8x8_transform,
            s.mmco_ops,
            s.long_term_marks,
            s.frame_num_gaps,
            s.weighted_slices,
            s.temporal_direct_slices
        );
    }
}

/// Pre-generate every fixture and its reference decode (`cargo xtask fixtures`).
#[test]
#[ignore]
fn generate_fixtures() {
    let dir = common::fixtures_dir();
    for f in common::FIXTURES {
        let outs = [dir.join(format!("{}.h264", f.name)), dir.join(format!("{}.yuv", f.name))];
        filmcraft_testkit::fixtures::generate_and_report(&format!("h264/{}", f.name), &outs, || common::ensure(f));
    }
}

/// Concurrent consumers must publish one complete fixture without replacing an open file.
#[test]
fn concurrent_fixture_publication_is_safe() {
    if filmcraft_testkit::ffmpeg_or_skip("concurrent_fixture_publication").is_none() {
        return;
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let name = Box::leak(format!("concurrent_fixture_{}_{stamp}", std::process::id()).into_boxed_str());
    let fixture =
        common::Fixture { pix_fmt: "yuv420p", name, source: "testsrc2", width: 64, height: 48, frames: 2, filter: "", args: &["-preset", "ultrafast", "-x264-params", "keyint=1"] };
    let open_published = |path: &std::path::Path| {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Like the external decoder, allow reads but deny replacement while open.
            options.share_mode(1);
        }
        options.open(path).unwrap()
    };
    let start = std::sync::Barrier::new(4);
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    let paths = common::ensure(&fixture).expect("software fixture must generate");
                    // Keep both published files open until every producer has finished.
                    let files = (open_published(&paths.0), open_published(&paths.1));
                    (paths, files)
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().expect("fixture producer panicked")).collect::<Vec<_>>()
    });
    let paths = results[0].0.clone();
    for (got, _) in &results {
        assert_eq!(got, &paths);
    }
    assert!(!std::fs::read(&paths.0).unwrap().is_empty());
    assert_eq!(std::fs::metadata(&paths.1).unwrap().len(), 2 * (64 * 48 + 2 * 32 * 24));
    drop(results);
    std::fs::remove_file(paths.0).unwrap();
    std::fs::remove_file(paths.1).unwrap();
}

/// Long-GOP High 4:2:2 must fail at the first inter slice, before reconstruction.
#[test]
fn h422_long_gop_returns_unsupported() {
    let f = common::Fixture {
        name: "h422_long_gop", pix_fmt: "yuv422p10le", source: "testsrc2",
        width: 64, height: 64, frames: 6, filter: "",
        args: &["-profile:v", "high422", "-x264-params", "keyint=30:min-keyint=30:scenecut=0:bframes=2:b-adapt=0"],
    };
    let Some((path, _)) = common::ensure(&f) else { return };
    let data = std::fs::read(path).unwrap();
    for threads in [1, 3, 0] {
        let outcome = std::panic::catch_unwind(|| {
            let mut decoder = filmcraft_h264::Decoder::with_threads(threads);
            let mut rejected = false;
            for au in common::split_access_units(&data) {
                if let Err(error) = decoder.decode(au, 0) {
                    assert!(matches!(error, filmcraft_h264::Error::Unsupported(ref message) if message == "inter prediction for 4:2:2"));
                    rejected = true;
                    break;
                }
            }
            decoder.flush();
            assert!(decoder.take_error().is_none());
            assert!(rejected, "inter slice must be rejected");
        });
        assert!(outcome.is_ok(), "long-GOP rejection panicked");
    }
}

/// Intra reference pictures keep exact output in draft mode, including frame threads.
#[test]
fn h422_draft_and_stats() {
    for name in ["h422_8_cabac0_t0", "h422_10_cabac1_t1"] {
        let f = common::fixture(name);
        let Some((path, yuv)) = common::ensure(f) else { return };
        let reference = std::fs::read(yuv).unwrap();
        for threads in [1, 3, 0] {
            let pics = common::decode_file_opts(&path, threads, true).unwrap();
            common::compare(&pics, &reference, f.width as usize, f.height as usize).unwrap();
            assert!(pics.iter().all(|p| !p.draft && p.chroma_format_idc == 2));
            let mut decoder = filmcraft_h264::Decoder::with_threads(threads);
            decoder.decode(&std::fs::read(&path).unwrap(), 0).unwrap();
            decoder.flush();
            assert!(decoder.take_error().is_none());
            let stats = decoder.stats();
            assert_eq!(stats.pictures, u64::from(f.frames));
            assert_eq!(stats.slices_i, u64::from(f.frames));
            assert!(stats.mb_i4x4 + stats.mb_i8x8 + stats.mb_i16x16 > 0);
        }
    }
}
