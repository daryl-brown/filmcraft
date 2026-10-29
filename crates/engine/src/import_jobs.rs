//! Media opening jobs. Workers own handles and plain data; only polling edits the session.
//!
//! `file.import {background: true}` opens media and walks folders off the calling thread.
//! `openThreads` sets the number of concurrent opens (clamped to 1 to 16), otherwise
//! `FILMCRAFT_OPEN_THREADS` supplies it, with a default of four.
//! Documents and image sequences remain immediate. Without `background`, and on wasm32,
//! imports retain the synchronous command behaviour. `file.importStatus` polls, cancels or
//! waits for import jobs (a 600000 ms default timeout, clamped to 1 to 3600000 ms).
//! Outcomes have one owner: either the status command or `Session::take_import_outcomes`.
//! A selected status query drains only that job's outcomes. Finished jobs commit in one
//! undo group, including the files opened before cancellation. No success toast is added.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use filmcraft_project::{BinId, ItemId, MediaIdentity};
use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::{bool_p, u64_p};
use crate::{Result, Session};
#[cfg(not(target_arch = "wasm32"))]
use crate::EngineError;

#[derive(Debug, Serialize)]
pub struct ImportOutcome {
    pub job: u64,
    pub items: Vec<ItemId>,
    pub errors: Vec<String>,
    pub bins: Vec<u64>,
    /// Sidecars skipped by the shared folder walker.
    pub skipped: u64,
}

struct Opened {
    path: String,
    name: String,
    identity: MediaIdentity,
    source: filmcraft_media::SharedSource,
    bin: Option<usize>,
}

struct Bin {
    name: String,
    parent: Option<usize>,
}

#[derive(Default)]
struct Results {
    opened: Vec<Opened>,
    bins: Vec<Bin>,
    errors: Vec<String>,
    skipped: u64,
}

pub struct PendingImport {
    pub job: u64,
    parent: Option<BinId>,
    results: Arc<Mutex<Results>>,
    current: Arc<Mutex<Vec<(usize, String, web_time::Instant)>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Default number of files opened concurrently by a background import.
pub const OPEN_CONCURRENCY: usize = 4;

#[cfg(not(target_arch = "wasm32"))]
fn open_limit(p: &Value, environment: Option<&str>) -> usize {
    let integer = |v: &Value| v.as_i64().map(|n| n.clamp(1, 16) as usize).or_else(|| v.as_u64().map(|n| n.clamp(1, 16) as usize));
    p.get("openThreads").and_then(integer)
        .or_else(|| environment.and_then(|v| v.parse::<i64>().ok()).map(|n| n.clamp(1, 16) as usize))
        .unwrap_or(OPEN_CONCURRENCY)
}

fn current_status(current: &[(usize, String, web_time::Instant)]) -> Option<String> {
    let (_, name, began) = current.iter().min_by_key(|(_, _, began)| *began)?;
    let mut status = format!("{name} ({} s)", began.elapsed().as_secs());
    if current.len() > 1 {
        status.push_str(&format!(", {} files in progress", current.len()));
    }
    Some(status)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn start(s: &mut Session, p: &Value, paths: Vec<String>, parent: Option<BinId>) -> Result<Value> {
    let as_sequence = bool_p(p, "imageSequence").unwrap_or(false);
    let detect = !as_sequence && s.prefs.media.import_image_sequences && paths.len() == 1;
    let mut immediate = Vec::new();
    let mut work = Vec::new();
    for path in paths {
        if as_sequence
            || (detect && crate::commands::detect_image_sequence(s, &path))
            || matches!(crate::media_browser::kind_of(&path), Some("project" | "caption"))
        {
            immediate.push(path);
        } else {
            work.push(path);
        }
    }
    let mut out = json!({"items": [], "errors": []});
    if !immediate.is_empty() {
        let mut q = p.clone();
        q["paths"] = json!(immediate);
        q["background"] = json!(false);
        q["replace"] = json!(false);
        match s.execute("file.import", q) {
            Ok(v) => out = v,
            Err(e) => out["errors"] = json!([e.to_string()]),
        }
    }
    if work.is_empty() {
        return Ok(out);
    }
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0).checked_add(1).ok_or_else(|| EngineError::Other("job ids exhausted".into()))?;
    let label = format!("Opening {} file{}", work.len(), if work.len() == 1 { "" } else { "s" });
    let job = crate::Job { id, label, progress: Default::default(), result: Default::default() };
    job.progress.total.store(work.len() as u64, Ordering::Relaxed);
    let results = Arc::new(Mutex::new(Results::default()));
    let current = Arc::new(Mutex::new(Vec::new()));
    let worker = Worker {
        media: s.media.clone(), services: s.services.clone(), progress: job.progress.clone(), results: results.clone(), current: current.clone(),
        limit: open_limit(p, std::env::var("FILMCRAFT_OPEN_THREADS").ok().as_deref()),
    };
    let result = job.result.clone();
    let run = move || {
        let began = web_time::Instant::now();
        worker.run(work);
        let seconds = began.elapsed().as_secs_f64();
        let frames = worker.progress.done.load(Ordering::Relaxed);
        *lock(&result) = Some(Ok(filmcraft_export::Report {
            path: String::new(), frames, seconds, bytes: 0, render_fps: frames as f64 / seconds.max(1e-6), extra_files: Vec::new(),
        }));
        worker.progress.finished.store(true, Ordering::Release);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    // Publish only after spawning succeeds, so a spawn failure cannot leave a pending job.
    std::thread::Builder::new().name("filmcraft-import".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    s.jobs.push(job);
    s.import_jobs.push(PendingImport { job: id, parent, results, current });
    out["job"] = json!(id);
    out["pending"] = json!(true);
    if bool_p(p, "wait").unwrap_or(false) {
        let waited = status(s, &json!({"job": id, "wait": true, "timeoutMs": u64_p(p, "timeoutMs").unwrap_or(600_000)}))?;
        out["pending"] = json!(waited["pending"].as_u64().unwrap_or(0) != 0);
        merge_waited(&mut out, &waited);
    }
    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
fn merge_waited(out: &mut Value, waited: &Value) {
    if let Some(outcomes) = waited["outcomes"].as_array() {
        for outcome in outcomes {
            for key in ["items", "errors", "bins"] {
                let values = outcome[key].as_array().cloned().unwrap_or_default();
                if out.get(key).is_none() {
                    out[key] = json!([]);
                }
                if let Some(all) = out[key].as_array_mut() {
                    all.extend(values);
                }
            }
            out["skipped"] = outcome["skipped"].clone();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
type IndexedOpens = Mutex<Vec<(usize, std::result::Result<Opened, String>)>>;

#[cfg(not(target_arch = "wasm32"))]
struct Worker {
    media: Arc<crate::media_pool::MediaPool>,
    services: Arc<dyn crate::Services>,
    progress: Arc<filmcraft_export::Progress>,
    results: Arc<Mutex<Results>>,
    current: Arc<Mutex<Vec<(usize, String, web_time::Instant)>>>,
    limit: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl Worker {
    fn cancelled(&self) -> bool {
        self.progress.cancel.load(Ordering::Relaxed)
    }

    fn run(&self, paths: Vec<String>) {
        let mut budget = crate::media_browser::FOLDER_IMPORT_MAX_FOLDERS;
        let mut files = Vec::new();
        for path in paths {
            if self.cancelled() {
                break;
            }
            files.extend(self.files(path, &mut budget));
        }
        self.progress.total.store(files.len() as u64, Ordering::Relaxed);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let opened = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..self.limit.min(files.len()) {
                let run = || self.open_next(&files, &next, &opened);
                if std::thread::Builder::new().name("filmcraft-open".into()).spawn_scoped(scope, run).is_err() {
                    // If a thread cannot be started, this worker drains the same queue.
                    self.open_next(&files, &next, &opened);
                    break;
                }
            }
        });
        let mut opened = lock(&opened);
        opened.sort_by_key(|(index, _)| *index);
        let mut results = lock(&self.results);
        for (_, result) in opened.drain(..) {
            match result {
                Ok(file) => results.opened.push(file),
                Err(error) => results.errors.push(error),
            }
        }
        drop(results);
        let n = lock(&self.results).opened.len();
        *lock(&self.progress.status) = if self.cancelled() {
            format!("Stopped: committing {n} already opened file(s)")
        } else {
            format!("Opened {n} file(s)")
        };
    }

    fn files(&self, path: String, budget: &mut usize) -> Vec<(String, Option<usize>)> {
        *lock(&self.progress.status) = path.clone();
        if !crate::media_browser::is_folder(&*self.services, &path) {
            return vec![(path, None)];
        }
        let mut skipped = 0;
        let folder = crate::media_browser::walk_folder(&*self.services, &path, 0, budget, &mut skipped);
        {
            let mut results = lock(&self.results);
            results.skipped = results.skipped.saturating_add(skipped);
        }
        let mut files = Vec::new();
        if let Some(folder) = folder {
            self.folder(folder, None, &mut files);
        } else {
            lock(&self.results).errors.push(format!("no media files in {path}"));
        }
        files
    }

    fn open_next(&self, files: &[(String, Option<usize>)], next: &std::sync::atomic::AtomicUsize, opened: &IndexedOpens) {
        while !self.cancelled() {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some((path, bin)) = files.get(index) else { break };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.open_one(index, path, *bin)))
                .unwrap_or_else(|_| Err(format!("{path}: the file could not be opened (internal error)")));
            lock(opened).push((index, result));
            let mut current = lock(&self.current);
            current.retain(|(i, _, _)| *i != index);
            if let Some(status) = current_status(&current) {
                *lock(&self.progress.status) = status;
            }
            self.progress.done.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn open_one(&self, index: usize, path: &str, bin: Option<usize>) -> std::result::Result<Opened, String> {
        let began = web_time::Instant::now();
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        {
            let mut current = lock(&self.current);
            current.push((index, name.clone(), began));
            if let Some(status) = current_status(&current) {
                *lock(&self.progress.status) = status;
            }
        }
        // One container open cannot be interrupted. Cancellation is checked between files.
        self.open(path, name, bin).map_err(|e| format!("{path}: {e} ({:.1} s)", began.elapsed().as_secs_f64()))
    }

    fn folder(&self, folder: crate::media_browser::Folder, parent: Option<usize>, files: &mut Vec<(String, Option<usize>)>) {
        let mut results = lock(&self.results);
        let bin = results.bins.len();
        results.bins.push(Bin { name: folder.name, parent });
        drop(results);
        files.extend(folder.files.into_iter().map(|path| (path, Some(bin))));
        for sub in folder.subs {
            self.folder(sub, Some(bin), files);
        }
    }

    fn open(&self, path: &str, name: String, bin: Option<usize>) -> Result<Opened> {
        let streamed = filmcraft_media::is_importable(std::path::Path::new(path)) && self.services.reader(path).is_some();
        let (identity, source) = if streamed {
            let identity = crate::relink::identity_of(&*self.services, path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            (identity, self.media.open_file(path, &*self.services)?)
        } else {
            let bytes: Arc<[u8]> = self.services.read_file(path).map_err(|e| EngineError::Other(e.to_string()))?.into();
            (crate::relink::identity_of_bytes(&bytes), self.media.open_bytes(&name, bytes)?)
        };
        Ok(Opened { path: path.into(), name, identity, source, bin })
    }
}

/// Apply finished workers on the session thread, including the files opened before cancellation.
pub fn poll(s: &mut Session) {
    let mut i = 0;
    while i < s.import_jobs.len() {
        let pending = &s.import_jobs[i];
        let job = s.jobs.iter().find(|j| j.id == pending.job);
        if job.is_some_and(|j| !j.progress.finished.load(Ordering::Acquire)) {
            if let Some(status) = current_status(&lock(&pending.current)) {
                if let Some(job) = job {
                    *lock(&job.progress.status) = status;
                }
            }
            i += 1;
            continue;
        }
        let cancelled = job.is_some_and(|j| j.progress.cancel.load(Ordering::Relaxed));
        let failure = job.and_then(|j| {
            lock(&j.result).as_ref().and_then(|r| r.as_ref().err().cloned()).or_else(|| lock(&j.progress.error).clone())
        });
        let pending = s.import_jobs.remove(i);
        let _scope = crate::watchdog::scope("import commit", &pending.job.to_string());
        let mut results = std::mem::take(&mut *lock(&pending.results));
        if let Some(error) = failure {
            results.errors.push(error);
        }
        let outcome = match s.edit_group("Import Files", |s| Ok(commit(s, pending.job, pending.parent, results, cancelled))) {
            Ok(outcome) => outcome,
            Err(e) => ImportOutcome { job: pending.job, items: Vec::new(), errors: vec![e.to_string()], bins: Vec::new(), skipped: 0 },
        };
        if cancelled {
            let names = outcome.items.iter().take(8).filter_map(|id| s.project.item(*id)).map(|item| item.name.as_str()).collect::<Vec<_>>().join(", ");
            if let Some(job) = s.jobs.iter().find(|j| j.id == outcome.job) {
                *lock(&job.progress.status) = format!("Stopped: imported {} already opened file(s): {names}", outcome.items.len());
            }
        }
        for error in &outcome.errors {
            s.error_toast("file.import", error.clone());
        }
        s.import_results.push(outcome);
    }
}

fn needed_bin(results: &Results, index: usize) -> bool {
    results.opened.iter().any(|file| {
        let mut at = file.bin;
        while let Some(i) = at {
            if i == index {
                return true;
            }
            at = results.bins.get(i).and_then(|b| b.parent);
        }
        false
    })
}

fn create_bins(s: &mut Session, results: &Results, parent: Option<BinId>, cancelled: bool, out: &mut ImportOutcome) -> Vec<Option<BinId>> {
    let mut bins = Vec::new();
    for (index, bin) in results.bins.iter().enumerate() {
        // On cancellation only create bins needed by an already opened file.
        let needed = !cancelled || needed_bin(results, index);
        let destination = bin.parent.and_then(|i| bins.get(i).copied().flatten()).or(parent);
        let id = if needed {
            match s.edit("New Bin", |pr, _| Ok(pr.add_bin(&bin.name, destination))) {
                Ok(id) => {
                    out.bins.push(id.0);
                    Some(id)
                }
                Err(e) => {
                    out.errors.push(e.to_string());
                    None
                }
            }
        } else {
            None
        };
        bins.push(id);
    }
    bins
}

fn commit(s: &mut Session, job: u64, parent: Option<BinId>, mut results: Results, cancelled: bool) -> ImportOutcome {
    let mut out = ImportOutcome { job, items: Vec::new(), errors: std::mem::take(&mut results.errors), bins: Vec::new(), skipped: results.skipped };
    let parent = parent.filter(|id| s.project.root.find_bin(*id).is_some());
    let bins = create_bins(s, &results, parent, cancelled, &mut out);
    for file in results.opened {
        let bin = file.bin.and_then(|i| bins.get(i).copied().flatten()).or(parent);
        match crate::commands::import_source(s, &file.path, file.name, file.source, file.identity, bin) {
            Ok(id) => out.items.push(id),
            Err(e) => out.errors.push(format!("{}: {e}", file.path)),
        }
    }
    match crate::commands::finish_import(s, out.items.iter().map(|id| id.0).collect(), Vec::new(), Vec::new(), Vec::new(), Vec::new()) {
        Ok(value) => out.errors.extend(value["errors"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string)),
        Err(e) => out.errors.push(e.to_string()),
    }
    out
}

pub(crate) fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let selected = u64_p(p, "job");
    let matches = |j: &&crate::Job| j.label.starts_with("Opening ") && selected.is_none_or(|id| id == j.id);
    if bool_p(p, "cancel").unwrap_or(false) {
        for job in s.jobs.iter().filter(matches) {
            job.progress.cancel.store(true, Ordering::Relaxed);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if bool_p(p, "wait").unwrap_or(false) {
        let timeout = std::time::Duration::from_millis(u64_p(p, "timeoutMs").unwrap_or(600_000).clamp(1, 3_600_000));
        let began = web_time::Instant::now();
        while s.jobs.iter().filter(matches).any(|j| !j.progress.finished.load(Ordering::Acquire)) && began.elapsed() < timeout {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    poll(s);
    let jobs: Vec<Value> = s.jobs.iter().filter(matches).map(|j| json!({
        "id": j.id, "label": j.label, "status": lock(&j.progress.status).clone(),
        "done": j.progress.done.load(Ordering::Relaxed), "total": j.progress.total.load(Ordering::Relaxed),
        "finished": j.progress.finished.load(Ordering::Acquire),
    })).collect();
    let pending = jobs.iter().filter(|j| j["finished"] == false).count();
    // A consumer owns what it drains. A selected query leaves other jobs' outcomes for the UI
    // or a later query; neither consumer can receive an outcome that was already drained.
    let mut outcomes = Vec::new();
    let mut remaining = Vec::new();
    for outcome in s.take_import_outcomes() {
        if selected.is_none_or(|id| id == outcome.job) {
            outcomes.push(outcome);
        } else {
            remaining.push(outcome);
        }
    }
    s.import_results = remaining;
    Ok(json!({"pending": pending, "jobs": jobs, "outcomes": outcomes}))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use serde_json::json;

    #[test]
    fn open_threads_precedence_clamping_and_hostile_values() {
        for (params, environment, expected) in [
            (json!({}), None, super::OPEN_CONCURRENCY),
            (json!({}), Some("3"), 3),
            (json!({}), Some("0"), 1),
            (json!({}), Some("-5"), 1),
            (json!({}), Some("99"), 16),
            (json!({}), Some("invalid"), super::OPEN_CONCURRENCY),
            (json!({"openThreads": 2}), Some("8"), 2),
            (json!({"openThreads": -1}), Some("8"), 1),
            (json!({"openThreads": 0}), None, 1),
            (json!({"openThreads": u64::MAX}), None, 16),
            (json!({"openThreads": 2.5}), Some("3"), 3),
            (json!({"openThreads": false}), Some("3"), 3),
            (json!({"openThreads": []}), None, super::OPEN_CONCURRENCY),
            (json!({"openThreads": "4"}), None, super::OPEN_CONCURRENCY),
            (json!(null), None, super::OPEN_CONCURRENCY),
        ] {
            assert_eq!(super::open_limit(&params, environment), expected);
        }
    }
}
