//! Final Cut Pro 7 XML (xmeml): a hand-written Premiere-style document (written from the public
//! xmeml element descriptions) and export → import round trips.

use crate::support;

use filmcraft_interchange::{ExportOptions, Format, Imported, detect, export, import};
use filmcraft_media::Generator;
use filmcraft_project::{ItemKind, Keyframe, Label, Marker, MarkerId, MarkerKind, MediaRef, Param, ParamValue, Project, Sequence, TrackKind, TransitionAlign};
use filmcraft_time::{FrameRate, Tick};
use proptest::prelude::*;
use support::*;

const RATE: &str = "<rate><timebase>24</timebase><ntsc>TRUE</ntsc></rate>";

fn premiere_style_doc() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="4">
  <project>
    <name>Lighthouse</name>
    <children>
      <bin>
        <name>Footage</name>
        <children>
          <clip id="masterclip-1">
            <name>tower.mov</name>
            <duration>2400</duration>
            {RATE}
            <labels><label2>Mango</label2></labels>
            <media><video><track><clipitem id="clipitem-m1"><name>tower.mov</name>
              <file id="file-1">
                <name>tower.mov</name>
                <pathurl>file://localhost/Volumes/Shoot/tower%20wide.mov</pathurl>
                {RATE}
                <duration>2400</duration>
                <timecode>{RATE}<string>01:00:00:00</string><frame>86400</frame><displayformat>NDF</displayformat></timecode>
                <media>
                  <video><samplecharacteristics>{RATE}<width>3840</width><height>2160</height></samplecharacteristics></video>
                  <audio><samplecharacteristics><depth>24</depth><samplerate>48000</samplerate></samplecharacteristics><channelcount>2</channelcount></audio>
                </media>
              </file>
            </clipitem></track></video></media>
          </clip>
        </children>
      </bin>
      <sequence id="sequence-1">
        <name>Main Edit</name>
        <duration>130</duration>
        {RATE}
        <timecode>{RATE}<string>01:00:00:00</string><frame>86400</frame><displayformat>NDF</displayformat></timecode>
        <media>
          <video>
            <format><samplecharacteristics>{RATE}<width>1920</width><height>1080</height><pixelaspectratio>square</pixelaspectratio></samplecharacteristics></format>
            <track>
              <clipitem id="clipitem-1">
                <masterclipid>masterclip-1</masterclipid>
                <name>tower.mov</name>
                <enabled>TRUE</enabled>
                {RATE}
                <start>0</start><end>-1</end><in>0</in><out>48</out>
                <file id="file-1"/>
                <link><linkclipref>clipitem-1</linkclipref><mediatype>video</mediatype><trackindex>1</trackindex><clipindex>1</clipindex></link>
                <link><linkclipref>clipitem-3</linkclipref><mediatype>audio</mediatype><trackindex>1</trackindex><clipindex>1</clipindex></link>
                <labels><label2>Cerulean</label2></labels>
              </clipitem>
              <transitionitem>
                {RATE}
                <start>36</start><end>60</end><alignment>center</alignment>
                <effect><name>Cross Dissolve</name><effectid>Cross Dissolve</effectid><effectcategory>Dissolve</effectcategory><effecttype>transition</effecttype><mediatype>video</mediatype></effect>
              </transitionitem>
              <clipitem id="clipitem-2">
                <name>waves</name>
                {RATE}
                <start>-1</start><end>120</end><in>100</in><out>172</out>
                <file id="file-2">
                  <name>waves.mov</name>
                  <pathurl>media/waves.mov</pathurl>
                  {RATE}
                  <duration>1000</duration>
                  <media><video><samplecharacteristics><width>1920</width><height>1080</height></samplecharacteristics></video></media>
                </file>
                <filter><effect>
                  <name>Basic Motion</name><effectid>basic</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype>
                  <parameter><parameterid>scale</parameterid><name>Scale</name><valuemin>0</valuemin><valuemax>1000</valuemax><value>100</value>
                    <keyframe><when>100</when><value>100</value></keyframe>
                    <keyframe><when>160</when><value>150</value></keyframe>
                  </parameter>
                  <parameter><parameterid>rotation</parameterid><name>Rotation</name><value>-12.5</value></parameter>
                  <parameter><parameterid>center</parameterid><name>Center</name><value><horiz>0.25</horiz><vert>-0.1</vert></value></parameter>
                </effect></filter>
                <filter><effect>
                  <name>Opacity</name><effectid>opacity</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype>
                  <parameter><parameterid>opacity</parameterid><name>opacity</name><valuemin>0</valuemin><valuemax>100</valuemax><value>50</value></parameter>
                </effect></filter>
                <filter><effect>
                  <name>Gaussian Blur</name><effectid>GaussianBlur</effectid><effecttype>filter</effecttype><mediatype>video</mediatype>
                </effect></filter>
                <filter><effect>
                  <name>Mystery Glow Pro</name><effectid>mystery</effectid><effecttype>filter</effecttype><mediatype>video</mediatype>
                </effect></filter>
                <marker><name>wave peak</name><comment>use this</comment><in>130</in><out>-1</out></marker>
              </clipitem>
            </track>
            <track>
              <clipitem id="clipitem-4">
                <name>Titles Nest</name>
                {RATE}
                <start>24</start><end>72</end><in>0</in><out>48</out>
                <sequence id="sequence-2"/>
              </clipitem>
              <generatoritem id="clipitem-7">
                <name>Red Matte</name>
                {RATE}
                <start>100</start><end>130</end><in>0</in><out>30</out>
                <effect><name>Color</name><effectid>Color</effectid><effectcategory>Matte</effectcategory><effecttype>generator</effecttype><mediatype>video</mediatype>
                  <parameter><parameterid>fillcolor</parameterid><name>Color</name><value><alpha>255</alpha><red>255</red><green>0</green><blue>0</blue></value></parameter>
                </effect>
              </generatoritem>
              <enabled>TRUE</enabled>
              <locked>TRUE</locked>
            </track>
          </video>
          <audio>
            <track>
              <clipitem id="clipitem-3">
                <name>tower.mov</name>
                {RATE}
                <start>0</start><end>48</end><in>0</in><out>48</out>
                <file id="file-1"/>
                <sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack>
                <filter><effect>
                  <name>Audio Levels</name><effectid>audiolevels</effectid><effectcategory>audiolevels</effectcategory><effecttype>audiolevels</effecttype><mediatype>audio</mediatype>
                  <parameter><parameterid>level</parameterid><name>Level</name><valuemin>0</valuemin><valuemax>3.98109</valuemax><value>0.5</value>
                    <keyframe><when>0</when><value>1</value></keyframe>
                    <keyframe><when>24</when><value>0.5</value></keyframe>
                  </parameter>
                </effect></filter>
                <link><linkclipref>clipitem-1</linkclipref><mediatype>video</mediatype></link>
                <link><linkclipref>clipitem-3</linkclipref><mediatype>audio</mediatype></link>
              </clipitem>
            </track>
            <track>
              <clipitem id="clipitem-5">
                <name>music.wav</name>
                <enabled>FALSE</enabled>
                {RATE}
                <start>0</start><end>120</end><in>0</in><out>120</out>
                <file id="file-3"><name>music.wav</name><pathurl>file:///Volumes/Audio/music.wav</pathurl>
                  <media><audio><samplecharacteristics><samplerate>44100</samplerate></samplecharacteristics><channelcount>2</channelcount></audio></media>
                </file>
              </clipitem>
            </track>
          </audio>
        </media>
        <marker><name>Check sky</name><comment></comment><in>30</in><out>-1</out></marker>
      </sequence>
      <sequence id="sequence-2">
        <name>Titles Nest</name>
        {RATE}
        <media>
          <video>
            <track>
              <clipitem id="clipitem-6">
                <name>waves fast</name>
                {RATE}
                <start>0</start><end>48</end><in>0</in><out>96</out>
                <file id="file-2"/>
                <filter><effect><name>Time Remap</name><effectid>timeremap</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype>
                  <parameter><parameterid>speed</parameterid><name>speed</name><value>200</value></parameter>
                  <parameter><parameterid>reverse</parameterid><name>reverse</name><value>FALSE</value></parameter>
                </effect></filter>
              </clipitem>
            </track>
          </video>
        </media>
      </sequence>
    </children>
  </project>
</xmeml>
"#
    )
}

#[test]
fn golden_premiere_style_nested_and_transitions() {
    let doc = premiere_style_doc();
    assert_eq!(detect(doc.as_bytes(), Some("xml")), Some(Format::Fcp7Xml));
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, Some("/proj")).unwrap();
    let p = &imp.project;
    assert_eq!(p.name, "Lighthouse");
    assert_eq!(imp.sequences.len(), 2);
    let main = imp.sequences.iter().copied().find(|s| p.item(*s).unwrap().name == "Main Edit").unwrap();
    let nest = imp.sequences.iter().copied().find(|s| p.item(*s).unwrap().name == "Titles Nest").unwrap();
    let s = p.sequence(main).unwrap();
    let r = FrameRate::FPS_23_976;
    assert_eq!(s.settings.frame_rate, r);
    assert_eq!(s.start_timecode, 86_400);

    assert_master_clip(p, r);
    assert_main_video_and_effects(p, s, r, &rep);
    assert_nested_and_audio(p, s, r, nest);
}

fn assert_master_clip(p: &Project, r: FrameRate) {
    // Master clip in the Footage bin, with its file characteristics.
    let tower = p.items.values().find(|i| i.name == "tower.mov").unwrap();
    let tm = tower.as_media().unwrap();
    assert_eq!(tm.media, MediaRef::File { path: "/Volumes/Shoot/tower wide.mov".into() });
    assert_eq!(tm.info.video.as_ref().unwrap().width, 3840);
    assert_eq!(tm.info.start_timecode, Some(86_400));
    assert_eq!(tm.info.duration, r.tick_of(2400));
    assert_eq!(tower.label, Label::Mango);
    let bin = p.root.children.iter().find_map(|c| if let filmcraft_project::BinEntry::Bin(b) = c { Some(b) } else { None }).unwrap();
    assert_eq!(bin.name, "Footage");
    assert_eq!(p.root.parent_of(tower.id), Some(bin.id));
}

fn assert_main_video_and_effects(p: &Project, s: &Sequence, r: FrameRate, rep: &filmcraft_interchange::Report) {
    // V1: -1 start/end resolved from in/out, centred dissolve.
    let v1 = &s.video_tracks[0];
    assert_eq!(v1.items.len(), 2);
    let (c1, c2) = (&v1.items[0], &v1.items[1]);
    assert_eq!((c1.start, c1.duration), (r.tick_of(0), r.tick_of(48)));
    assert_eq!((c2.start, c2.duration, c2.source_in), (r.tick_of(48), r.tick_of(72), r.tick_of(100)));
    assert_eq!(c1.label, Label::Cerulean);
    assert_eq!(media_key(p, c2.item), "/proj/media/waves.mov");
    let t = &v1.transitions[0];
    assert_eq!((t.start, t.duration, t.from, t.to, t.align), (r.tick_of(36), r.tick_of(24), Some(c1.id), Some(c2.id), TransitionAlign::CenterAtCut));
    assert_eq!(t.effect.effect, "cross_dissolve");
    // Motion / opacity / generic effect by name.
    let scale = c2.effect("motion").unwrap().param("scale").unwrap();
    assert_eq!(scale.keyframes.len(), 2);
    assert_eq!(scale.keyframes[1].time, r.tick_of(160));
    assert_eq!(scale.keyframes[1].value, ParamValue::Float(150.0));
    assert_eq!(c2.effect("motion").unwrap().param("rotation").unwrap().value, ParamValue::Float(-12.5));
    let pos = c2.effect("motion").unwrap().param("position").unwrap().value.as_vec2().unwrap();
    assert!((pos.x - (960.0 + 480.0)).abs() < 1e-9 && (pos.y - (540.0 - 108.0)).abs() < 1e-9, "{pos:?}");
    assert_eq!(c2.effect("opacity").unwrap().param("opacity").unwrap().value, ParamValue::Float(50.0));
    assert!(c2.effect("gaussian_blur").is_some());
    assert!(rep.mentions("Mystery Glow Pro"), "{rep}");
    assert_eq!(c2.markers.len(), 1);
    assert_eq!((c2.markers[0].name.as_str(), c2.markers[0].start), ("wave peak", r.tick_of(130)));
}

fn assert_nested_and_audio(p: &Project, s: &Sequence, r: FrameRate, nest: filmcraft_project::ItemId) {
    // V2: nested sequence clip, colour matte generator, locked track.
    let v2 = &s.video_tracks[1];
    assert!(v2.locked);
    assert_eq!(v2.items[0].item, nest);
    assert!(matches!(p.item(v2.items[0].item).unwrap().kind, ItemKind::Sequence(_)));
    let g = p.item(v2.items[1].item).unwrap().as_media().unwrap();
    assert_eq!(g.media, MediaRef::Generator(Generator::ColorMatte { color: [1.0, 0.0, 0.0, 1.0] }));
    let ns = p.sequence(nest).unwrap();
    assert_eq!(ns.video_tracks[0].items[0].speed, 2.0);
    assert_eq!(ns.video_tracks[0].items[0].item, s.video_tracks[0].items[1].item, "file-2 reference shared across sequences");

    // Audio: levels as dB with keyframes, link to the video clip, disabled music.
    let a1 = &s.audio_tracks[0].items[0];
    let lvl = a1.effect("volume").unwrap().param("level").unwrap();
    assert!((lvl.keyframes[1].value.as_f64().unwrap() + 6.0206).abs() < 1e-3);
    assert_eq!(lvl.keyframes[0].value, ParamValue::Float(0.0));
    assert!(a1.link.is_some());
    assert_eq!(a1.link, s.video_tracks[0].items[0].link);
    let music = &s.audio_tracks[1].items[0];
    assert!(!music.enabled);
    assert!(p.item(music.item).unwrap().as_media().unwrap().info.video.is_none());
    assert_eq!(s.markers[0].name, "Check sky");
    assert_eq!(s.markers[0].start, r.tick_of(30));
}

/// Both fades resolve the sentinel bounds of an imported audio clip.
#[test]
fn transition_bounded_audio_clip_keeps_both_transitions() {
    let doc = transition_bounded_audio_doc();
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let s = imp.project.sequence(imp.sequences[0]).unwrap();
    let track = &s.audio_tracks[0];
    let r = FrameRate::FPS_25;
    assert_eq!(track.items.len(), 1);
    assert_eq!(track.items[0].start, r.tick_of(698));
    assert_eq!(track.items[0].duration, r.tick_of(179));
    assert_eq!(track.transitions.len(), 2);
    assert!(!rep.mentions("neither start nor end"), "{rep}");
}

#[test]
fn audio_clip_without_bounds_or_transitions_is_still_skipped() {
    let doc = r#"<xmeml version="4"><sequence id="sequence-1">
      <name>Unbounded audio</name><rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
      <media><audio><track>
        <clipitem id="clipitem-1"><name>music.wav</name>
          <start>-1</start><end>-1</end><in>5378</in><out>5557</out>
          <file id="file-1"><name>music.wav</name><pathurl>file:///audio/music.wav</pathurl></file>
        </clipitem>
      </track></audio></media>
    </sequence></xmeml>"#;
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let s = imp.project.sequence(imp.sequences[0]).unwrap();
    assert!(s.audio_tracks[0].items.is_empty());
    assert!(rep.mentions("clip \"music.wav\" has neither start nor end; skipped"), "{rep}");
}

fn rich_project() -> (Project, filmcraft_project::ItemId) {
    let r = FrameRate::FPS_29_97;
    let mut p = Project::new("P");
    let a = media(&mut p, "/m/a b.mov", true, true, r);
    let b = media(&mut p, "/m/b.mov", true, true, r);
    let m = media(&mut p, "/m/music.wav", false, true, r);
    let nest = sequence(&mut p, "Nested", r, true);
    clip(&mut p, nest, TrackKind::Video, 0, b, 0..90, 30);
    let s = sequence(&mut p, "Main", r, true);
    p.sequence_mut(s).unwrap().start_timecode = 107_892;
    let v1 = clip(&mut p, s, TrackKind::Video, 0, a, 0..60, 10);
    let a1 = clip(&mut p, s, TrackKind::Audio, 0, a, 0..60, 10);
    link(&mut p, s, &[v1, a1]);
    let v2 = clip(&mut p, s, TrackKind::Video, 0, b, 60..60 + (45), 300);
    transition(&mut p, s, (TrackKind::Video, 0), "dip_to_black", (Some(v1), Some(v2)), 50..50 + (20), TransitionAlign::CenterAtCut);
    transition(&mut p, s, (TrackKind::Video, 0), "cross_dissolve", (Some(v2), None), 90..90 + (15), TransitionAlign::EndAtCut);
    let n = clip(&mut p, s, TrackKind::Video, 1, nest, 20..20 + (40), 0);
    let mm = clip(&mut p, s, TrackKind::Audio, 1, m, 0..105, 0);
    transition(&mut p, s, (TrackKind::Audio, 1), "constant_power", (None, Some(mm)), 0..10, TransitionAlign::StartAtCut);
    {
        let q = p.sequence_mut(s).unwrap();
        let (_, it) = q.find_item_mut(v2).unwrap();
        it.speed = 1.5;
        it.label = Label::Rose;
        let mut sc = Param::new(ParamValue::Float(100.0));
        sc.keyframes.push(Keyframe::new(r.tick_of(300), ParamValue::Float(100.0)));
        sc.keyframes.push(Keyframe::new(r.tick_of(330), ParamValue::Float(80.0)));
        it.effect_mut("motion").unwrap().params.insert("scale".into(), sc);
        it.effect_mut("opacity").unwrap().params.insert("opacity".into(), Param::new(ParamValue::Float(75.0)));
        it.effects.push(filmcraft_project::find_effect("gaussian_blur").unwrap().instance());
        it.markers.push(Marker {
            id: MarkerId(900),
            start: r.tick_of(310),
            duration: r.tick_of(5),
            name: "hit".into(),
            comment: "c".into(),
            kind: MarkerKind::Comment,
            color: Label::Yellow,
        });
        let (_, ni) = q.find_item_mut(n).unwrap();
        ni.enabled = false;
        ni.reverse = true;
        let (_, ai) = q.find_item_mut(a1).unwrap();
        ai.effect_mut("volume").unwrap().params.insert("level".into(), Param::new(ParamValue::Float(-6.0)));
        q.markers.push(Marker {
            id: MarkerId(901),
            start: r.tick_of(12),
            duration: Tick::ZERO,
            name: "seq m".into(),
            comment: String::new(),
            kind: MarkerKind::Comment,
            color: Label::Blue,
        });
        q.video_tracks[1].locked = true;
    }
    (p, s)
}

#[test]
fn rich_roundtrip() {
    let (p, s) = rich_project();
    let (imp, text, rep) = roundtrip(&p, s, Format::Fcp7Xml, &ExportOptions::default());
    assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"4\">"));
    assert!(text.contains("<pathurl>file://localhost/m/a%20b.mov</pathurl>"), "{text}");
    assert!(!rep.has_warnings(), "{rep}");
    let si = only_seq(&imp);
    let q = &imp.project;
    for kind in [TrackKind::Video, TrackKind::Audio] {
        assert_eq!(structure(q, si, kind), structure(&p, s, kind), "{text}");
    }
    assert_eq!(links(q, si), links(&p, s));
    let qs = q.sequence(si).unwrap();
    let ps = p.sequence(s).unwrap();
    assert_eq!(qs.start_timecode, ps.start_timecode);
    assert!(qs.settings.drop_frame);
    assert_eq!(
        qs.markers.iter().map(|m| (&m.name, m.start, m.color)).collect::<Vec<_>>(),
        ps.markers.iter().map(|m| (&m.name, m.start, m.color)).collect::<Vec<_>>()
    );
    assert!(qs.video_tracks[1].locked);
    let v2 = &qs.video_tracks[0].items[1];
    let pv2 = &ps.video_tracks[0].items[1];
    assert_eq!(v2.label, Label::Rose);
    assert_eq!(v2.effect("motion").unwrap().param("scale"), pv2.effect("motion").unwrap().param("scale"));
    assert_eq!(v2.effect("opacity").unwrap().param("opacity").unwrap().value, ParamValue::Float(75.0));
    assert!(v2.effect("gaussian_blur").is_some());
    assert_eq!(
        v2.markers.iter().map(|m| (&m.name, m.start, m.duration)).collect::<Vec<_>>(),
        pv2.markers.iter().map(|m| (&m.name, m.start, m.duration)).collect::<Vec<_>>()
    );
    let lvl = qs.audio_tracks[0].items[0].effect("volume").unwrap().param("level").unwrap().value.as_f64().unwrap();
    assert!((lvl + 6.0).abs() < 1e-9);
    // nested sequence survived as a sequence item with its own clip
    let nested = qs.video_tracks[1].items[0].item;
    assert_eq!(q.item(nested).unwrap().name, "Nested");
    assert_eq!(q.sequence(nested).unwrap().video_tracks[0].items[0].source_in, FrameRate::FPS_29_97.tick_of(30));
    // Re-export is stable.
    let (again, _) = export(q, si, Format::Fcp7Xml, &ExportOptions::default()).unwrap();
    let (imp2, _) = import(&again, Format::Fcp7Xml, None).unwrap();
    assert_eq!(structure(&imp2.project, imp2.sequences[0], TrackKind::Video), structure(&p, s, TrackKind::Video));
}

#[test]
fn version_5_and_generators() {
    let (mut p, s) = rich_project();
    let g = generator(&mut p, "Bars", Generator::BarsAndTone);
    let k = generator(&mut p, "Black", Generator::BlackVideo);
    clip(&mut p, s, TrackKind::Video, 2, g, 0..30, 0);
    clip(&mut p, s, TrackKind::Video, 2, k, 30..30 + (30), 0);
    let o = ExportOptions { xmeml_version: 5, ..Default::default() };
    let (imp, text, _) = roundtrip(&p, s, Format::Fcp7Xml, &o);
    assert!(text.contains("<xmeml version=\"5\">"));
    assert!(text.contains("<effectid>slug</effectid>"));
    let si = only_seq(&imp);
    assert_eq!(structure(&imp.project, si, TrackKind::Video), structure(&p, s, TrackKind::Video));
}

// ---------------------------------------------------------------------------------------------
// Property round trip over multi-track timelines with transitions, speed, links, enable flags.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct G {
    track: usize,
    gap: i64,
    dur: i64,
    media: usize,
    src: i64,
    speed: u8,
    enabled: bool,
    tr: u8,
    tr_len: i64,
    audio: bool,
}

fn gen_clip() -> impl Strategy<Value = G> {
    (0usize..3, 0i64..3, 20i64..150, 0usize..3, 0i64..2000, 0u8..5, prop::bool::weighted(0.9), 0u8..6, 1i64..9, any::<bool>()).prop_map(
        |(track, gap, dur, media, src, speed, enabled, tr, tr_len, audio)| G { track, gap: gap * 5, dur, media, src, speed, enabled, tr, tr_len, audio },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn xmeml_roundtrip(clips in prop::collection::vec(gen_clip(), 1..20), ri in 0usize..5) {
        let rate = [FrameRate::FPS_23_976, FrameRate::FPS_25, FrameRate::FPS_29_97, FrameRate::FPS_59_94, FrameRate::FPS_24][ri];
        let mut p = Project::new("P");
        let ms = [media(&mut p, "/x/one.mov", true, true, rate), media(&mut p, "/x/two.mov", true, true, rate), media(&mut p, "/y/three four.mxf", true, true, rate)];
        let s = sequence(&mut p, "S", rate, false);
        let mut ends = [0i64; 3];
        let mut prev: [Option<filmcraft_project::ClipId>; 3] = [None; 3];
        let mut aends = [0i64; 3];
        for g in &clips {
            let t = ends[g.track] + g.gap;
            let v = clip(&mut p, s, TrackKind::Video, g.track, ms[g.media], t..t + (g.dur), g.src);
            {
                let q = p.sequence_mut(s).unwrap();
                let (_, it) = q.find_item_mut(v).unwrap();
                it.enabled = g.enabled;
                match g.speed {
                    1 => it.speed = 2.0,
                    2 => it.speed = 0.5,
                    3 => it.reverse = true,
                    _ => {}
                }
            }
            let d = g.tr_len;
            let adjacent = g.gap == 0 && prev[g.track].is_some();
            match g.tr {
                0 if adjacent => transition(&mut p, s, (TrackKind::Video, g.track), "cross_dissolve", (prev[g.track], Some(v)), t - d / 2..t - d / 2 + (d), TransitionAlign::CenterAtCut),
                1 if adjacent => transition(&mut p, s, (TrackKind::Video, g.track), "dip_to_white", (prev[g.track], Some(v)), t..t + (d), TransitionAlign::StartAtCut),
                2 if adjacent => transition(&mut p, s, (TrackKind::Video, g.track), "push", (prev[g.track], Some(v)), t - d..t - d + (d), TransitionAlign::EndAtCut),
                3 if !adjacent => transition(&mut p, s, (TrackKind::Video, g.track), "cross_dissolve", (None, Some(v)), t..t + (d), TransitionAlign::StartAtCut),
                _ => {}
            }
            if g.audio && aends[g.track] <= t {
                let a = clip(&mut p, s, TrackKind::Audio, g.track, ms[g.media], t..t + (g.dur), g.src);
                link(&mut p, s, &[v, a]);
                aends[g.track] = t + g.dur;
            }
            ends[g.track] = t + g.dur;
            prev[g.track] = Some(v);
        }
        let (imp, text, _) = roundtrip(&p, s, Format::Fcp7Xml, &ExportOptions::default());
        let si = only_seq(&imp);
        prop_assert_eq!(structure(&imp.project, si, TrackKind::Video), structure(&p, s, TrackKind::Video), "{}", text);
        prop_assert_eq!(structure(&imp.project, si, TrackKind::Audio), structure(&p, s, TrackKind::Audio));
        prop_assert_eq!(links(&imp.project, si), links(&p, s));
    }
}

#[test]
fn a_master_clip_used_by_an_earlier_sequence_still_lands_in_its_bin() {
    // Premiere and other writers may put a stringout sequence ahead of the bins that hold its clips.
    let file = format!(
        r#"<file id="file-1"><name>take.mov</name><pathurl>file://localhost/media/take.mov</pathurl>{RATE}<duration>120</duration><media><video><samplecharacteristics><width>1920</width><height>1080</height></samplecharacteristics></video></media></file>"#
    );
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="4"><project><name>Order</name><children>
  <bin><name>Stringouts</name><children>
    <sequence id="sequence-1"><name>All takes</name><duration>120</duration>{RATE}
      <media><video><track><clipitem id="clipitem-1"><masterclipid>masterclip-1</masterclipid><name>take</name>{RATE}
        <start>0</start><end>120</end><in>0</in><out>120</out>{file}</clipitem></track></video></media>
    </sequence>
  </children></bin>
  <bin><name>Footage</name><children><bin><name>Comedy</name><children>
    <clip id="masterclip-1"><name>take</name><duration>120</duration>{RATE}
      <media><video><track><clipitem id="clipitem-2"><masterclipid>masterclip-1</masterclipid><name>take</name>{RATE}<file id="file-1"/></clipitem></track></video></media>
    </clip>
  </children></bin></children></bin>
</children></project></xmeml>"#
    );
    let (imp, _) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let p = &imp.project;
    let mut items = Vec::new();
    p.root.all_items(&mut items);
    let take = items.iter().copied().find(|i| p.item(*i).unwrap().name == "take.mov").unwrap();
    assert_eq!(items.iter().filter(|i| **i == take).count(), 1);
    let bin = p.root.parent_of(take).unwrap();
    assert_ne!(bin, p.root.id, "the take was left loose at the top");
    assert_eq!(p.root.find_bin(bin).unwrap().name, "Comedy");
}

/// `start`/`end`/`in`/`out` of every sequence clip item in an exported document.
fn clip_spans(xml: &str) -> Vec<(i64, i64, i64, i64)> {
    let tag = |block: &str, name: &str| -> i64 {
        let open = format!("<{name}>");
        let at = block.find(&open).unwrap_or_else(|| panic!("no <{name}> in {block}")) + open.len();
        block[at..].split('<').next().unwrap().trim().parse().unwrap()
    };
    xml.split("<clipitem").skip(1).filter(|b| b.contains("<start>")).map(|b| (tag(b, "start"), tag(b, "end"), tag(b, "in"), tag(b, "out"))).collect()
}

/// #340: a clip placed by seconds in a 23.976 sequence has sub-frame start, duration and source in.
/// Its record span and source span were rounded to frames independently, so `out - in` could come
/// out a frame shorter than `end - start` (Premiere then reads a different source range).
#[test]
fn sub_frame_clips_keep_equal_source_and_record_spans() {
    const SECOND: i64 = 254_016_000_000;
    let r = FrameRate::FPS_23_976;
    let mut p = Project::new("P");
    let a = media(&mut p, "/m/a.mov", true, true, r);
    let s = sequence(&mut p, "t", r, false);
    let c = clip(&mut p, s, TrackKind::Video, 0, a, 0..24, 0);
    // the issue's case first (start 0, source in 20 s, 3 s long), then starts, ins and lengths in tenths of a second
    let mut cases = vec![(0, 200, 30)];
    for start in [0, 1, 5, 17, 33] {
        for src in [0, 3, 7, 200, 413] {
            for dur in [1, 4, 30, 71, 125] {
                cases.push((start, src, dur));
            }
        }
    }
    for (start, src, dur) in cases {
        let (_, it) = p.sequence_mut(s).unwrap().find_item_mut(c).unwrap();
        it.start = Tick(start * SECOND / 10);
        it.source_in = Tick(src * SECOND / 10);
        it.duration = Tick(dur * SECOND / 10);
        let (bytes, _) = export(&p, s, Format::Fcp7Xml, &ExportOptions::default()).expect("export");
        let xml = String::from_utf8(bytes).unwrap();
        let spans = clip_spans(&xml);
        assert_eq!(spans.len(), 1, "{xml}");
        let (start_f, end_f, in_f, out_f) = spans[0];
        assert_eq!(out_f - in_f, end_f - start_f, "start {start}, in {src}, duration {dur} (tenths of a second): {spans:?}");
    }
}

/// #463: a stereo file split onto two mono tracks (A1 = channel 1, A2 = channel 2) lost the right channel,
/// because every audio clip item was written with `sourcetrack/trackindex` 1 and the importer ignored it.
#[test]
fn split_mono_tracks_keep_their_source_channel() {
    let r = FrameRate::FPS_23_976;
    let mut p = Project::new("P");
    let a = media(&mut p, "/m/a.wav", false, true, r);
    let s = sequence(&mut p, "t", r, false);
    let left = clip(&mut p, s, TrackKind::Audio, 0, a, 0..24, 0);
    let right = clip(&mut p, s, TrackKind::Audio, 1, a, 0..24, 0);
    for (id, ch) in [(left, 0u16), (right, 1u16)] {
        let (_, it) = p.sequence_mut(s).unwrap().find_item_mut(id).unwrap();
        it.source_channels = vec![ch];
    }
    let (imp, xml, _) = roundtrip(&p, s, Format::Fcp7Xml, &ExportOptions::default());
    assert!(xml.contains("<trackindex>2</trackindex>"), "{xml}");
    let seq = imp.project.sequence(only_seq(&imp)).unwrap();
    let channels = |t: usize| seq.tracks(TrackKind::Audio)[t].items.iter().map(|i| i.source_channels.clone()).collect::<Vec<_>>();
    assert_eq!(channels(0), vec![vec![0u16]], "{xml}");
    assert_eq!(channels(1), vec![vec![1u16]], "{xml}");
    let (bytes, _) = export(&imp.project, only_seq(&imp), Format::Fcp7Xml, &ExportOptions::default()).expect("export");
    assert!(String::from_utf8(bytes).unwrap().contains("<trackindex>2</trackindex>"));
}

#[test]
fn audio_sourcetrack_trackindex_2_imports_as_source_channel_1() {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="4"><sequence id="sequence-1"><name>S</name><duration>48</duration>{RATE}
  <media><audio><track><clipitem id="clipitem-1"><name>take</name>{RATE}<start>0</start><end>48</end><in>0</in><out>48</out>
    <file id="file-1"><name>take.wav</name><pathurl>file://localhost/media/take.wav</pathurl>{RATE}<duration>48</duration><media><audio><channelcount>2</channelcount></audio></media></file>
    <sourcetrack><mediatype>audio</mediatype><trackindex>2</trackindex></sourcetrack>
  </clipitem></track></audio></media>
</sequence></xmeml>"#
    );
    let (imp, _) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let seq = imp.project.sequence(only_seq(&imp)).unwrap();
    assert_eq!(seq.tracks(TrackKind::Audio)[0].items[0].source_channels, vec![1u16]);
}

/// #463: Premiere writes a stereo file split onto two mono tracks as clip items with `trackindex` 1 and 2;
/// a file used only with `trackindex` 1 is a stereo clip and keeps the default (all channels) mapping.
#[test]
fn audio_trackindex_1_is_channel_0_only_when_the_file_is_split() {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="4"><sequence id="sequence-1"><name>S</name><duration>48</duration>{RATE}
  <media><audio>
  <track><clipitem id="clipitem-1"><name>take</name>{RATE}<start>0</start><end>48</end><in>0</in><out>48</out>
    <file id="file-1"><name>take.wav</name><pathurl>file://localhost/media/take.wav</pathurl>{RATE}<duration>48</duration><media><audio><channelcount>2</channelcount></audio></media></file>
    <sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack>
  </clipitem>
  <clipitem id="clipitem-3"><name>other</name>{RATE}<start>48</start><end>96</end><in>0</in><out>48</out>
    <file id="file-2"><name>other.wav</name><pathurl>file://localhost/media/other.wav</pathurl>{RATE}<duration>48</duration><media><audio><channelcount>2</channelcount></audio></media></file>
    <sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack>
  </clipitem></track>
  <track><clipitem id="clipitem-2"><name>take</name>{RATE}<start>0</start><end>48</end><in>0</in><out>48</out>
    <file id="file-1"/>
    <sourcetrack><mediatype>audio</mediatype><trackindex>2</trackindex></sourcetrack>
  </clipitem></track>
  </audio></media>
</sequence></xmeml>"#
    );
    let (imp, _) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let seq = imp.project.sequence(only_seq(&imp)).unwrap();
    let a1 = &seq.tracks(TrackKind::Audio)[0].items;
    assert_eq!(a1[0].source_channels, vec![0u16]);
    assert!(a1[1].source_channels.is_empty(), "{:?}", a1[1].source_channels);
    assert_eq!(seq.tracks(TrackKind::Audio)[1].items[0].source_channels, vec![1u16]);
}

/// A Premiere-style stereo file split onto two mono tracks (A1 = `trackindex` 1, A2 = 2).
fn split_stereo_doc(pan_a1: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="4"><sequence id="sequence-1"><name>S</name><duration>48</duration>{RATE}
  <media><audio>
  <track currentExplodedTrackIndex="0" totalExplodedTrackCount="2" premiereTrackType="Stereo"><clipitem id="clipitem-1"><name>sfx</name>{RATE}<start>0</start><end>48</end><in>0</in><out>48</out>
    <file id="file-1"><name>sfx.wav</name><pathurl>file://localhost/media/sfx.wav</pathurl>{RATE}<duration>48</duration><media><audio><channelcount>2</channelcount></audio></media></file>
    <sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack>{pan_a1}
  </clipitem><outputchannelindex>1</outputchannelindex></track>
  <track currentExplodedTrackIndex="1" totalExplodedTrackCount="2" premiereTrackType="Stereo"><clipitem id="clipitem-2"><name>sfx</name>{RATE}<start>0</start><end>48</end><in>0</in><out>48</out>
    <file id="file-1"/>
    <sourcetrack><mediatype>audio</mediatype><trackindex>2</trackindex></sourcetrack>
  </clipitem><outputchannelindex>2</outputchannelindex></track>
  </audio></media>
</sequence></xmeml>"#
    )
}

fn balances(imp: &Imported) -> Vec<f64> {
    let seq = imp.project.sequence(only_seq(imp)).unwrap();
    seq.tracks(TrackKind::Audio)
        .iter()
        .flat_map(|t| &t.items)
        .map(|i| i.effect("panner").and_then(|e| e.param("balance")).and_then(|p| p.value.as_f64()).unwrap_or(0.0))
        .collect()
}

/// #371: the halves of a split stereo file played centred, so both channels summed on both sides
/// (+3 dB for a stereo effect, +6 dB for dual-mono dialogue). They now play on their own side.
#[test]
fn split_stereo_halves_are_panned_to_their_side() {
    let (imp, _) = import(split_stereo_doc("").as_bytes(), Format::Fcp7Xml, None).unwrap();
    assert_eq!(balances(&imp), vec![-100.0, 100.0]);
}

/// An Audio Pan filter on a split half still wins.
#[test]
fn split_stereo_half_keeps_its_audio_pan() {
    let pan = r#"<filter><effect><name>Audio Pan</name><effectid>audiopan</effectid><effectcategory>audiopan</effectcategory><effecttype>audiopan</effecttype><mediatype>audio</mediatype>
      <parameter><parameterid>pan</parameterid><name>Pan</name><valuemin>-1</valuemin><valuemax>1</valuemax><value>0</value></parameter></effect></filter>"#;
    let (imp, _) = import(split_stereo_doc(pan).as_bytes(), Format::Fcp7Xml, None).unwrap();
    assert_eq!(balances(&imp), vec![0.0, 100.0]);
}

/// A centred one-channel clip (Breakout to Mono) round-trips centred: export writes its Audio Pan.
#[test]
fn centred_mono_breakout_round_trips_centred() {
    let r = FrameRate::FPS_23_976;
    let mut p = Project::new("P");
    let a = media(&mut p, "/m/a.wav", false, true, r);
    let s = sequence(&mut p, "t", r, false);
    let left = clip(&mut p, s, TrackKind::Audio, 0, a, 0..24, 0);
    let right = clip(&mut p, s, TrackKind::Audio, 1, a, 0..24, 0);
    for (id, ch) in [(left, 0u16), (right, 1u16)] {
        let (_, it) = p.sequence_mut(s).unwrap().find_item_mut(id).unwrap();
        it.source_channels = vec![ch];
    }
    let (imp, xml, _) = roundtrip(&p, s, Format::Fcp7Xml, &ExportOptions::default());
    assert_eq!(balances(&imp), vec![0.0, 0.0], "{xml}");
}

/// The two-fade audio shape used by the importer and exporter regressions.
fn transition_bounded_audio_doc() -> &'static str {
    r#"<xmeml version="4"><sequence id="sequence-1">
      <name>Faded audio</name><duration>877</duration>
      <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
      <media><audio><track>
        <transitionitem><start>698</start><end>760</end><alignment>start-black</alignment>
          <effect><name>Cross Fade (+3dB)</name><effectid>Cross Fade (+3dB)</effectid><mediatype>audio</mediatype></effect>
        </transitionitem>
        <clipitem id="clipitem-1"><name>music.wav</name>
          <start>-1</start><end>-1</end><in>5378</in><out>5557</out>
          <file id="file-1"><name>music.wav</name><pathurl>file:///audio/music.wav</pathurl></file>
        </clipitem>
        <transitionitem><start>820</start><end>877</end><alignment>end-black</alignment>
          <effect><name>Cross Fade (+3dB)</name><effectid>Cross Fade (+3dB)</effectid><mediatype>audio</mediatype></effect>
        </transitionitem>
      </track></audio></media>
    </sequence></xmeml>"#
}

/// Export writes sentinel bounds without changing source spans or either fade.
#[test]
fn fcp7_export_transition_bounded_audio() {
    let r = FrameRate::FPS_25;
    let mut p = Project::new("P");
    let a = media(&mut p, "/audio/music.wav", false, true, r);
    let s = sequence(&mut p, "Faded audio", r, false);
    let c = clip(&mut p, s, TrackKind::Audio, 0, a, 698..698+179, 5378);
    transition(&mut p, s, (TrackKind::Audio, 0), "constant_power", (None, Some(c)), 698..698+62, TransitionAlign::StartAtCut);
    transition(&mut p, s, (TrackKind::Audio, 0), "constant_power", (Some(c), None), 820..820+57, TransitionAlign::EndAtCut);
    let (bytes, _) = export(&p, s, Format::Fcp7Xml, &ExportOptions::default()).unwrap();
    let xml = String::from_utf8(bytes).unwrap();
    assert_eq!(clip_spans(&xml), vec![(-1, -1, 5378, 5557)], "{xml}");
    let fades: Vec<_> = xml.split("<transitionitem>").skip(1).map(|b| b.split("</transitionitem>").next().unwrap()).collect();
    assert_eq!(fades.len(), 2, "{xml}");
    for (fade, start, end) in [(fades[0], 698, 760), (fades[1], 820, 877)] {
        assert!(fade.contains(&format!("<start>{start}</start>")), "{fade}");
        assert!(fade.contains(&format!("<end>{end}</end>")), "{fade}");
    }
}

/// Sentinel bounds survive import, export and a second import with both fades.
#[test]
fn fcp7_transition_bounded_audio_roundtrip() {
    let (imp, _) = import(transition_bounded_audio_doc().as_bytes(), Format::Fcp7Xml, None).unwrap();
    let (again, xml, _) = roundtrip(&imp.project, only_seq(&imp), Format::Fcp7Xml, &ExportOptions::default());
    assert_eq!(clip_spans(&xml), vec![(-1, -1, 5378, 5557)], "{xml}");
    let seq = again.project.sequence(only_seq(&again)).unwrap();
    let track = &seq.audio_tracks[0];
    let r = FrameRate::FPS_25;
    assert_eq!(track.items.len(), 1);
    assert_eq!((track.items[0].start, track.items[0].duration), (r.tick_of(698), r.tick_of(179)));
    assert_eq!(track.transitions.len(), 2);
}

/// A clip without transitions retains numeric record bounds.
#[test]
fn fcp7_export_unbounded_audio_keeps_numeric_bounds() {
    let r = FrameRate::FPS_25;
    let mut p = Project::new("P");
    let a = media(&mut p, "/audio/music.wav", false, true, r);
    let s = sequence(&mut p, "S", r, false);
    clip(&mut p, s, TrackKind::Audio, 0, a, 698..698+179, 5378);
    let (_, xml, _) = roundtrip(&p, s, Format::Fcp7Xml, &ExportOptions::default());
    assert_eq!(clip_spans(&xml), vec![(698, 877, 5378, 5557)], "{xml}");
}

/// A start fade alone retains numeric bounds at both edges.
#[test]
fn fcp7_export_start_fade_keeps_numeric_bounds() {
    let r = FrameRate::FPS_25;
    let mut p = Project::new("P");
    let a = media(&mut p, "/audio/music.wav", false, true, r);
    let s = sequence(&mut p, "S", r, false);
    let c = clip(&mut p, s, TrackKind::Audio, 0, a, 698..698+179, 5378);
    transition(&mut p, s, (TrackKind::Audio, 0), "constant_power", (None, Some(c)), 698..698+62, TransitionAlign::StartAtCut);
    let (_, xml, _) = roundtrip(&p, s, Format::Fcp7Xml, &ExportOptions::default());
    assert_eq!(clip_spans(&xml), vec![(698, 877, 5378, 5557)], "{xml}");
}

/// Premiere's own name and effect id for the audio crossfade are written, so it does not warn that the transition was
/// not translated. Constant Gain (no Premiere id known) and video transitions keep their display names.
#[test]
fn fcp7_export_audio_constant_power_uses_premiere_id() {
    let r = FrameRate::FPS_25;
    let mut p = Project::new("P");
    let a = media(&mut p, "/audio/music.wav", false, true, r);
    let v = media(&mut p, "/video/shot.mov", true, false, r);
    let s = sequence(&mut p, "S", r, false);
    let c = clip(&mut p, s, TrackKind::Audio, 0, a, 698..698+179, 5378);
    transition(&mut p, s, (TrackKind::Audio, 0), "constant_power", (None, Some(c)), 698..698+62, TransitionAlign::StartAtCut);
    let g = clip(&mut p, s, TrackKind::Audio, 1, a, 100..100+100, 0);
    transition(&mut p, s, (TrackKind::Audio, 1), "constant_gain", (None, Some(g)), 100..100+20, TransitionAlign::StartAtCut);
    let vc = clip(&mut p, s, TrackKind::Video, 0, v, 0..0+100, 0);
    transition(&mut p, s, (TrackKind::Video, 0), "cross_dissolve", (None, Some(vc)), 0..0+20, TransitionAlign::StartAtCut);
    let (bytes, _) = export(&p, s, Format::Fcp7Xml, &ExportOptions::default()).unwrap();
    let xml = String::from_utf8(bytes).unwrap();
    assert!(xml.contains("<name>Cross Fade (+3dB)</name>"), "{xml}");
    assert!(xml.contains("<effectid>KGAudioTransCrossFade3dB</effectid>"), "{xml}");
    assert!(!xml.contains("<name>Constant Power</name>"), "{xml}");
    assert!(xml.contains("<name>Constant Gain</name>") && xml.contains("<effectid>Constant Gain</effectid>"), "{xml}");
    assert!(xml.contains("<name>Cross Dissolve</name>") && xml.contains("<effectid>Cross Dissolve</effectid>"), "{xml}");
}

/// A Premiere audio crossfade survives import, export and re-import as Constant Power.
#[test]
fn fcp7_audio_crossfade_id_roundtrip_keeps_type() {
    let (imp, _) = import(transition_bounded_audio_doc().as_bytes(), Format::Fcp7Xml, None).unwrap();
    let (again, xml, _) = roundtrip(&imp.project, only_seq(&imp), Format::Fcp7Xml, &ExportOptions::default());
    assert_eq!(xml.matches("<effectid>KGAudioTransCrossFade3dB</effectid>").count(), 2, "{xml}");
    let seq = again.project.sequence(only_seq(&again)).unwrap();
    let kinds: Vec<_> = seq.audio_tracks[0].transitions.iter().map(|t| t.effect.effect.as_str()).collect();
    assert_eq!(kinds, vec!["constant_power", "constant_power"]);
}


fn slider_doc(kind: &str, filters: &[String]) -> String {
    let clips: String = filters.iter().enumerate().map(|(i, filter)| format!(r#"
        <clipitem id="clip-{i}"><name>sliders</name><rate><timebase>25</timebase></rate>
        <start>{}</start><end>{}</end><in>6145</in><out>6170</out>
        <file id="file-{i}"><name>sliders.mov</name><pathurl>file:///sliders.mov</pathurl>
        <duration>7000</duration><rate><timebase>25</timebase></rate><media>
        <video><samplecharacteristics><width>1920</width><height>1080</height></samplecharacteristics></video>
        <audio><channelcount>1</channelcount><samplecharacteristics><samplerate>48000</samplerate></samplecharacteristics></audio>
        </media></file>{filter}</clipitem>"#, i * 25, (i + 1) * 25)).collect();
    format!(r#"<xmeml version="4"><sequence><name>Sliders</name><duration>{}</duration>
        <rate><timebase>25</timebase></rate><media><{kind}><track>{clips}</track></{kind}></media>
        </sequence></xmeml>"#, filters.len() * 25)
}

fn slider_parameter(id: &str, value: &str) -> String {
    format!("<parameter><parameterid>{id}</parameterid><value>{value}</value></parameter>")
}

fn lumetri_filter(params: &str) -> String {
    format!("<filter><effect><name /><effectid>Lumetri</effectid><pproBypass>false</pproBypass>{params}</effect></filter>")
}

fn gain_filter(value: &str, enabled: bool) -> String {
    format!("<filter><enabled>{enabled}</enabled><effect><name>Gain</name><effectid>{{61756678, 4761696e, 4b657947}}</effectid>{}</effect></filter>", slider_parameter("Gain(dB)", value))
}

fn audio_levels(animated: bool) -> String {
    let param = if animated {
        "<parameter><parameterid>level</parameterid><value>3.28024</value><keyframe><when>6148</when><value>2.00668</value></keyframe><keyframe><when>6172</when><value>3.28024</value></keyframe></parameter>".to_string()
    } else {
        slider_parameter("level", "1.0")
    };
    format!("<filter><effect><effectid>audiolevels</effectid>{param}</effect></filter>")
}

/// Clip gain leaves trimmed Audio Levels automation unchanged.
#[test]
fn premiere_gain_preserves_keyframes() {
    let doc = slider_doc("audio", &[gain_filter("2", true) + &audio_levels(true), audio_levels(true)]);
    let (imp, _) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let clips = &imp.project.sequence(only_seq(&imp)).unwrap().audio_tracks[0].items;
    assert_eq!(clips[0].gain_db, 2.0);
    let level = |i: usize| clips[i].effect("volume").unwrap().param("level").unwrap();
    assert_eq!(level(0), level(1));
    assert_eq!(level(0).keyframes.len(), 2);
}

/// Gain is folded once into Audio Levels and stays stable over two cycles.
#[test]
fn premiere_gain_round_trip() {
    let doc = slider_doc("audio", &[gain_filter("2", true) + &audio_levels(false)]);
    let (mut imp, _) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let clip = &imp.project.sequence(only_seq(&imp)).unwrap().audio_tracks[0].items[0];
    assert_eq!(clip.gain_db, 2.0);
    assert_eq!(clip.effect("volume").unwrap().param("level").unwrap().value, ParamValue::Float(0.0));
    for _ in 0..2 {
        let (bytes, _) = export(&imp.project, only_seq(&imp), Format::Fcp7Xml, &ExportOptions::default()).unwrap();
        let xml = std::str::from_utf8(&bytes).unwrap();
        assert!(!xml.contains("<name>Gain</name>"));
        let parsed = roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).unwrap();
        let amplitude: f64 = parsed.descendants().find(|n| n.has_tag_name("parameter") && n.children().any(|c| c.has_tag_name("parameterid") && c.text() == Some("level")))
            .unwrap().children().find(|n| n.has_tag_name("value")).unwrap().text().unwrap().parse().unwrap();
        assert!((amplitude - 10.0_f64.powf(2.0 / 20.0)).abs() < 1e-6);
        imp = import(&bytes, Format::Fcp7Xml, None).unwrap().0;
        let clip = &imp.project.sequence(only_seq(&imp)).unwrap().audio_tracks[0].items[0];
        assert_eq!(clip.gain_db, 0.0);
        assert!((clip.effect("volume").unwrap().param("level").unwrap().value.as_f64().unwrap() - 2.0).abs() < 1e-6);
    }
}

/// Disabled Gain does not change clip gain.
#[test]
fn premiere_gain_disabled() {
    let doc = slider_doc("audio", &[gain_filter("2", false)]);
    let (imp, _) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    assert_eq!(imp.project.sequence(only_seq(&imp)).unwrap().audio_tracks[0].items[0].gain_db, 0.0);
}

/// Numeric Premiere ids distinguish the two Saturation controls.
#[test]
fn premiere_lumetri_values() {
    let a = slider_parameter("11", "-91445760000000000,1.255506992340087890625,0,0,0,0,0,0")
        + &slider_parameter("12", "-91445760000000000,18.94272613525390625,0,0,0,0,0,0")
        + &slider_parameter("29", "0.") + &slider_parameter("4", "");
    let b = slider_parameter("20", "-91445760000000000,117.687,0,0,0,0,0,0")
        + &slider_parameter("31", "100.") + &slider_parameter("51", "0.") + &slider_parameter("52", "50.");
    let doc = slider_doc("video", &[lumetri_filter(&a), lumetri_filter(&b)]);
    let (imp, _) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let clips = &imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items;
    let a = clips[0].effect("lumetri").unwrap();
    let b = clips[1].effect("lumetri").unwrap();
    assert_eq!(a.param("exposure").unwrap().value, ParamValue::Float(1.255506992340087890625));
    assert_eq!(a.param("contrast").unwrap().value, ParamValue::Float(18.94272613525390625));
    for (id, value) in [("saturation", 117.687), ("creative_sat", 100.0), ("vignette_amount", 0.0), ("vignette_midpoint", 50.0)] {
        assert_eq!(b.param(id).unwrap().value, ParamValue::Float(value));
    }
    for id in ["basic_on", "creative_on", "curves_on", "wheels_on", "vignette_on"] {
        assert_eq!(b.param(id).unwrap().value, ParamValue::Bool(true));
    }
}

/// Default sliders and neutral unmapped values preserve registry defaults.
#[test]
fn premiere_lumetri_defaults() {
    let mut params = String::new();
    for id in ["7", "8", "11", "12", "13", "14", "15", "16", "28", "30", "51", "53", "29", "33"] {
        params += &slider_parameter(id, "-91445760000000000,0.,0,0,0,0,0,0");
    }
    for id in ["20", "26", "31"] { params += &slider_parameter(id, "100."); }
    for id in ["52", "54"] { params += &slider_parameter(id, "50."); }
    for id in ["3", "23", "37", "46", "50"] { params += &slider_parameter(id, "-91445760000000000,true,0,0,0,0,0,0"); }
    params += &slider_parameter("24", "false");
    let doc = slider_doc("video", &[lumetri_filter(&params)]);
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let effect = imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items[0].effect("lumetri").unwrap();
    assert_eq!(effect.params, filmcraft_project::find_effect("lumetri").unwrap().instance().params);
    assert!(!rep.mentions("not imported"), "{rep}");
}

/// Invalid tuples warn without replacing the default exposure.
#[test]
fn premiere_lumetri_bad_tuple() {
    let doc = slider_doc("video", &[lumetri_filter(&slider_parameter("11", "-91445760000000000,abc,0,0,0,0,0,0"))]);
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    assert_eq!(imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items[0].effect("lumetri").unwrap().param("exposure").unwrap().value, ParamValue::Float(0.0));
    assert!(rep.mentions("Lumetri: parameter 11 value not read; skipped"));
    assert!(rep.mentions("Lumetri: 1 parameter not imported"));
}

/// Unmapped data produces one summary for each filter.
#[test]
fn premiere_lumetri_unmapped_data() {
    let params = slider_parameter("29", "-91445760000000000,35.,0,0,0,0,0,0") + &slider_parameter("39", "YWJjZGVmZ2hpamtsbW5vcA==");
    let doc = slider_doc("video", &[lumetri_filter(&params)]);
    let (_, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let warnings: Vec<_> = rep.warnings().filter(|e| e.message.starts_with("Lumetri:")).collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].message, "Lumetri: 2 parameters not imported");
    assert_eq!(warnings[0].count, 1);
}

/// An empty Premiere name still resolves Lumetri.
#[test]
fn premiere_lumetri_empty_name() {
    let doc = slider_doc("video", &[lumetri_filter("")]);
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    assert!(imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items[0].effect("lumetri").is_some());
    assert!(!rep.mentions("effect \"\" is not supported"));
}

/// Unknown ids with empty names remain unsupported.
#[test]
fn premiere_lumetri_lookup_negative_control() {
    let doc = slider_doc("video", &["<filter><effect><name /><effectid>NoSuchEffectXyz</effectid></effect></filter>".into()]);
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    assert!(imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items[0].effect("lumetri").is_none());
    assert!(rep.mentions("effect \"NoSuchEffectXyz\" is not supported"));
}


/// Repeated Gain filters add, clamp and warn on invalid values.
#[test]
fn premiere_gain_additive_and_invalid() {
    let fallback = gain_filter("-91445760000000000,3.,0,0,0,0,0,0", true).replace("{61756678, 4761696e, 4b657947}", "alternate").replace("<name>Gain</name>", "<name>gAiN</name>");
    let filters = gain_filter("2", true) + &fallback + &gain_filter("abc", true);
    let doc = slider_doc("audio", &[filters, gain_filter("95", true) + &gain_filter("5", true), gain_filter("-95", true) + &gain_filter("-5", true)]);
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    let clips = &imp.project.sequence(only_seq(&imp)).unwrap().audio_tracks[0].items;
    assert_eq!(clips[0].gain_db, 5.0);
    assert_eq!(clips[1].gain_db, 96.0);
    assert_eq!(clips[2].gain_db, -96.0);
    assert!(rep.mentions("Gain: value not read; skipped"));
}

/// Keyframes and malformed tuples count once while bypass disables the effect.
#[test]
fn premiere_lumetri_keyframes_and_bypass() {
    let params = "<parameter><parameterid>11</parameterid><value>2.</value><keyframe><when>6148</when><value>3.</value></keyframe></parameter>".to_string()
        + &slider_parameter("12", "0,2") + &slider_parameter("unknown", "true") + &slider_parameter("7", "NaN");
    let bypass = lumetri_filter(&params).replace("<pproBypass>false", "<pproBypass>true");
    let disabled = lumetri_filter("").replace("<filter>", "<filter><enabled>false</enabled>");
    let doc = slider_doc("video", &[bypass, disabled]);
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    for clip in &imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items {
        let effect = clip.effect("lumetri").unwrap();
        assert!(!effect.enabled);
        assert_eq!(effect.param("exposure").unwrap().value, ParamValue::Float(0.0));
    }
    assert!(rep.mentions("Lumetri: 3 parameters not imported"));
    assert!(rep.mentions("Lumetri: parameter 12 value not read; skipped"));
    assert!(!rep.mentions("Lumetri: parameter 11 value not read"));
}

/// An empty name falls back to the effectid, so the existing name lookup resolves it. A non-empty name that
/// matches nothing is not rescued by the effectid: the effect is still skipped, as before.
#[test]
fn premiere_lumetri_generic_lookup() {
    for id in ["GAUSSIAN_BLUR", "Gaussian Blur"] {
        let filter = format!("<filter><effect><name /><effectid>{id}</effectid></effect></filter>");
        let doc = slider_doc("video", &[filter]);
        let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
        assert!(imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items[0].effect("gaussian_blur").is_some());
        assert!(!rep.mentions("is not supported"));
    }
    let filter = "<filter><effect><name>Mystery Glow</name><effectid>GAUSSIAN_BLUR</effectid></effect></filter>".to_string();
    let doc = slider_doc("video", &[filter]);
    let (imp, rep) = import(doc.as_bytes(), Format::Fcp7Xml, None).unwrap();
    assert!(imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items[0].effect("gaussian_blur").is_none());
    assert!(rep.mentions("effect \"Mystery Glow\" is not supported"), "{rep}");
}
