//! Multi-clip audio placement: `audio_placement_specs` and dynamic audio track allocation.

use filmcraft_media::MediaSource;
use filmcraft_project::{AudioChannels, ItemId, TrackKind};
use filmcraft_time::{FrameRate, Tick, TimeRange};

use serde_json::json;

use crate::Session;
use crate::commands::{AudioPlacementSpec, audio_placement_specs, ensure_audio_tracks};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn active(s: &Session) -> ItemId {
    s.state.active_sequence.expect("demo session has an active sequence")
}

#[test]
fn specs_always_place_one_clip_on_the_destination() {
    assert_eq!(audio_placement_specs(&[], 1), vec![AudioPlacementSpec { track_offset: 0, audio_stream: 0, source_channels: None }]);
    assert_eq!(audio_placement_specs(&[vec![0, 1]], 1).len(), 1);
}

#[test]
fn specs_put_further_clips_on_the_tracks_below() {
    let specs = audio_placement_specs(&[vec![0], vec![1], vec![2, 3]], 1);
    let got: Vec<(usize, Option<Vec<u16>>)> = specs.into_iter().map(|s| (s.track_offset, s.source_channels)).collect();
    assert_eq!(got, vec![(0, None), (1, Some(vec![1])), (2, Some(vec![2, 3]))]);
}

#[test]
fn specs_are_capped_for_hostile_channel_maps() {
    let clips = vec![vec![0u16]; 100_000];
    assert!(audio_placement_specs(&clips, usize::MAX).len() <= crate::sequence_tools::MAX_TRACKS);
}

#[test]
fn specs_append_container_streams_after_channel_map_clips() {
    let specs = audio_placement_specs(&[vec![1], vec![0]], 3);
    assert_eq!(
        specs.iter().map(|s| (s.track_offset, s.audio_stream, s.source_channels.clone())).collect::<Vec<_>>(),
        vec![(0, 0, None), (1, 0, Some(vec![0])), (2, 1, None), (3, 2, None)]
    );
    assert_eq!(audio_placement_specs(&[], 0).len(), 1);
}

#[test]
fn placing_media_with_two_streams_expands_tracks_and_keeps_stream_indices() {
    let mut s = Session::default();
    let mut p = (*s.project).clone();
    let rate = FrameRate::FPS_24;
    let seq = p.new_sequence("Two streams", Default::default(), 1, 1, None);
    let source = filmcraft_media::generators::GeneratorSource::new(filmcraft_media::Generator::BarsAndTone, 32, 32, rate, rate.tick_of(24));
    let mut info = source.info().clone();
    info.audio_streams.push(info.audio_streams[0].clone());
    let item = p.add_item(
        "two streams",
        filmcraft_project::Label::Iris,
        filmcraft_project::ItemKind::Media(Box::new(filmcraft_project::MediaClip {
            media: filmcraft_project::MediaRef::Generator(filmcraft_media::Generator::BarsAndTone),
            info,
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        })),
        None,
    );
    let (vdest, adest) = {
        let q = p.sequence(seq).unwrap();
        (q.video_tracks[0].id, q.audio_tracks[0].id)
    };
    s.project = std::sync::Arc::new(p);
    s.state.active_sequence = Some(seq);
    let ids = crate::commands::place_item(
        &mut s,
        crate::commands::ItemPlacement {
            item,
            range: TimeRange::new(Tick::ZERO, rate.tick_of(24)),
            at: Tick::ZERO,
            vdest: Some(vdest),
            adest: Some(adest),
            insert: false,
            label: "test",
            audio_split: None,
        },
    )
    .unwrap();
    assert_eq!(ids.len(), 3);
    let q = s.project.sequence(seq).unwrap();
    assert_eq!(q.audio_tracks.len(), 2);
    let (a, b) = (&q.audio_tracks[0].items[0], &q.audio_tracks[1].items[0]);
    assert_eq!((a.audio_stream, b.audio_stream), (0, 1));
    assert_eq!(a.link, b.link);
    assert!(a.link.is_some());
    assert_ne!(a.id, b.id);
    assert_eq!(a.source_in, b.source_in);
    assert_eq!(a.start, b.start);
}

#[test]
fn clip_peaks_measure_the_stream_the_clip_plays() {
    let mut s = Session::default();
    let mut p = (*s.project).clone();
    let rate = FrameRate::FPS_24;
    let seq = p.new_sequence("Two streams", Default::default(), 1, 1, None);
    let source = filmcraft_media::generators::GeneratorSource::new(filmcraft_media::Generator::BarsAndTone, 32, 32, rate, rate.tick_of(24));
    let mut info = source.info().clone();
    info.audio_streams.push(info.audio_streams[0].clone());
    let item = p.add_item(
        "two streams",
        filmcraft_project::Label::Iris,
        filmcraft_project::ItemKind::Media(Box::new(filmcraft_project::MediaClip {
            media: filmcraft_project::MediaRef::Generator(filmcraft_media::Generator::BarsAndTone),
            info,
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        })),
        None,
    );
    let (vdest, adest) = {
        let q = p.sequence(seq).unwrap();
        (q.video_tracks[0].id, q.audio_tracks[0].id)
    };
    s.project = std::sync::Arc::new(p);
    s.state.active_sequence = Some(seq);
    let ids = crate::commands::place_item(
        &mut s,
        crate::commands::ItemPlacement {
            item,
            range: TimeRange::new(Tick::ZERO, rate.tick_of(24)),
            at: Tick::ZERO,
            vdest: Some(vdest),
            adest: Some(adest),
            insert: false,
            label: "test",
            audio_split: None,
        },
    )
    .unwrap();
    let q = s.project.sequence(seq).unwrap();
    let (a, b) = (q.audio_tracks[0].items[0].id, q.audio_tracks[1].items[0].id);
    assert!(ids.contains(&a) && ids.contains(&b));
    let peaks = crate::mixer::clip_peaks(&s, &[a, b]);
    let db = |c: filmcraft_project::ClipId| peaks.iter().find(|(id, _)| *id == c).map(|(_, d)| *d).unwrap();
    // The generator only has stream 0, so the clip playing stream 1 is silent, not a copy of stream 0.
    assert!(db(a) > -30.0, "{peaks:?}");
    assert!(db(b) < -100.0, "{peaks:?}");
}

#[test]
fn missing_audio_tracks_are_cloned_from_the_destination() {
    let s = demo();
    let seq = active(&s);
    let mut p = (*s.project).clone();
    let have = p.sequence(seq).map(|q| q.audio_tracks.len()).unwrap_or(0);
    assert!(have >= 1);
    if let Some(t) = p.sequence_mut(seq).and_then(|q| q.audio_tracks.get_mut(0)) {
        t.channels = AudioChannels::Mono;
        t.volume_db = -3.0;
        t.locked = true;
    }
    let ids = ensure_audio_tracks(&mut p, seq, 0, have + 3, "test").unwrap();
    assert_eq!(ids.len(), have + 3);
    let q = p.sequence(seq).unwrap();
    for t in &q.audio_tracks[have..] {
        assert_eq!(t.kind, TrackKind::Audio);
        assert_eq!(t.channels, AudioChannels::Mono);
        assert_eq!(t.volume_db, -3.0);
        assert!(t.items.is_empty() && !t.locked);
    }
    let mut unique = ids.clone();
    unique.sort_by_key(|t| t.0);
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "track ids are unique");
    // enough tracks already: nothing is added
    let again = ensure_audio_tracks(&mut p, seq, 0, have + 3, "test").unwrap();
    assert_eq!(again, ids);
}

/// #823: a container with two PCM tracks imports both streams, and overwrite places linked clips.
#[test]
fn import_keeps_every_audio_stream_and_places_linked_clips() {
    use filmcraft_isobmff::{Brand, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
    let mut mux = Mp4Writer::new(std::io::Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
    for (rate, value) in [(48_000u32, 8192i16), (24_000, -16384)] {
        let entry = SampleEntry::pcm(PcmConfig { bits: 16, float: false, big_endian: false, signed: true, channels: 1, sample_rate: rate as f64 });
        let track = mux.add_track(TrackConfig::new(entry, rate)).unwrap();
        let data: Vec<u8> = (0..rate / 10).flat_map(|_| value.to_le_bytes()).collect();
        mux.write_sample(track, WriteSample { data: &data, duration: rate / 10, composition_offset: 0, is_sync: true }).unwrap();
    }
    let bytes = mux.finish().unwrap().into_inner();
    let mut s = Session::default();
    let item = crate::commands::import_bytes(&mut s, "/two-audio.mov", bytes.into(), None).unwrap();
    let streams = s.project.item(item).unwrap().as_media().unwrap().info.audio_streams.len();
    assert_eq!(streams, 2);
    s.execute("file.newSequence", json!({"name": "S", "fromItem": item.0, "video": 1, "audio": 1})).unwrap();
    let seq = s.state.active_sequence.unwrap();
    let q = s.project.sequence(seq).unwrap();
    assert_eq!(q.audio_tracks.len(), 2);
    let (a, b) = (&q.audio_tracks[0].items[0], &q.audio_tracks[1].items[0]);
    assert_eq!((a.audio_stream, b.audio_stream), (0, 1));
    assert_eq!(a.link, b.link);
    assert!(a.link.is_some());
}

#[test]
fn hostile_track_counts_are_an_error_not_a_crash() {
    let s = demo();
    let seq = active(&s);
    let mut p = (*s.project).clone();
    assert!(ensure_audio_tracks(&mut p, seq, usize::MAX, usize::MAX, "test").is_err());
    assert!(ensure_audio_tracks(&mut p, seq, 0, 1_000_000, "test").is_err());
    assert!(ensure_audio_tracks(&mut p, ItemId(u64::MAX), 0, 1, "test").is_err());
}

#[test]
fn multichannel_import_defaults_cover_formats_and_caps() {
    use filmcraft_project::AudioChannelMap;

    use crate::commands::default_audio_channels;
    for pref in ["mono", "stereo", "asIs", "unknown"] {
        for channels in [0, 1, 2, 3, 6, 8, usize::MAX] {
            let got = default_audio_channels(pref, channels);
            if channels <= 2 || pref == "asIs" {
                assert_eq!(got, None, "{pref}, {channels}");
                continue;
            }
            let format = if pref == "stereo" { AudioChannels::Stereo } else { AudioChannels::Mono };
            let mut want = AudioChannelMap::for_format(format, u16::try_from(channels).unwrap_or(u16::MAX));
            want.clips.truncate(crate::sequence_tools::MAX_TRACKS);
            assert_eq!(got, Some(want), "{pref}, {channels}");
            if channels == usize::MAX {
                let map = default_audio_channels(pref, channels).unwrap();
                assert_eq!(map.clips.len(), crate::sequence_tools::MAX_TRACKS);
                let last = u16::try_from(crate::sequence_tools::MAX_TRACKS - 1).unwrap();
                let expected = if pref == "stereo" { vec![last * 2, last * 2 + 1] } else { vec![last] };
                assert_eq!(map.clips.last(), Some(&expected));
            }
        }
    }
    assert_eq!(default_audio_channels("stereo", 3).unwrap().clips, vec![vec![0, 1], vec![2]]);
}

/// Exact binary fractions remain distinguishable after the fixture's 16-bit PCM encoding.
fn multichannel_wav(channels: u16) -> Vec<u8> {
    let samples: Vec<f32> = (0..4800).flat_map(|_| (0..channels).map(|k| f32::from(k + 1) / 128.0)).collect();
    filmcraft_media::wav::write_wav16(&samples, channels, 48_000)
}

fn import_multichannel(s: &mut Session, channels: u16) -> ItemId {
    crate::commands::import_bytes(s, "/multichannel.wav", multichannel_wav(channels).into(), None).unwrap()
}

fn sequence_from_audio(s: &mut Session, item: ItemId) {
    s.execute("file.newSequence", json!({"name": "Audio", "fromItem": item.0, "video": 1, "audio": 1})).unwrap();
}

#[test]
fn multichannel_wav_import_places_every_channel_with_one_link() {
    use filmcraft_project::AudioChannelMap;
    for channels in [3u16, 6, 8] {
        let mut s = Session::default();
        let item = import_multichannel(&mut s, channels);
        let media = s.project.item(item).unwrap().as_media().unwrap();
        assert_eq!(media.info.audio_streams.len(), 1);
        assert_eq!(media.info.audio().unwrap().channels, u32::from(channels));
        assert_eq!(media.interpret.audio_channels, Some(AudioChannelMap::for_format(AudioChannels::Mono, channels)));
        sequence_from_audio(&mut s, item);
        let q = s.active_sequence().unwrap();
        assert_eq!(q.audio_tracks.len(), usize::from(channels));
        let link = q.audio_tracks[0].items[0].link;
        assert!(link.is_some());
        for (k, track) in q.audio_tracks.iter().enumerate() {
            assert_eq!(track.items.len(), 1);
            let clip = &track.items[0];
            assert_eq!(clip.source_channels, vec![u16::try_from(k).unwrap()]);
            assert_eq!(clip.audio_stream, 0);
            assert_eq!(clip.link, link);
        }
    }
}

#[test]
fn multichannel_import_preferences_control_pairs_and_preserve_small_sources() {
    use filmcraft_project::AudioChannelMap;
    for pref in ["mono", "stereo", "asIs"] {
        for channels in [1u16, 2, 4] {
            let mut s = Session::default();
            s.execute("prefs.set", json!({"key": "media.defaultAudioChannels", "value": pref})).unwrap();
            let item = import_multichannel(&mut s, channels);
            let want = match (channels, pref) {
                (4, "mono") => Some(AudioChannelMap::for_format(AudioChannels::Mono, 4)),
                (4, "stereo") => Some(AudioChannelMap::for_format(AudioChannels::Stereo, 4)),
                _ => None,
            };
            assert_eq!(s.project.item(item).unwrap().as_media().unwrap().interpret.audio_channels, want);
            sequence_from_audio(&mut s, item);
            let tracks = &s.active_sequence().unwrap().audio_tracks;
            let expected = want.map(|m| m.clips).unwrap_or_else(|| vec![vec![]]);
            assert_eq!(tracks.len(), expected.len());
            for (track, channels) in tracks.iter().zip(expected) {
                assert_eq!(track.items.len(), 1);
                assert_eq!(track.items[0].source_channels, channels);
            }
        }
    }
}

#[test]
fn imported_eight_channel_clips_render_only_their_mapped_samples() {
    let mut s = Session::default();
    let item = import_multichannel(&mut s, 8);
    sequence_from_audio(&mut s, item);
    let q = s.active_sequence().unwrap();
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    for (k, track) in q.audio_tracks.iter().enumerate() {
        let sound = filmcraft_render::audio::track_input(&s.project, track, 0, 128, 48_000, &provider);
        assert_eq!(sound.channels.len(), 2);
        let want = (k + 1) as f32 / 128.0;
        assert!(sound.channels.iter().all(|c| c.len() == 128));
        assert!(sound.channels.iter().flatten().all(|v| (*v - want).abs() < 1e-6), "channel {k}: {:?}", sound.channels);
    }
}

/// Audio-only OP1a, using the writer's existing multitrack PCM support.
fn eight_mono_track_mxf() -> Vec<u8> {
    use filmcraft_mxf::Rational;
    use filmcraft_mxf::write::{MxfWriter, PackageIds, Pattern, SoundDesc, WriterConfig};
    let mut cfg = WriterConfig::new(Pattern::Op1a, Rational::new(25, 1), PackageIds::from_seed("eight-mono", "Eight mono tracks"));
    cfg.sound = vec![SoundDesc { sample_rate: 48_000, channels: 1, bits: 24 }; 8];
    let mut writer = MxfWriter::new(std::io::Cursor::new(Vec::new()), cfg).unwrap();
    for k in 0..8i32 {
        let sample = ((k + 1) * 65_536).to_le_bytes();
        let pcm: Vec<u8> = (0..1920).flat_map(|_| sample[..3].iter().copied()).collect();
        writer.push_sound(usize::try_from(k).unwrap(), &pcm).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn mxf_mono_sound_tracks_import_as_one_stream_with_a_mono_breakout() {
    let mut s = Session::default();
    let item = crate::commands::import_bytes(&mut s, "/eight-mono.mxf", eight_mono_track_mxf().into(), None).unwrap();
    let media = s.project.item(item).unwrap().as_media().unwrap();
    assert_eq!(media.info.audio_streams.len(), 1);
    assert_eq!(media.info.audio().unwrap().channels, 8);
    assert_eq!(media.info.audio().unwrap().bits_per_sample, Some(24));
    assert_eq!(media.interpret.audio_channels, Some(filmcraft_project::AudioChannelMap::for_format(AudioChannels::Mono, 8)));
    sequence_from_audio(&mut s, item);
    assert_eq!(s.active_sequence().unwrap().audio_tracks.len(), 8);
}

#[test]
fn imported_mono_breakout_survives_project_save_and_open() {
    let dir = crate::media_test_util::tmp_dir("audio-breakout-roundtrip");
    let path = dir.join("audio.fcproj");
    let mut s = Session::default();
    let item = import_multichannel(&mut s, 8);
    let want = s.project.item(item).unwrap().as_media().unwrap().interpret.audio_channels.clone();
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let mut loaded = Session::default();
    loaded.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(loaded.project.item(item).unwrap().as_media().unwrap().interpret.audio_channels, want);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn import_maps_only_stream_zero_and_keeps_later_stream_layouts() {
    use filmcraft_isobmff::{Brand, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
    let mut mux = Mp4Writer::new(std::io::Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
    for channels in [4u16, 2] {
        let entry = SampleEntry::pcm(PcmConfig { bits: 16, float: false, big_endian: false, signed: true, channels, sample_rate: 48_000.0 });
        let track = mux.add_track(TrackConfig::new(entry, 48_000)).unwrap();
        let data = vec![0u8; 4800 * usize::from(channels) * 2];
        mux.write_sample(track, WriteSample { data: &data, duration: 4800, composition_offset: 0, is_sync: true }).unwrap();
    }
    let mut s = Session::default();
    let item = crate::commands::import_bytes(&mut s, "/mapped-stream.mov", mux.finish().unwrap().into_inner().into(), None).unwrap();
    let media = s.project.item(item).unwrap().as_media().unwrap();
    assert_eq!(media.info.audio_streams.iter().map(|a| a.channels).collect::<Vec<_>>(), vec![4, 2]);
    assert_eq!(media.interpret.audio_channels, Some(filmcraft_project::AudioChannelMap::for_format(AudioChannels::Mono, 4)));
    sequence_from_audio(&mut s, item);
    let tracks = &s.active_sequence().unwrap().audio_tracks;
    assert_eq!(tracks.len(), 5);
    for (k, track) in tracks.iter().enumerate() {
        assert_eq!(track.items.len(), 1);
        let clip = &track.items[0];
        assert_eq!(clip.audio_stream, usize::from(k == 4));
        assert_eq!(clip.source_channels, if k == 4 { vec![] } else { vec![u16::try_from(k).unwrap()] });
        assert_eq!(clip.link, tracks[0].items[0].link);
    }
}
