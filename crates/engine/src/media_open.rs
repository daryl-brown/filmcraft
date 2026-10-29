//! Bring project media online without blocking the calling thread.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use filmcraft_project::ItemId;
use serde_json::{Value, json};

use crate::{Result, Session};

type Current = Arc<Mutex<Vec<(usize, String, web_time::Instant)>>>;

pub(crate) struct PendingOpen {
    job: u64,
    items: Vec<ItemId>,
    current: Current,
}

#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) fn retry_ready(last: Option<&web_time::Instant>, now: web_time::Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(*last) >= std::time::Duration::from_secs(5))
}

pub(crate) fn reset(s: &mut Session) {
    for pending in &s.open_workers {
        if let Some(job) = s.jobs.iter().find(|j| j.id == pending.job) {
            job.progress.cancel.store(true, Ordering::Relaxed);
        }
    }
    s.open_workers.clear();
    s.opening_items.clear();
    s.opening_times.clear();
}

pub(crate) fn poll(s: &mut Session) {
    s.open_workers.retain(|pending| {
        let job = s.jobs.iter().find(|j| j.id == pending.job);
        if job.is_some_and(|j| !j.progress.finished.load(Ordering::Acquire)) {
            if let Some(status) = crate::import_jobs::current_status(&pending.current.lock().unwrap_or_else(PoisonError::into_inner))
                && let Some(job) = job
            {
                *job.progress.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
            }
            return true;
        }
        for item in &pending.items {
            s.opening_items.remove(item);
        }
        false
    });
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn start(_s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({"job": null, "pending": false, "files": 0, "skipped": 0}))
}

#[cfg(not(target_arch = "wasm32"))]
fn candidates(s: &Session, p: &Value) -> Result<Vec<ItemId>> {
    use crate::commands::{bad, str_p};
    if let Some(items) = p.get("items") {
        let items = items.as_array().ok_or_else(|| bad("media.openInBackground", "`items` must be an array of ids"))?;
        return items.iter().map(|v| v.as_u64().map(ItemId).ok_or_else(|| bad("media.openInBackground", "invalid item id"))).collect();
    }
    if p.get("scope").is_some_and(|scope| !scope.is_string()) {
        return Err(bad("media.openInBackground", "`scope` must be a string"));
    }
    match str_p(p, "scope").unwrap_or("openSequences") {
        "all" => Ok(s.project.items.values().filter(|i| i.as_media().is_some()).map(|i| i.id).collect()),
        "openSequences" => {
            let (_, used) = crate::project_manager::used_ranges(&s.project, &s.state.open_sequences);
            Ok(used.into_iter().filter(|id| s.project.item(*id).is_some_and(|i| i.as_media().is_some())).collect())
        }
        _ => Err(bad("media.openInBackground", "unknown scope")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn work(s: &Session, p: &Value) -> Result<(Vec<ItemId>, usize)> {
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    let mut skipped = 0;
    for id in candidates(s, p)? {
        let Some((item, media, _)) = s.project.resolve_media(id) else {
            skipped += 1;
            continue;
        };
        if !seen.insert(item) {
            continue;
        }
        let skip = media.offline || media.info.kind == filmcraft_media::MediaKind::ImageSequence || s.opening_items.contains(&item)
            || s.media.cached_source(&s.project, item).is_some();
        let exists = !skip && match &media.media {
            filmcraft_project::MediaRef::File { path } => s.media.follow(path).ok().is_some_and(|path| s.services.file_size(&path).is_ok()),
            _ => false,
        };
        if exists {
            items.push(item);
        } else {
            skipped += 1;
        }
    }
    Ok((items, skipped))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn start(s: &mut Session, p: &Value) -> Result<Value> {
    poll(s);
    let (items, skipped) = work(s, p)?;
    if items.is_empty() {
        return Ok(json!({"job": null, "pending": false, "files": 0, "skipped": skipped}));
    }
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0).checked_add(1)
        .ok_or_else(|| crate::EngineError::Other("job ids exhausted".into()))?;
    let job = crate::Job { id, label: format!("Opening {} media files", items.len()), progress: Default::default(), result: Default::default() };
    job.progress.total.store(items.len() as u64, Ordering::Relaxed);
    let current = Current::default();
    let worker = Worker {
        project: s.project.clone(), media: s.media.clone(), services: s.services.clone(),
        progress: job.progress.clone(), current: current.clone(),
    };
    let limit = crate::import_jobs::open_limit(p, std::env::var("FILMCRAFT_OPEN_THREADS").ok().as_deref());
    let files = items.len();
    let queued = items.clone();
    let result = job.result.clone();
    let run = move || {
        let began = web_time::Instant::now();
        worker.run(&queued, limit);
        let seconds = began.elapsed().as_secs_f64();
        let frames = worker.progress.done.load(Ordering::Relaxed);
        let error = worker.progress.error.lock().unwrap_or_else(PoisonError::into_inner).clone();
        *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(match error {
            Some(error) => Err(error),
            None => Ok(filmcraft_export::Report {
                path: String::new(), frames, seconds, bytes: 0, render_fps: frames as f64 / seconds.max(1e-6), extra_files: Vec::new(),
            }),
        });
        worker.progress.finished.store(true, Ordering::Release);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    std::thread::Builder::new().name("filmcraft-media-open".into()).spawn(run).map_err(|e| crate::EngineError::Other(e.to_string()))?;
    for item in &items {
        s.opening_items.insert(*item);
        s.opening_times.insert(*item, web_time::Instant::now());
    }
    s.open_workers.push(PendingOpen { job: id, items, current });
    s.jobs.push(job);
    Ok(json!({"job": id, "pending": true, "files": files, "skipped": skipped}))
}

#[cfg(not(target_arch = "wasm32"))]
struct Worker {
    project: Arc<filmcraft_project::Project>,
    media: Arc<crate::media_pool::MediaPool>,
    services: Arc<dyn crate::Services>,
    progress: Arc<filmcraft_export::Progress>,
    current: Current,
}

#[cfg(not(target_arch = "wasm32"))]
impl Worker {
    fn run(&self, items: &[ItemId], limit: usize) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..limit.min(items.len()) {
                if std::thread::Builder::new().name("filmcraft-open".into()).spawn_scoped(scope, || self.open_next(items, &next)).is_err() {
                    self.open_next(items, &next);
                    break;
                }
            }
        });
        *self.progress.status.lock().unwrap_or_else(PoisonError::into_inner) = format!(
            "{} {} media files", if self.progress.cancel.load(Ordering::Relaxed) { "Stopped after" } else { "Opened" },
            self.progress.done.load(Ordering::Relaxed),
        );
    }

    fn open_next(&self, items: &[ItemId], next: &std::sync::atomic::AtomicUsize) {
        while !self.progress.cancel.load(Ordering::Relaxed) {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(item) = items.get(index) else { break };
            let name = self.project.item(*item).map(|i| i.name.clone()).unwrap_or_default();
            {
                let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
                current.push((index, name.clone(), web_time::Instant::now()));
                self.status(&current);
            }
            let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.media.source_for(&self.project, *item, &*self.services)));
            if opened.is_err() || self.media.offline_status(*item).is_some() {
                // Failed opens must be eligible for a later request after its cooldown.
                self.media.remove(*item);
                *self.progress.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(format!("{name}: media could not be opened"));
            }
            let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
            current.retain(|(i, _, _)| *i != index);
            self.status(&current);
            self.progress.done.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn status(&self, current: &[(usize, String, web_time::Instant)]) {
        if let Some(status) = crate::import_jobs::current_status(current) {
            *self.progress.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn failed_request_cooldown_has_an_exact_boundary() {
        let now = web_time::Instant::now();
        assert!(super::retry_ready(None, now));
        assert!(!super::retry_ready(Some(&now), now + std::time::Duration::from_millis(4999)));
        assert!(super::retry_ready(Some(&now), now + std::time::Duration::from_secs(5)));
    }
}
