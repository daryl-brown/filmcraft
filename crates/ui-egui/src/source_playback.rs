//! Source-file auditioning. Playback is presentation state: it never edits the project.

use std::sync::{Arc, Mutex};

use filmcraft_engine::clip_ops::source_view;
use filmcraft_project::ItemId;
use filmcraft_time::Tick;
use serde_json::json;

use crate::{AUDIO_RETRY_S, AUDIO_STALL_S, FilmcraftApp, Playback};

#[derive(Default)]
pub struct SourcePlayback {
    pub clock: Playback,
    item: Option<ItemId>,
    shown: Tick,
    range_start: Option<Tick>,
    failure: Arc<Mutex<Option<String>>>,
}

impl FilmcraftApp {
    pub fn toggle_source_play(&mut self) -> Result<(), String> {
        if self.source_playback.clock.playing {
            self.stop_source();
            Ok(())
        } else {
            self.play_source()
        }
    }

    pub fn play_source(&mut self) -> Result<(), String> {
        self.start_source_playback(None, None, 1.0)
    }

    /// J / K / L in the Source monitor: play at `speed` × real time (negative = backward) from the
    /// Source playhead. Sound plays at normal forward speed only, as in the Program monitor.
    pub fn shuttle_source(&mut self, speed: f64) -> Result<(), String> {
        if !speed.is_finite() || speed == 0.0 {
            return Err("Source shuttle speed must be a non-zero number".into());
        }
        self.start_source_playback(None, None, speed.clamp(-8.0, 8.0))
    }

    pub fn play_source_range(&mut self, from_playhead: bool, preroll: bool) -> Result<(), String> {
        let item = self.session.state.source_item.ok_or("Open a Source clip first")?;
        let view = source_view(&self.session, item).ok_or("Source item is unavailable")?;
        let range = view.selected_range();
        if range.duration.0 <= 0 {
            return Err("Source marked range is empty".into());
        }
        let seconds = |v: f64| Tick::from_seconds_f64(if v.is_finite() { v.clamp(0.0, 3600.0) } else { 0.0 });
        let start = if from_playhead {
            self.session.state.source_playhead
        } else if preroll {
            Tick(range.start.0.saturating_sub(seconds(self.session.prefs.playback.preroll_seconds).0)).max(view.start)
        } else {
            range.start
        };
        let stop =
            if preroll { Tick(range.end().0.saturating_add(seconds(self.session.prefs.playback.postroll_seconds).0)).min(view.end) } else { range.end() };
        if start >= stop {
            return Err("Source playhead is at or beyond the Out point".into());
        }
        self.start_source_playback(Some(start), Some(stop), 1.0)
    }

    fn start_source_playback(&mut self, start: Option<Tick>, stop: Option<Tick>, speed: f64) -> Result<(), String> {
        let item = self.session.state.source_item.ok_or("Open a media file in the Source monitor first")?;
        let view = source_view(&self.session, item).ok_or("Source item is unavailable")?;
        let media = self.session.project.item(view.media).and_then(|i| i.as_media()).ok_or("Source playback currently supports media files and subclips")?;
        if media.offline {
            return Err("Source media is offline: use Link Media first".into());
        }
        if view.end <= view.start {
            return Err("Source clip has no playable duration".into());
        }
        let looping = self.source_playback.clock.looping;
        let range = view.selected_range();
        if looping && range.duration.0 <= 0 {
            return Err("Source marked range is empty".into());
        }
        let first = if looping { range.start } else { view.start };
        let end = stop.unwrap_or(if looping { range.end() } else { view.end });
        self.stop();
        self.stop_source();
        let mut time = start.unwrap_or(self.session.state.source_playhead);
        // forward from the end (or before a looped range) starts over; backward runs from where it is
        if start.is_none() && speed > 0.0 && (time < first || time >= view.rate.snap(Tick(end.0.saturating_sub(view.rate.frame_duration().0))).max(first)) {
            time = first;
        }
        self.session.execute("source.setPlayhead", json!({"time": time.0})).map_err(|e| e.to_string())?;
        let time = self.session.state.source_playhead;
        self.source_playback.range_start = start;
        self.source_playback.item = Some(item);
        self.source_playback.shown = time;
        self.source_playback.clock = Playback { playing: true, speed, anchor_time: -1.0, anchor_tick: time, looping, stop_at: stop, ..Default::default() };
        self.source_playback.clock.meter.start(speed);
        self.start_source_audio(0.0);
        Ok(())
    }

    pub fn stop_source(&mut self) {
        if !self.source_playback.clock.playing {
            return;
        }
        self.source_playback.clock.playing = false;
        self.source_playback.clock.audio_clock = false;
        self.source_playback.clock.meter.finish();
        self.frames.stop_prefetch();
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
    }

    fn start_source_audio(&mut self, now: f64) {
        self.source_playback.clock.audio_clock = false;
        if (self.source_playback.clock.speed - 1.0).abs() > 1e-9 {
            if let Some(audio) = self.audio.as_mut() {
                audio.stop();
            }
            self.source_playback.clock.audio_retry_at = 0.0;
            return;
        }
        let Some(item) = self.source_playback.item else { return };
        let Some(view) = source_view(&self.session, item) else { return };
        let Some(media) = self.session.project.item(view.media).and_then(|i| i.as_media()) else { return };
        if !media.info.has_audio() {
            self.source_playback.clock.audio_retry_at = 0.0;
            return;
        }
        let channels = media.interpret.audio_channels.as_ref().and_then(|m| m.clips.first()).cloned().unwrap_or_default();
        let Some(source) = self.session.source_if_open(view.media) else {
            self.ui.status = "Opening source audio".into();
            self.source_playback.clock.audio_retry_at = now + AUDIO_RETRY_S;
            return;
        };
        let Some(audio) = self.audio.as_mut() else { return };
        let sr = audio.sample_rate();
        if sr == 0 || audio.channels() == 0 {
            self.ui.status = "Source audio output has an invalid sample rate or channel count".into();
            return;
        }
        let map = [self.session.prefs.audio_hardware.map_left, self.session.prefs.audio_hardware.map_right];
        let mixdown = filmcraft_audio_dsp::channels::Mixdown::from_id(&self.session.prefs.audio.mixdown_type).unwrap_or_default();
        // A seek gets its own failure slot: a late callback from the old stream cannot stop it.
        self.source_playback.failure = Arc::new(Mutex::new(None));
        let failure = self.source_playback.failure.clone();
        let end = self
            .source_playback
            .clock
            .stop_at
            .unwrap_or_else(|| if self.source_playback.clock.looping { view.selected_range().end() } else { view.end })
            .to_units_floor(i64::from(sr));
        let mix = move |cursor: i64, buf: &mut [f32], ch: usize| {
            buf.fill(0.0);
            if ch == 0 || cursor >= end {
                return;
            }
            let frames = (buf.len() / ch).min(usize::try_from(end.saturating_sub(cursor)).unwrap_or(0));
            match source.audio(cursor, frames, sr) {
                Ok(mut sound) => {
                    if !channels.is_empty() {
                        sound.channels = channels.iter().filter_map(|c| sound.channels.get(usize::from(*c)).cloned()).collect();
                    }
                    let sound = filmcraft_render::audio::to_layout(sound, filmcraft_audio_dsp::channels::Layout::Stereo, mixdown);
                    let Some(left) = sound.channels.first() else { return };
                    let right = sound.channels.get(1).unwrap_or(left);
                    filmcraft_engine::settings::map_output(left, right, buf, ch, map);
                }
                Err(e) => {
                    *failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(format!("Source audio decoding failed: {e}"));
                }
            }
        };
        audio.stop();
        let cursor = self.session.state.source_playhead.to_units_floor(i64::from(sr));
        #[cfg(not(target_arch = "wasm32"))]
        let fill = crate::play_ahead::spawn(mix, cursor, sr, audio.channels(), self.source_playback.clock.audio_stats.clone());
        #[cfg(target_arch = "wasm32")]
        let fill = {
            let (mix, mut cursor) = (mix, cursor);
            Box::new(move |buf: &mut [f32], ch: usize| {
                mix(cursor, buf, ch);
                cursor = cursor.saturating_add(i64::try_from(buf.len() / ch.max(1)).unwrap_or(0));
            })
        };
        match audio.start(fill) {
            Ok(_) => {
                self.source_playback.clock.audio_clock = true;
                self.source_playback.clock.audio_retry_at = 0.0;
            }
            Err(e) => {
                self.ui.status = format!("Source audio output failed: {e}");
                self.source_playback.clock.audio_retry_at = now + AUDIO_RETRY_S;
            }
        }
    }

    /// Advance Source audition playback while playing; stop on a missing or unavailable item or a failure, and re-anchor when the Source playhead is moved
    /// externally.
    pub(crate) fn advance_source_playback(&mut self, ctx: &egui::Context) {
        if !self.source_playback.clock.playing {
            return;
        }
        let item = self.source_playback.item;
        let Some(view) = item.filter(|i| Some(*i) == self.session.state.source_item).and_then(|i| source_view(&self.session, i)) else {
            self.stop_source();
            return;
        };
        let error = self.source_playback.failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        if let Some(error) = error {
            self.stop_source();
            self.ui.status = error;
            return;
        }
        let now = self.now();
        let time = self.session.state.source_playhead;
        if time != self.source_playback.shown {
            self.source_playback.clock.anchor_tick = time;
            self.source_playback.clock.anchor_time = now;
            self.source_playback.clock.audio_seen = (0, now);
            self.start_source_audio(now);
        }
        if !self.source_playback.clock.audio_clock && self.source_playback.clock.audio_retry_at > 0.0 && now >= self.source_playback.clock.audio_retry_at {
            // The output failed mid-play: open it again where the wall clock got to, so a
            // recovering device brings the sound back without a pause/play.
            self.source_playback.clock.audio_retry_at = now + AUDIO_RETRY_S;
            self.source_playback.clock.anchor_tick = time;
            self.source_playback.clock.anchor_time = now;
            self.source_playback.clock.audio_seen = (0, now);
            self.start_source_audio(now);
        }
        if self.source_playback.clock.audio_clock
            && let Some(audio) = self.audio.as_mut()
        {
            audio.recover();
        }
        let clock = &mut self.source_playback.clock;
        if clock.anchor_time < 0.0 {
            clock.anchor_time = now;
            clock.audio_seen = (0, now);
        }
        let reading = if clock.audio_clock { self.audio.as_ref().and_then(|a| a.played_frames().map(|n| (n, a.sample_rate()))) } else { None };
        if let Some((frames, _)) = reading
            && frames != clock.audio_seen.0
        {
            clock.audio_seen = (frames, now);
        }
        if clock.audio_clock && (reading.is_none() || now - clock.audio_seen.1 >= AUDIO_STALL_S) {
            clock.anchor_tick = time;
            clock.anchor_time = now;
            clock.audio_clock = false;
            // A dead stream is reopened on a timer; a stalled one keeps the wall clock (#136).
            if reading.is_none() {
                clock.audio_retry_at = now + AUDIO_RETRY_S;
            }
            if let Some(audio) = self.audio.as_mut() {
                audio.stop();
            }
            self.ui.status = "Source audio output stopped responding: playing without sound (check Settings > Audio Hardware)".into();
        }
        let elapsed = match reading {
            Some((frames, sr)) if clock.audio_clock && sr > 0 => frames as f64 / f64::from(sr),
            _ => (now - clock.anchor_time).max(0.0),
        };
        let next = Tick(clock.anchor_tick.0.saturating_add(Tick::from_seconds_f64(elapsed * clock.speed).0));
        if clock.speed < 0.0 {
            // backward: stops on the first frame (as the Program monitor stops at the start)
            let ended = next <= view.start;
            match self.session.execute("source.setPlayhead", json!({"time": next.max(view.start).0})) {
                Ok(_) => self.source_playback.shown = self.session.state.source_playhead,
                Err(e) => {
                    self.stop_source();
                    self.ui.status = e.to_string();
                    return;
                }
            }
            if ended {
                self.stop_source();
            }
            ctx.request_repaint();
            return;
        }
        let range = view.selected_range();
        let end = clock.stop_at.unwrap_or_else(|| if clock.looping { range.end() } else { view.end }).min(view.end);
        let ended = next >= end;
        if ended && clock.looping {
            let start = self.source_playback.range_start.unwrap_or(range.start).max(view.start);
            if let Err(e) = self.session.execute("source.setPlayhead", json!({"time":start.0})) {
                self.stop_source();
                self.ui.status = e.to_string();
                return;
            }
            let time = self.session.state.source_playhead;
            self.source_playback.shown = time;
            self.source_playback.clock.anchor_tick = time;
            self.source_playback.clock.anchor_time = now;
            self.source_playback.clock.audio_seen = (0, now);
            self.start_source_audio(now);
            ctx.request_repaint();
            return;
        }
        let next = if ended { Tick(end.0.saturating_sub(view.rate.frame_duration().0)).max(view.start) } else { next.max(view.start) };
        match self.session.execute("source.setPlayhead", json!({"time": next.0})) {
            Ok(_) => self.source_playback.shown = self.session.state.source_playhead,
            Err(e) => {
                self.stop_source();
                self.ui.status = e.to_string();
                return;
            }
        }
        if ended {
            self.stop_source();
        }
        ctx.request_repaint();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use filmcraft_engine::{Services, Session};
    use filmcraft_project::{ItemId, ItemKind, Label, MediaClip, MediaRef};
    use serde_json::json;

    use crate::{AudioOut, FilmcraftApp};

    struct GatedFs {
        bytes: Vec<u8>,
        entered: Sender<()>,
        gate: Mutex<Option<Receiver<()>>>,
    }

    impl Services for GatedFs {
        fn read_file(&self, _path: &str) -> std::io::Result<Vec<u8>> {
            let gate = self.gate.lock().unwrap().take();
            if let Some(gate) = gate {
                self.entered.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(30)).unwrap();
            }
            Ok(self.bytes.clone())
        }
        fn write_file(&self, _path: &str, _bytes: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn file_size(&self, _path: &str) -> std::io::Result<u64> {
            Ok(self.bytes.len() as u64)
        }
    }

    pub(crate) struct Gate {
        pub(crate) entered: Receiver<()>,
        pub(crate) release: Sender<()>,
    }

    impl Drop for Gate {
        fn drop(&mut self) {
            let _ = self.release.send(());
        }
    }

    pub(crate) fn gated_session() -> (Session, ItemId, Gate) {
        let bytes = filmcraft_media::wav::write_wav16(&vec![0.0; 48_000 * 2 * 20], 2, 48_000);
        let source = filmcraft_engine::MediaPool::default().open_bytes("test.wav", bytes.clone().into()).unwrap();
        let (entered, ready) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let mut session = Session::new(Arc::new(GatedFs { bytes, entered, gate: Mutex::new(Some(gate)) }));
        let info = source.info().clone();
        let item = Arc::make_mut(&mut session.project).add_item("test", Label::Iris, ItemKind::Media(Box::new(MediaClip {
            media: MediaRef::File { path: "/test.wav".into() }, info, interpret: Default::default(), mark_in: None, mark_out: None,
            markers: Vec::new(), offline: false, proxy: None, identity: None,
        })), None);
        (session, item, Gate { entered: ready, release })
    }

    struct Output(Arc<AtomicUsize>);
    impl AudioOut for Output {
        fn start(&mut self, _fill: Box<dyn FnMut(&mut [f32], usize) + Send>) -> Result<u32, String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(48_000)
        }
        fn stop(&mut self) {}
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn channels(&self) -> usize {
            2
        }
        fn played_frames(&self) -> Option<u64> {
            Some(0)
        }
    }

    #[test]
    fn source_audio_returns_while_opening_and_retries_on_the_wall_clock() {
        let (mut session, item, gate) = gated_session();
        session.execute("source.open", json!({"item": item.0})).unwrap();
        let mut app = FilmcraftApp::new(session);
        let starts = Arc::new(AtomicUsize::new(0));
        app.audio = Some(Box::new(Output(starts.clone())));
        let began = Instant::now();
        app.play_source().unwrap();
        assert!(began.elapsed() < Duration::from_secs(5));
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(app.source_playback.clock.playing);
        assert!(!app.source_playback.clock.audio_clock);
        assert!(app.source_playback.clock.audio_retry_at > 0.0);
        assert_eq!(starts.load(Ordering::Relaxed), 0);
        assert_eq!(app.ui.status, "Opening source audio");
        let ctx = egui::Context::default();
        app.set_now_for_test(0.0);
        app.advance_source_playback(&ctx);
        app.set_now_for_test(1.0);
        app.advance_source_playback(&ctx);
        assert!(app.session.state.source_playhead > filmcraft_time::Tick::ZERO);
        gate.release.send(()).unwrap();
        let job = app.session.jobs[0].id;
        app.session.execute("file.importStatus", json!({"job": job, "wait": true, "timeoutMs": 5000})).unwrap();
        app.set_now_for_test(3.0);
        app.advance_source_playback(&ctx);
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        assert!(app.source_playback.clock.audio_clock);
        assert_eq!(app.source_playback.clock.audio_retry_at, 0.0);
        app.stop_source();
    }
}
