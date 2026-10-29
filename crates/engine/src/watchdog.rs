//! Named scopes for diagnosing stalls on one watched thread.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use web_time::Instant;

#[derive(Default)]
struct State {
    watched: Option<ThreadId>,
    stack: Vec<(u64, String, Instant)>,
    counter: u64,
    /// Open [`Pause`] guards: while any is open the thread is waiting on the user, not stalled.
    paused: u32,
}

/// Shared scope stack. Clones share the same watched thread and entries.
#[derive(Clone, Default)]
pub struct Monitor {
    state: Arc<Mutex<State>>,
}

impl Monitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn global() -> &'static Self {
        static MONITOR: OnceLock<Monitor> = OnceLock::new();
        MONITOR.get_or_init(Self::new)
    }

    pub fn watch_current_thread(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).watched = Some(thread::current().id());
    }

    pub fn scope(&self, kind: &str, detail: &str) -> Scope<'_> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.watched != Some(thread::current().id()) {
            return Scope { monitor: None, id: 0 };
        }
        // Refuse further entries on exhaustion rather than reusing a live id.
        let Some(id) = state.counter.checked_add(1) else { return Scope { monitor: None, id: 0 } };
        state.counter = id;
        let end = detail.char_indices().nth(160).map_or(detail.len(), |(i, _)| i);
        let detail = detail.get(..end).unwrap_or_default();
        let extra = if detail.is_empty() { 0 } else { detail.len().saturating_add(3) };
        let mut label = String::with_capacity(kind.len().saturating_add(extra));
        label.push_str(kind);
        if !detail.is_empty() {
            label.push_str(" (");
            label.push_str(detail);
            label.push(')');
        }
        state.stack.push((id, label, Instant::now()));
        Scope { monitor: Some(self), id }
    }

    /// Suspend stall detection while the watched thread waits on the user (a modal native dialog).
    /// While any [`Pause`] is held the snapshot is empty; when the last one drops, the open scopes
    /// start counting again from that moment, so the wait is never reported as a stall.
    pub fn pause(&self) -> Pause<'_> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.watched != Some(thread::current().id()) {
            return Pause { monitor: None };
        }
        state.paused = state.paused.saturating_add(1);
        Pause { monitor: Some(self) }
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.paused > 0 {
            return Snapshot::default();
        }
        Snapshot { scopes: state.stack.clone() }
    }
}

/// Ends the pause begun by [`Monitor::pause`] when dropped.
pub struct Pause<'a> {
    monitor: Option<&'a Monitor>,
}

impl Drop for Pause<'_> {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor {
            let mut state = monitor.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.paused = state.paused.saturating_sub(1);
            if state.paused == 0 {
                let now = Instant::now();
                for entry in &mut state.stack {
                    entry.2 = now;
                }
            }
        }
    }
}

/// Removes only its own entry, even when guards are dropped out of order.
pub struct Scope<'a> {
    monitor: Option<&'a Monitor>,
    id: u64,
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor {
            let mut state = monitor.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(index) = state.stack.iter().position(|(id, _, _)| *id == self.id) {
                state.stack.remove(index);
            }
        }
    }
}

pub fn watch_current_thread() {
    Monitor::global().watch_current_thread();
}

pub fn scope(kind: &str, detail: &str) -> Scope<'static> {
    Monitor::global().scope(kind, detail)
}

/// [`Monitor::pause`] on the global monitor.
pub fn pause() -> Pause<'static> {
    Monitor::global().pause()
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub scopes: Vec<(u64, String, Instant)>,
}

pub struct Config {
    pub threshold: Duration,
    pub repeat: Duration,
    pub poll: Duration,
}

impl Config {
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            repeat: threshold.saturating_mul(5).max(Duration::from_secs(10)),
            poll: (threshold / 4).clamp(Duration::from_millis(10), Duration::from_millis(250)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Stalled,
    StillStalled,
    Recovered,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub kind: ReportKind,
    pub elapsed: Duration,
    pub scopes: Vec<(String, Duration)>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.elapsed.as_secs_f64();
        match self.kind {
            ReportKind::Stalled => write!(f, "UI thread stalled for {seconds:.1} s: ")?,
            ReportKind::StillStalled => write!(f, "UI thread still stalled after {seconds:.1} s: ")?,
            ReportKind::Recovered => write!(f, "UI thread recovered after {seconds:.1} s (was in: ")?,
        }
        for (index, (label, _)) in self.scopes.iter().enumerate() {
            if index != 0 {
                write!(f, " > ")?;
            }
            write!(f, "{label}")?;
        }
        if self.kind == ReportKind::Recovered {
            write!(f, ")")?;
        }
        Ok(())
    }
}

struct Active {
    id: u64,
    reported: Instant,
    evaluated: Instant,
    elapsed: Duration,
    scopes: Vec<(String, Duration)>,
}

#[derive(Default)]
pub struct Tracker {
    active: Option<Active>,
}

impl Tracker {
    pub fn evaluate(&mut self, snapshot: &Snapshot, now: Instant, cfg: &Config) -> Option<Report> {
        let outer = snapshot.scopes.first();
        if self.active.as_ref().is_some_and(|active| outer.map(|entry| entry.0) != Some(active.id)) {
            let active = self.active.take()?;
            let since = now.saturating_duration_since(active.evaluated);
            return Some(Report {
                kind: ReportKind::Recovered,
                elapsed: active.elapsed.saturating_add(since),
                scopes: active.scopes.into_iter().map(|(label, age)| (label, age.saturating_add(since))).collect(),
            });
        }
        let (id, _, started) = outer?;
        let elapsed = now.saturating_duration_since(*started);
        let scopes = || -> Vec<(String, Duration)> {
            snapshot.scopes.iter().map(|(_, label, start)| (label.clone(), now.saturating_duration_since(*start))).collect()
        };
        if let Some(active) = &mut self.active {
            active.elapsed = elapsed;
            active.evaluated = now;
            active.scopes = scopes();
            if now.saturating_duration_since(active.reported) < cfg.repeat {
                return None;
            }
            active.reported = now;
            return Some(Report { kind: ReportKind::StillStalled, elapsed, scopes: active.scopes.clone() });
        }
        if elapsed < cfg.threshold {
            return None;
        }
        let scopes = scopes();
        self.active = Some(Active { id: *id, reported: now, evaluated: now, elapsed, scopes: scopes.clone() });
        Some(Report { kind: ReportKind::Stalled, elapsed, scopes })
    }
}

/// Stops and joins the worker when dropped.
pub struct Handle {
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<thread::JoinHandle<()>>,
    #[cfg(not(target_arch = "wasm32"))]
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(worker) = self.worker.take() {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

/// The worker owns shared monitor state, so the caller need not leak a test monitor.
#[cfg(not(target_arch = "wasm32"))]
pub fn start(monitor: &Monitor, cfg: Config, sink: Arc<dyn Fn(&Report) + Send + Sync>) -> std::io::Result<Handle> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let monitor = monitor.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let worker = thread::Builder::new().name("filmcraft-watchdog".into()).spawn(move || {
        let mut tracker = Tracker::default();
        while !stopping.load(Ordering::Acquire) {
            thread::park_timeout(cfg.poll);
            if stopping.load(Ordering::Acquire) {
                break;
            }
            if let Some(report) = tracker.evaluate(&monitor.snapshot(), Instant::now(), &cfg) {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(&report)));
            }
        }
    })?;
    Ok(Handle { worker: Some(worker), stop })
}

#[cfg(target_arch = "wasm32")]
pub fn start(_monitor: &Monitor, _cfg: Config, _sink: Arc<dyn Fn(&Report) + Send + Sync>) -> std::io::Result<Handle> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "stall watchdog threads are unavailable on wasm32"))
}

pub fn threshold_from_env(value: Option<&str>, default: Option<Duration>) -> Option<Duration> {
    let Some(value) = value else { return default };
    if value == "0" || value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("false") {
        return None;
    }
    match value.parse::<u64>() {
        Ok(ms) if ms > 0 => Some(Duration::from_millis(ms.clamp(250, 600_000))),
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Monitor, Report, ReportKind, Snapshot, Tracker, threshold_from_env};
    #[cfg(not(target_arch = "wasm32"))]
    use super::start;
    #[cfg(not(target_arch = "wasm32"))]
    use std::sync::Arc;
    use std::time::Duration;

    use web_time::Instant;

    fn stack(now: Instant, id: u64) -> Snapshot {
        Snapshot { scopes: vec![(id, "file.import (Z:\\a.mxf)".into(), now), (id + 1, "media.open (Z:\\a.mxf)".into(), now)] }
    }

    #[test]
    fn evaluate_reports_threshold_repeat_and_recovery_once() {
        let now = Instant::now();
        let cfg = Config::new(Duration::from_secs(2));
        let snapshot = stack(now, 1);
        let mut tracker = Tracker::default();
        assert!(tracker.evaluate(&snapshot, now + Duration::from_millis(1999), &cfg).is_none());
        let report = tracker.evaluate(&snapshot, now + cfg.threshold, &cfg).unwrap();
        assert_eq!(report.kind, ReportKind::Stalled);
        assert_eq!(report.elapsed, cfg.threshold);
        assert_eq!(report.scopes[1].1, cfg.threshold);
        assert_eq!(report.to_string(), "UI thread stalled for 2.0 s: file.import (Z:\\a.mxf) > media.open (Z:\\a.mxf)");
        assert!(tracker.evaluate(&snapshot, now + Duration::from_millis(11999), &cfg).is_none());
        let report = tracker.evaluate(&snapshot, now + Duration::from_secs(12), &cfg).unwrap();
        assert_eq!(report.kind, ReportKind::StillStalled);
        assert!(tracker.evaluate(&snapshot, now + Duration::from_millis(12300), &cfg).is_none());
        let report = tracker.evaluate(&Snapshot::default(), now + Duration::from_millis(12400), &cfg).unwrap();
        assert_eq!(report.kind, ReportKind::Recovered);
        assert_eq!(report.elapsed, Duration::from_millis(12400));
        assert_eq!(report.scopes[0].1, report.elapsed);
        assert_eq!(report.scopes[0].0, "file.import (Z:\\a.mxf)");
        assert!(tracker.evaluate(&Snapshot::default(), now + Duration::from_secs(100), &cfg).is_none());
    }

    #[test]
    fn evaluate_ignores_idle_and_scopes_that_close_early() {
        let now = Instant::now();
        let cfg = Config::new(Duration::from_secs(2));
        let mut tracker = Tracker::default();
        assert!(tracker.evaluate(&Snapshot::default(), now, &cfg).is_none());
        assert!(tracker.evaluate(&stack(now, 1), now + Duration::from_secs(1), &cfg).is_none());
        assert!(tracker.evaluate(&Snapshot::default(), now + Duration::from_secs(20), &cfg).is_none());
    }

    #[test]
    fn different_outermost_recovers_then_tracks_the_new_scope() {
        let now = Instant::now();
        let cfg = Config::new(Duration::from_secs(2));
        let mut tracker = Tracker::default();
        assert_eq!(tracker.evaluate(&stack(now, 1), now + cfg.threshold, &cfg).unwrap().kind, ReportKind::Stalled);
        let next = stack(now + Duration::from_secs(3), 3);
        let report = tracker.evaluate(&next, now + Duration::from_secs(3), &cfg).unwrap();
        assert_eq!(report.kind, ReportKind::Recovered);
        assert_eq!(report.elapsed, Duration::from_secs(3));
        assert!(tracker.evaluate(&next, now + Duration::from_secs(4), &cfg).is_none());
        assert_eq!(tracker.evaluate(&next, now + Duration::from_secs(5), &cfg).unwrap().kind, ReportKind::Stalled);
    }

    #[test]
    fn outermost_age_controls_stalls_and_last_stack_is_kept() {
        let now = Instant::now();
        let cfg = Config::new(Duration::from_secs(2));
        let mut tracker = Tracker::default();
        let mut snapshot = stack(now, 1);
        snapshot.scopes[1].2 = now + Duration::from_secs(1);
        let report = tracker.evaluate(&snapshot, now + cfg.threshold, &cfg).unwrap();
        assert_eq!(report.scopes[1].1, Duration::from_secs(1));
        snapshot.scopes.pop();
        assert!(tracker.evaluate(&snapshot, now + Duration::from_secs(3), &cfg).is_none());
        let report = tracker.evaluate(&Snapshot::default(), now + Duration::from_secs(4), &cfg).unwrap();
        assert_eq!(report.scopes.len(), 1);
        assert_eq!(report.elapsed, Duration::from_secs(4));
    }

    #[test]
    fn scopes_nest_and_drop_only_their_own_entry() {
        let monitor = Monitor::new();
        monitor.watch_current_thread();
        let a = monitor.scope("a", "");
        let b = monitor.scope("b", "detail");
        let c = monitor.scope("c", "");
        let snapshot = monitor.snapshot();
        let labels: Vec<_> = snapshot.scopes.iter().map(|entry| entry.1.as_str()).collect();
        assert_eq!(labels, ["a", "b (detail)", "c"]);
        drop(b);
        assert_eq!(monitor.snapshot().scopes.len(), 2);
        drop(a);
        assert_eq!(monitor.snapshot().scopes[0].1, "c");
        drop(c);
        assert!(monitor.snapshot().scopes.is_empty());
    }

    #[test]
    fn a_pause_hides_the_stack_and_restarts_the_clocks() {
        let monitor = Monitor::new();
        monitor.watch_current_thread();
        let _scope = monitor.scope("ui pass", "ui");
        let before = monitor.snapshot().scopes[0].2;
        {
            let outer = monitor.pause();
            let inner = monitor.pause();
            assert!(monitor.snapshot().scopes.is_empty());
            drop(inner);
            assert!(monitor.snapshot().scopes.is_empty());
            drop(outer);
        }
        let after = monitor.snapshot().scopes;
        assert_eq!(after.len(), 1);
        assert!(after[0].2 >= before);
        let worker_monitor = monitor.clone();
        std::thread::spawn(move || {
            let _pause = worker_monitor.pause();
            assert_eq!(worker_monitor.snapshot().scopes.len(), 1);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn unwatched_and_worker_scopes_are_inert() {
        let monitor = Monitor::new();
        let inert = monitor.scope("ignored", "");
        assert!(monitor.snapshot().scopes.is_empty());
        monitor.watch_current_thread();
        let worker_monitor = monitor.clone();
        std::thread::spawn(move || {
            let _scope = worker_monitor.scope("worker", "");
            assert!(worker_monitor.snapshot().scopes.is_empty());
        }).join().unwrap();
        drop(inert);
        assert!(monitor.snapshot().scopes.is_empty());
    }

    #[test]
    fn last_watched_thread_wins() {
        let monitor = Monitor::new();
        monitor.watch_current_thread();
        let worker_monitor = monitor.clone();
        std::thread::spawn(move || {
            worker_monitor.watch_current_thread();
            let _scope = worker_monitor.scope("worker", "");
            assert_eq!(worker_monitor.snapshot().scopes.len(), 1);
        }).join().unwrap();
        let _scope = monitor.scope("old thread", "");
        assert!(monitor.snapshot().scopes.is_empty());
    }

    #[test]
    fn detail_truncates_at_a_character_boundary() {
        let monitor = Monitor::new();
        monitor.watch_current_thread();
        let detail = format!("{}{}tail", "a".repeat(159), '\u{00e9}');
        let _scope = monitor.scope("open", &detail);
        assert_eq!(monitor.snapshot().scopes[0].1, format!("open ({}{})", "a".repeat(159), '\u{00e9}'));
    }

    #[test]
    fn display_formats_all_report_kinds() {
        for (kind, expected) in [
            (ReportKind::Stalled, "UI thread stalled for 2.3 s: a > b"),
            (ReportKind::StillStalled, "UI thread still stalled after 2.3 s: a > b"),
            (ReportKind::Recovered, "UI thread recovered after 2.3 s (was in: a > b)"),
        ] {
            let report = Report { kind, elapsed: Duration::from_millis(2300), scopes: vec![("a".into(), Duration::ZERO), ("b".into(), Duration::ZERO)] };
            assert_eq!(report.to_string(), expected);
        }
    }

    #[test]
    fn environment_threshold_table() {
        let default = Some(Duration::from_secs(2));
        for (input, expected) in [
            (None, default), (Some("0"), None), (Some("oFf"), None), (Some("FALSE"), None),
            (Some("1"), Some(Duration::from_millis(250))), (Some("250"), Some(Duration::from_millis(250))),
            (Some("2500"), Some(Duration::from_millis(2500))), (Some("999999"), Some(Duration::from_secs(600))),
            (Some("-1"), default), (Some(""), default), (Some("oops"), default),
            (Some("18446744073709551616"), default),
        ] {
            assert_eq!(threshold_from_env(input, default), expected, "{input:?}");
        }
        assert_eq!(threshold_from_env(None, None), None);
        assert_eq!(threshold_from_env(Some("bad"), None), None);
    }

    #[test]
    fn config_clamps_poll_and_repeat() {
        let short = Config::new(Duration::from_millis(1));
        assert_eq!(short.poll, Duration::from_millis(10));
        assert_eq!(short.repeat, Duration::from_secs(10));
        let medium = Config::new(Duration::from_millis(400));
        assert_eq!(medium.poll, Duration::from_millis(100));
        let long = Config::new(Duration::from_secs(3));
        assert_eq!(long.poll, Duration::from_millis(250));
        assert_eq!(long.repeat, Duration::from_secs(15));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_worker_reports_stall_and_recovery() {
        let monitor = Monitor::new();
        monitor.watch_current_thread();
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = Config { threshold: Duration::from_millis(40), repeat: Duration::from_secs(10), poll: Duration::from_millis(10) };
        let import = monitor.scope("file.import", "Z:\\a.mxf");
        let open = monitor.scope("media.open", "Z:\\a.mxf");
        let handle = start(&monitor, cfg, Arc::new(move |report| { let _ = tx.send(report.clone()); })).unwrap();
        let report = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(report.kind, ReportKind::Stalled);
        assert!(report.to_string().contains("file.import (Z:\\a.mxf) > media.open (Z:\\a.mxf)"));
        drop(open);
        drop(import);
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap().kind, ReportKind::Recovered);
        drop(handle);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn worker_survives_a_sink_panic_and_drop_wakes_a_long_poll() {
        let monitor = Monitor::new();
        monitor.watch_current_thread();
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = Config { threshold: Duration::ZERO, repeat: Duration::ZERO, poll: Duration::from_millis(10) };
        let handle = start(&monitor, cfg, Arc::new(move |report| {
            let _ = tx.send(report.clone());
            panic!("test sink failure");
        })).unwrap();
        let _scope = monitor.scope("test", "");
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        drop(handle);
        let cfg = Config { threshold: Duration::ZERO, repeat: Duration::ZERO, poll: Duration::from_secs(3600) };
        let handle = start(&monitor, cfg, Arc::new(|_| {})).unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let join = std::thread::spawn(move || {
            drop(handle);
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        join.join().unwrap();
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_start_is_unsupported() {
        let result = super::start(&Monitor::new(), Config::new(Duration::from_secs(2)), std::sync::Arc::new(|_| {}));
        assert_eq!(result.err().unwrap().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn execute_command_behaviour_is_unchanged() {
        let mut session = crate::Session::default();
        session.execute("file.newProject", serde_json::json!({"name": "watchdog test"})).unwrap();
        assert_eq!(session.project.name, "watchdog test");
        assert!(session.execute("watchdog.unknown", serde_json::json!({})).is_err());
    }
}
