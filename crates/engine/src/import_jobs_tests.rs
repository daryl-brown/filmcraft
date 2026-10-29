//! Background imports against the same fake filesystem as the browser tests.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{Arc, Condvar, Mutex};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};

    use filmcraft_project::{Bin, BinEntry, ItemId, MediaRef};
    use serde_json::{Value, json};

    use crate::media_browser_tests::{FakeFs, folder_fixture};
    use crate::{Services, Session};

    struct GateFs {
        fs: FakeFs,
        gate: Mutex<Option<Receiver<()>>>,
        entered: Sender<()>,
        path: String,
    }

    impl GateFs {
        fn block(&self, path: &str) -> std::io::Result<()> {
            if path == self.path {
                if let Some(gate) = self.gate.lock().unwrap().take() {
                    self.entered.send(()).unwrap();
                    gate.recv_timeout(Duration::from_secs(30)).map_err(|e| std::io::Error::new(std::io::ErrorKind::TimedOut, e))?;
                }
            }
            Ok(())
        }
    }

    impl Services for GateFs {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            self.block(path)?;
            self.fs.read_file(path)
        }

        fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
            self.fs.write_file(path, bytes)
        }

        fn file_size(&self, path: &str) -> std::io::Result<u64> {
            self.fs.file_size(path)
        }

        fn list_entries(&self, path: &str) -> Option<std::io::Result<Vec<crate::media_browser::DirEntry>>> {
            if let Err(e) = self.block(path) {
                return Some(Err(e));
            }
            self.fs.list_entries(path)
        }
    }

    struct Gate {
        release: Sender<()>,
        entered: Receiver<()>,
    }

    impl Drop for Gate {
        fn drop(&mut self) {
            let _ = self.release.send(());
        }
    }

    fn gated() -> (Session, Gate) {
        gated_path("/outside.wav")
    }

    fn gated_path(path: &str) -> (Session, Gate) {
        let (release, receive) = mpsc::channel();
        let (entered, ready) = mpsc::channel();
        let fs = GateFs { fs: folder_fixture(), gate: Mutex::new(Some(receive)), entered, path: path.into() };
        (Session::new(Arc::new(fs)), Gate { release, entered: ready })
    }

    fn wait(s: &mut Session, job: u64) -> Value {
        let value = s.execute("file.importStatus", json!({"job": job, "wait": true, "timeoutMs": 30_000})).unwrap();
        assert_eq!(value["pending"], 0, "{value}");
        assert_eq!(value["jobs"][0]["finished"], true);
        value
    }

    #[derive(Default)]
    struct OpenState {
        active: usize,
        maximum: usize,
        entered: usize,
        released: bool,
        opens: std::collections::BTreeMap<String, usize>,
    }

    struct PoolFs {
        fs: FakeFs,
        state: Mutex<OpenState>,
        changed: Condvar,
        threshold: usize,
    }

    impl PoolFs {
        fn release(&self) {
            self.state.lock().unwrap().released = true;
            self.changed.notify_all();
        }

        fn wait_for(&self, count: usize) {
            let state = self.state.lock().unwrap();
            let (_state, timeout) = self.changed.wait_timeout_while(state, Duration::from_secs(30), |s| s.entered < count).unwrap();
            assert!(!timeout.timed_out(), "opens did not reach the gate");
        }
    }

    impl Services for PoolFs {
        fn read_range(&self, path: &str, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
            let bytes = self.fs.read_file(path)?;
            let start = usize::try_from(offset).unwrap().min(bytes.len());
            Ok(bytes[start..start.saturating_add(len).min(bytes.len())].to_vec())
        }

        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            let mut state = self.state.lock().unwrap();
            state.active += 1;
            state.entered += 1;
            *state.opens.entry(path.to_string()).or_default() += 1;
            state.maximum = state.maximum.max(state.active);
            if self.threshold > 0 && state.active >= self.threshold {
                state.released = true;
            }
            self.changed.notify_all();
            let (mut state, timeout) = self.changed.wait_timeout_while(state, Duration::from_secs(30), |s| !s.released).unwrap();
            state.active -= 1;
            drop(state);
            assert!(!timeout.timed_out(), "concurrent opens did not reach the gate");
            assert!(!path.ends_with("panic.wav"), "synthetic open panic");
            self.fs.read_file(path)
        }

        fn file_size(&self, path: &str) -> std::io::Result<u64> {
            self.fs.file_size(path)
        }

        fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
            self.fs.write_file(path, bytes)
        }

        fn list_entries(&self, path: &str) -> Option<std::io::Result<Vec<crate::media_browser::DirEntry>>> {
            self.fs.list_entries(path)
        }
    }

    fn pool_fixture(count: usize, threshold: usize) -> (Session, Arc<PoolFs>, Vec<String>) {
        let fs = folder_fixture();
        let paths: Vec<String> = (0..count).map(|n| format!("/file{n}.wav")).collect();
        {
            let mut files = fs.files.lock().unwrap();
            let wav = files.get("/outside.wav").unwrap().clone();
            for path in &paths {
                files.insert(path.clone(), wav.clone());
            }
        }
        let fs = Arc::new(PoolFs { fs, state: Mutex::new(OpenState::default()), changed: Condvar::new(), threshold });
        (Session::new(fs.clone()), fs, paths)
    }

    fn missing_pool(count: usize) -> (Session, Arc<PoolFs>, Vec<String>, Vec<ItemId>) {
        let (mut s, fs, paths) = pool_fixture(count, 0);
        fs.release();
        let imported = s.execute("file.import", json!({"paths": paths})).unwrap();
        let items: Vec<ItemId> = imported["items"].as_array().unwrap().iter().map(|id| ItemId(id.as_u64().unwrap())).collect();
        s.edit("Move media", |project, _| {
            for (item, path) in items.iter().zip(&paths) {
                project.item_mut(*item).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: format!("/missing{path}") };
            }
            Ok(())
        }).unwrap();
        crate::relink::refresh(&mut s);
        *fs.state.lock().unwrap() = OpenState::default();
        (s, fs, paths, items)
    }

    #[test]
    fn background_relink_defers_commit_matches_sync_and_opens_once() {
        for threads in [1, 3] {
            let (mut s, fs, paths, items) = missing_pool(6);
            let (mut sync, sync_fs, _, _) = missing_pool(6);
            sync_fs.release();
            let params = json!({"item": items[0].0, "path": paths[0]});
            let expected = sync.execute("media.relink", params.clone()).unwrap();
            assert!(expected.get("pending").is_none());
            let mut params = params;
            params["background"] = json!(true);
            params["openThreads"] = json!(threads);
            let began = Instant::now();
            let out = s.execute("media.relink", params).unwrap();
            assert!(began.elapsed() < Duration::from_secs(5));
            assert_eq!(out["pending"], true);
            assert_eq!(out["files"], 6);
            fs.wait_for(threads);
            assert_eq!(s.offline.missing, items);
            for (item, path) in items.iter().zip(&paths) {
                assert_eq!(s.project.item(*item).unwrap().as_media().unwrap().media, MediaRef::File { path: format!("/missing{path}") });
            }
            fs.release();
            let done = wait(&mut s, out["job"].as_u64().unwrap());
            assert_eq!(done["commands"][0]["command"], "media.relink");
            assert_eq!(done["commands"][0]["result"]["Ok"], expected);
            assert!(done["outcomes"].as_array().unwrap().is_empty());
            assert!(s.offline.missing.is_empty());
            let state = fs.state.lock().unwrap();
            assert_eq!(state.entered, paths.len());
            assert_eq!(state.opens.len(), paths.len());
            assert!(state.opens.values().all(|count| *count == 1));
            assert_eq!(state.maximum, threads);
        }
    }

    #[test]
    fn cancelled_relink_never_commits_and_stops_the_queue() {
        let (mut s, fs, paths, items) = missing_pool(6);
        let out = s.execute("media.relink", json!({"item": items[0].0, "path": paths[0], "background": true, "openThreads": 2})).unwrap();
        let job = out["job"].as_u64().unwrap();
        fs.wait_for(2);
        s.execute("file.importStatus", json!({"job": job, "cancel": true})).unwrap();
        fs.release();
        let done = wait(&mut s, job);
        assert_eq!(done["commands"][0]["result"]["Err"], "cancelled");
        assert_eq!(s.offline.missing, items);
        assert_eq!(fs.state.lock().unwrap().entered, 2);
    }

    #[test]
    fn background_prefix_relink_matches_sync_and_waits_without_selected_job() {
        let (mut s, fs, _, _) = missing_pool(4);
        let (mut sync, sync_fs, _, _) = missing_pool(4);
        sync_fs.release();
        let expected = sync.execute("media.autoRelink", json!({"from": "/missing", "to": ""})).unwrap();
        assert!(expected.get("job").is_none());
        let out = s.execute("media.autoRelink", json!({"from": "/missing", "to": "", "background": true, "openThreads": 2})).unwrap();
        assert_eq!(out["files"], 4);
        fs.wait_for(2);
        assert_eq!(s.offline.missing.len(), 4);
        fs.release();
        let done = s.execute("file.importStatus", json!({"wait": true, "timeoutMs": 30_000})).unwrap();
        assert_eq!(done["pending"], 0);
        assert_eq!(done["commands"][0]["result"]["Ok"], expected);
        assert_eq!(fs.state.lock().unwrap().entered, 4);
    }

    #[test]
    fn deferred_relink_revalidates_item_after_project_edit() {
        let (mut s, fs, paths, items) = missing_pool(2);
        let params = json!({"item": items[0].0, "path": paths[0], "background": true, "openThreads": 1});
        let out = s.execute("media.relink", params).unwrap();
        fs.wait_for(1);
        s.edit("Delete media", |project, _| {
            project.items.remove(&items[0]);
            project.root.remove_item(items[0]);
            Ok(())
        }).unwrap();
        let expected = s.execute("media.relink", json!({"item": items[0].0, "path": paths[0]})).unwrap_err().to_string();
        fs.release();
        let done = wait(&mut s, out["job"].as_u64().unwrap());
        assert_eq!(done["commands"][0]["result"]["Err"], expected);
    }

    fn folder_relink_pool(root: &std::path::Path) -> (Session, Arc<PoolFs>) {
        let (s, fs, paths, _) = missing_pool(3);
        let mut files = fs.fs.files.lock().unwrap();
        for (index, path) in paths.iter().enumerate() {
            let mut bytes = files.remove(path).unwrap();
            if index == 2 {
                bytes.push(0);
            }
            files.insert(root.join(format!("file{index}.wav")).to_string_lossy().into_owned(), bytes);
        }
        drop(files);
        (s, fs)
    }

    #[test]
    fn background_folder_relink_matches_sync_and_skips_size_mismatch() {
        let root = crate::media_test_util::tmp_dir("background-folder-relink");
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..3 {
            std::fs::write(root.join(format!("file{index}.wav")), []).unwrap();
        }
        let (mut s, fs) = folder_relink_pool(&root);
        let (mut sync, sync_fs) = folder_relink_pool(&root);
        sync_fs.release();
        let expected = sync.execute("media.autoRelink", json!({"folder": root.to_string_lossy()})).unwrap();
        assert!(expected.get("job").is_none());
        assert_eq!(expected["relinked"].as_array().unwrap().len(), 2);
        assert_eq!(expected["failed"].as_array().unwrap().len(), 1);
        let out = s.execute("media.autoRelink", json!({"folder": root.to_string_lossy(), "background": true, "openThreads": 2})).unwrap();
        assert_eq!(out["files"], 2);
        fs.wait_for(2);
        assert_eq!(s.offline.missing.len(), 3);
        fs.release();
        let done = wait(&mut s, out["job"].as_u64().unwrap());
        assert_eq!(done["commands"][0]["result"]["Ok"], expected);
        assert_eq!(fs.state.lock().unwrap().entered, 2);
        assert_eq!(s.offline.missing.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stashed_candidates_and_empty_remaps_commit_immediately() {
        let (mut s, fs, paths, items) = missing_pool(2);
        fs.release();
        for path in &paths {
            let source = s.media.open_file(path, &*fs).unwrap();
            s.media.stash_source(path, source);
        }
        let out = s.execute("media.relink", json!({"item": items[0].0, "path": paths[0], "background": true})).unwrap();
        assert!(out.get("job").is_none());
        assert_eq!(out["relinked"].as_array().unwrap().len(), 2);
        assert!(s.jobs.is_empty());
        let out = s.execute("media.autoRelink", json!({"from": "/unused", "to": "/elsewhere", "background": true})).unwrap();
        assert!(out.get("pending").is_none());
        assert_eq!(out["relinked"], json!([]));
    }

    #[test]
    fn selected_command_outcomes_leave_other_consumers_results_available() {
        let mut s = Session::default();
        for job in [1, 2, 3] {
            s.command_results.push(crate::import_jobs::CommandOutcome {
                job, command: "media.relink".into(), result: Ok(json!({"relinked": []})),
            });
        }
        let selected = s.execute("file.importStatus", json!({"job": 2})).unwrap();
        assert_eq!(selected["commands"].as_array().unwrap().len(), 1);
        assert_eq!(selected["commands"][0]["job"], 2);
        assert_eq!(s.take_command_outcome(1).unwrap().job, 1);
        assert_eq!(s.take_command_outcomes().iter().map(|outcome| outcome.job).collect::<Vec<_>>(), vec![3]);
        assert!(s.take_command_outcomes().is_empty());
    }

    #[test]
    fn background_relink_validates_parameters_before_starting_a_job() {
        let mut s = Session::default();
        for (command, params) in [
            ("media.relink", json!({})),
            ("media.relink", json!({"item": 1})),
            ("media.relink", json!({"item": 1, "path": "/candidate.wav"})),
            ("media.autoRelink", json!({"from": "/old"})),
        ] {
            let expected = s.execute(command, params.clone()).unwrap_err().to_string();
            let mut params = params;
            params["background"] = json!(true);
            assert_eq!(s.execute(command, params).unwrap_err().to_string(), expected);
            assert!(s.jobs.is_empty());
        }
    }

    #[test]
    fn background_opens_are_concurrent_and_bounded() {
        for (count, threads, threshold) in [(4, 4, 2), (4, 1, 1), (8, 3, 3)] {
            let (mut s, fs, paths) = pool_fixture(count, threshold);
            let out = s.execute("file.import", json!({"paths": paths, "background": true, "openThreads": threads})).unwrap();
            let completed = wait(&mut s, out["job"].as_u64().unwrap());
            assert_eq!(completed["outcomes"][0]["items"].as_array().unwrap().len(), count);
            assert_eq!(completed["outcomes"][0]["errors"], json!([]));
            let maximum = fs.state.lock().unwrap().maximum;
            assert!(maximum >= threshold && maximum <= threads);
            if threads == 1 {
                assert_eq!(maximum, 1);
            }
        }
    }

    #[test]
    fn concurrent_cancel_keeps_only_active_files_and_reports_status() {
        let (mut s, fs, paths) = pool_fixture(8, 0);
        let out = s.execute("file.import", json!({"paths": paths, "background": true, "openThreads": 2})).unwrap();
        let job = out["job"].as_u64().unwrap();
        fs.wait_for(2);
        let pending = s.execute("file.importStatus", json!({"job": job, "cancel": true})).unwrap();
        assert!(pending["jobs"][0]["status"].as_str().unwrap().contains("2 files in progress"));
        fs.release();
        let completed = wait(&mut s, job);
        assert_eq!(completed["outcomes"][0]["items"].as_array().unwrap().len(), 2);
        assert_eq!(completed["outcomes"][0]["errors"], json!([]));
        assert_eq!(completed["jobs"][0]["done"], 2);
        // Only the two files that were being opened were opened (the commit may read them again).
        assert_eq!(fs.state.lock().unwrap().opens.len(), 2);
    }

    fn ordered_items(s: &Session, outcome: &Value) -> Vec<Value> {
        outcome["items"].as_array().unwrap().iter().map(|id| {
            let id = ItemId(id.as_u64().unwrap());
            let item = s.project.item(id).unwrap();
            let bin = s.project.root.parent_of(id).and_then(|bin| s.project.root.find_bin(bin)).map(|bin| &bin.name);
            json!({"name": item.name, "media": item.as_media().unwrap().media, "bin": bin})
        }).collect()
    }

    struct OrderedFs {
        fs: FakeFs,
        gates: Mutex<std::collections::BTreeMap<String, Receiver<()>>>,
        entered: Sender<String>,
    }

    impl Services for OrderedFs {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            let gate = self.gates.lock().unwrap().remove(path);
            if let Some(gate) = gate {
                self.entered.send(path.to_string()).unwrap();
                gate.recv_timeout(Duration::from_secs(30)).unwrap();
            }
            self.fs.read_file(path)
        }

        fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
            self.fs.write_file(path, bytes)
        }

        fn list_entries(&self, path: &str) -> Option<std::io::Result<Vec<crate::media_browser::DirEntry>>> {
            self.fs.list_entries(path)
        }
    }

    fn six_files() -> (FakeFs, Vec<String>) {
        let mut fs = FakeFs::default();
        fs.dirs = ["/six", "/six/A", "/six/B", "/empty"].map(String::from).to_vec();
        let paths: Vec<String> = ["/six/0.wav", "/six/1.wav", "/six/A/2.wav", "/six/A/3.wav", "/six/B/4.wav", "/six/B/5.wav"]
            .map(String::from).to_vec();
        let wav = filmcraft_media::wav::write_wav16(&[0.0; 4800], 2, 48_000);
        for path in &paths {
            fs.files.get_mut().unwrap().insert(path.clone(), wav.clone());
        }
        (fs, paths)
    }

    fn wait_done(s: &Session, done: u64) {
        let began = Instant::now();
        while s.jobs[0].progress.done.load(std::sync::atomic::Ordering::Relaxed) < done {
            assert!(began.elapsed() < Duration::from_secs(30));
            std::thread::yield_now();
        }
    }

    #[test]
    fn concurrent_reversed_opens_keep_sequential_items_bins_and_errors() {
        let (fs, paths) = six_files();
        let mut gates = std::collections::BTreeMap::new();
        let mut release = std::collections::BTreeMap::new();
        for path in &paths {
            let (send, receive) = mpsc::channel();
            gates.insert(path.clone(), receive);
            release.insert(path.clone(), send);
        }
        let (entered, ready) = mpsc::channel();
        let fs = OrderedFs { fs, gates: Mutex::new(gates), entered };
        let mut background = Session::new(Arc::new(fs));
        let out = background.execute("file.import", json!({"paths": ["/six", "/empty"], "background": true, "openThreads": 4})).unwrap();
        let mut first = Vec::new();
        for _ in 0..4 {
            first.push(ready.recv_timeout(Duration::from_secs(30)).unwrap());
        }
        first.sort();
        assert_eq!(first, paths[..4]);
        // Keep the earliest three files blocked while later files finish first.
        for (done, index) in [3, 4, 5, 2, 1, 0].into_iter().enumerate() {
            release.get(&paths[index]).unwrap().send(()).unwrap();
            wait_done(&background, done as u64 + 1);
            if done < 2 {
                assert_eq!(ready.recv_timeout(Duration::from_secs(30)).unwrap(), paths[done + 4]);
            }
        }
        let completed = wait(&mut background, out["job"].as_u64().unwrap());
        let (fs, _) = six_files();
        let mut sync = Session::new(Arc::new(fs));
        let a = sync.execute("file.import", json!({"paths": ["/six", "/empty"]})).unwrap();
        let b = &completed["outcomes"][0];
        assert_eq!(ordered_items(&sync, &a), ordered_items(&background, b));
        assert_eq!(tree(&sync, &sync.project.root), tree(&background, &background.project.root));
        assert_eq!(a["errors"], b["errors"]);
        assert_eq!(a["skipped"], b["skipped"]);
        assert_eq!(a["bins"].as_array().unwrap().len(), b["bins"].as_array().unwrap().len());
    }

    #[test]
    fn concurrent_errors_and_panics_do_not_lose_good_files() {
        let (mut s, fs, paths) = pool_fixture(4, 1);
        let out = s.execute("file.import", json!({
            "paths": [paths[0], "/missing.wav", paths[1], "/panic.wav", paths[2], paths[3]],
            "background": true, "openThreads": 4,
        })).unwrap();
        let completed = wait(&mut s, out["job"].as_u64().unwrap());
        let outcome = &completed["outcomes"][0];
        assert_eq!(outcome["items"].as_array().unwrap().len(), 4);
        let names: Vec<Value> = ordered_items(&s, outcome).iter().map(|item| item["name"].clone()).collect();
        assert_eq!(names, [json!("file0.wav"), json!("file1.wav"), json!("file2.wav"), json!("file3.wav")]);
        assert_eq!(outcome["errors"].as_array().unwrap().len(), 2);
        assert!(outcome["errors"][0].as_str().unwrap().starts_with("/missing.wav:"));
        assert_eq!(outcome["errors"][1], "/panic.wav: the file could not be opened (internal error)");
        assert_eq!(completed["jobs"][0]["done"], 6);
        assert!(fs.state.lock().unwrap().maximum <= 4);
    }

    #[test]
    fn browser_background_folder_returns_before_walk_and_matches_sync() {
        let (mut background, gate) = gated_path("/root");
        let began = Instant::now();
        let out = background.execute("mediaBrowser.import", json!({"paths": ["/root"], "background": true, "openThreads": 2})).unwrap();
        assert!(began.elapsed() < Duration::from_secs(5));
        assert_eq!(out["pending"], true);
        assert_eq!(out["items"], json!([]));
        assert_eq!(out["errors"], json!([]));
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(background.project.root.children.is_empty());
        gate.release.send(()).unwrap();
        let completed = wait(&mut background, out["job"].as_u64().unwrap());
        let b = &completed["outcomes"][0];
        let mut sync = Session::new(Arc::new(folder_fixture()));
        let a = sync.execute("mediaBrowser.import", json!({"paths": ["/root"]})).unwrap();
        assert!(a.get("job").is_none());
        assert_eq!(ordered_items(&sync, &a), ordered_items(&background, b));
        assert_eq!(tree(&sync, &sync.project.root), tree(&background, &background.project.root));
        assert_eq!(a["bins"].as_array().unwrap().len(), b["bins"].as_array().unwrap().len());
        assert_eq!(a["skipped"], b["skipped"]);
        assert_eq!(a["errors"], b["errors"]);
    }

    #[test]
    fn background_import_returns_before_open_and_commits_with_undo() {
        let (mut s, gate) = gated();
        let began = Instant::now();
        let out = s.execute("file.import", json!({"paths": ["/outside.wav"], "background": true})).unwrap();
        assert!(began.elapsed() < Duration::from_secs(5));
        let job = out["job"].as_u64().unwrap();
        assert_eq!(out["pending"], true);
        assert_eq!(out["items"], json!([]));
        assert!(s.project.items.is_empty());
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let pending = s.execute("file.importStatus", json!({"job": job})).unwrap();
        assert_eq!(pending["pending"], 1);
        assert_eq!(pending["jobs"][0]["finished"], false);
        gate.release.send(()).unwrap();
        let completed = wait(&mut s, job);
        let id = ItemId(completed["outcomes"][0]["items"][0].as_u64().unwrap());
        assert!(matches!(&s.project.item(id).unwrap().as_media().unwrap().media, MediaRef::File { path } if path == "/outside.wav"));
        assert!(s.take_import_outcomes().is_empty());
        assert_eq!(s.execute("file.importStatus", json!({"job": job})).unwrap()["outcomes"], json!([]));
        assert!(s.undo().is_some());
        assert!(s.project.item(id).is_none());
    }

    #[test]
    fn background_folder_walk_returns_before_listing() {
        let (mut s, gate) = gated_path("/root");
        let began = Instant::now();
        let out = s.execute("file.import", json!({"paths": ["/root"], "background": true})).unwrap();
        assert!(began.elapsed() < Duration::from_secs(5));
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(s.project.root.children.is_empty());
        gate.release.send(()).unwrap();
        let completed = wait(&mut s, out["job"].as_u64().unwrap());
        assert_eq!(completed["outcomes"][0]["items"].as_array().unwrap().len(), 3);
        assert_eq!(completed["outcomes"][0]["skipped"], 3);
    }

    #[test]
    fn background_wait_timeout_keeps_the_job_pending() {
        let (mut s, gate) = gated();
        let out = s.execute("file.import", json!({"paths": ["/outside.wav"], "background": true})).unwrap();
        let job = out["job"].as_u64().unwrap();
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let pending = s.execute("file.importStatus", json!({"job": job, "wait": true, "timeoutMs": 0})).unwrap();
        assert_eq!(pending["pending"], 1);
        assert_eq!(pending["outcomes"], json!([]));
        gate.release.send(()).unwrap();
        wait(&mut s, job);
    }

    #[test]
    fn background_replace_clears_the_project_before_opening() {
        let (mut s, gate) = gated();
        s.execute("file.openDemoProject", json!({})).unwrap();
        assert!(!s.project.items.is_empty());
        let out = s.execute("file.import", json!({"paths": ["/outside.wav"], "background": true, "replace": true})).unwrap();
        assert!(s.project.items.is_empty());
        gate.release.send(()).unwrap();
        wait(&mut s, out["job"].as_u64().unwrap());
        assert_eq!(s.project.items.len(), 1);
    }

    #[test]
    fn background_image_sequences_remain_immediate() {
        for explicit in [false, true] {
            let fs = folder_fixture();
            {
                let mut files = fs.files.lock().unwrap();
                let png = files.get("/root/B/b1.png").unwrap().clone();
                files.insert("/root/B/b2.png".into(), png);
            }
            let mut s = Session::new(Arc::new(fs));
            s.prefs.media.import_image_sequences = true;
            let out = s.execute("file.import", json!({"paths": ["/root/B/b1.png"], "imageSequence": explicit, "background": true})).unwrap();
            assert!(out.get("job").is_none());
            assert_eq!(out["items"].as_array().unwrap().len(), 1);
            assert_eq!(out["imageSequences"][0]["frames"], 2);
            assert!(s.import_jobs.is_empty());
        }
    }

    fn tree(s: &Session, bin: &Bin) -> Value {
        let mut children: Vec<Value> = bin.children.iter().map(|entry| match entry {
            BinEntry::Bin(child) => tree(s, child),
            BinEntry::Item(id) => {
                let item = s.project.item(*id).unwrap();
                json!({"item": item.name, "media": item.as_media().unwrap().media})
            }
        }).collect();
        children.sort_by_key(Value::to_string);
        json!({"bin": bin.name, "children": children})
    }

    #[test]
    fn background_folder_matches_sync_tree_and_carries_skipped() {
        let mut sync = Session::new(Arc::new(folder_fixture()));
        let mut background = Session::new(Arc::new(folder_fixture()));
        let a = sync.execute("file.import", json!({"paths": ["/root"]})).unwrap();
        let b = background.execute("file.import", json!({"paths": ["/root"], "background": true, "wait": true})).unwrap();
        assert_eq!(tree(&sync, &sync.project.root), tree(&background, &background.project.root));
        assert_eq!(a["items"].as_array().unwrap().len(), b["items"].as_array().unwrap().len());
        assert_eq!(a["bins"].as_array().unwrap().len(), b["bins"].as_array().unwrap().len());
        assert_eq!(a["errors"], b["errors"]);
        assert_eq!(a["skipped"], b["skipped"]);
        assert_eq!(b["pending"], false);
        assert!(background.take_import_outcomes().is_empty());
        background.undo().unwrap();
        assert!(background.project.items.is_empty());
        assert!(background.project.root.children.is_empty());
    }

    #[test]
    fn mixed_document_is_immediate_while_media_waits() {
        let (mut s, gate) = gated();
        let mut source = Session::default();
        source.execute("file.openDemoProject", json!({})).unwrap();
        let (xml, _) = filmcraft_interchange::export(
            &source.project, source.state.active_sequence.unwrap(), filmcraft_interchange::Format::Fcp7Xml,
            &filmcraft_interchange::ExportOptions::default(),
        ).unwrap();
        s.services.write_file("/cut.xml", &xml).unwrap();
        let out = s.execute("file.import", json!({"paths": ["/cut.xml", "/outside.wav"], "background": true})).unwrap();
        assert_eq!(out["sequences"].as_array().unwrap().len(), 1);
        assert_eq!(out["documents"].as_array().unwrap().len(), 1);
        assert_eq!(out["pending"], true);
        assert!(s.project.items.values().all(|item| item.name != "outside.wav"));
        gate.release.send(()).unwrap();
        let completed = wait(&mut s, out["job"].as_u64().unwrap());
        assert_eq!(completed["outcomes"][0]["items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn background_error_keeps_good_file_and_reports_error() {
        let mut s = Session::new(Arc::new(folder_fixture()));
        let out = s.execute("file.import", json!({"paths": ["/missing.wav", "/outside.wav"], "background": true})).unwrap();
        let completed = wait(&mut s, out["job"].as_u64().unwrap());
        let outcome = &completed["outcomes"][0];
        assert_eq!(outcome["items"].as_array().unwrap().len(), 1);
        assert_eq!(outcome["errors"].as_array().unwrap().len(), 1);
        assert!(outcome["errors"][0].as_str().unwrap().contains("/missing.wav"));
    }

    #[test]
    fn cancel_during_open_commits_only_the_opened_file() {
        let (mut s, gate) = gated();
        let out = s.execute("file.import", json!({
            "paths": ["/outside.wav", "/root/A/a1.wav", "/root/A/sub/s1.wav"], "background": true, "openThreads": 1,
        })).unwrap();
        let job = out["job"].as_u64().unwrap();
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        s.execute("file.importStatus", json!({"job": job, "cancel": true})).unwrap();
        gate.release.send(()).unwrap();
        let completed = wait(&mut s, job);
        assert_eq!(completed["outcomes"][0]["items"].as_array().unwrap().len(), 1);
        assert_eq!(s.project.items.len(), 1);
        assert_eq!(s.project.items.values().next().unwrap().name, "outside.wav");
        assert!(completed["jobs"][0]["status"].as_str().unwrap().contains("1 already opened"));
    }

    #[test]
    fn deleted_destination_bin_falls_back_to_root() {
        let (mut s, gate) = gated();
        let bin = s.edit("New Bin", |pr, _| Ok(pr.add_bin("Target", None))).unwrap();
        let out = s.execute("file.import", json!({"paths": ["/outside.wav"], "bin": bin.0, "background": true})).unwrap();
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        s.edit("Delete Bin", |pr, _| { pr.root.remove_bin(bin); Ok(()) }).unwrap();
        gate.release.send(()).unwrap();
        let completed = wait(&mut s, out["job"].as_u64().unwrap());
        let id = ItemId(completed["outcomes"][0]["items"][0].as_u64().unwrap());
        assert_eq!(s.project.root.parent_of(id), Some(s.project.root.id));
    }

    #[test]
    fn sync_import_retains_old_result_shape() {
        let mut s = Session::new(Arc::new(folder_fixture()));
        let out = s.execute("file.import", json!({"paths": ["/outside.wav"]})).unwrap();
        assert!(out.get("job").is_none());
        assert!(out.get("pending").is_none());
        assert_eq!(out["items"].as_array().unwrap().len(), 1);
        assert_eq!(out["errors"], json!([]));
    }

    #[test]
    fn import_status_wait_without_id_commits_all_import_jobs() {
        let mut s = Session::new(Arc::new(folder_fixture()));
        for path in ["/outside.wav", "/root/A/a1.wav"] {
            s.execute("file.import", json!({"paths": [path], "background": true})).unwrap();
        }
        let out = s.execute("file.importStatus", json!({"wait": true, "timeoutMs": 30_000})).unwrap();
        assert_eq!(out["pending"], 0);
        assert_eq!(out["jobs"].as_array().unwrap().len(), 2);
        assert_eq!(out["outcomes"].as_array().unwrap().len(), 2);
        assert_eq!(s.project.items.len(), 2);
        assert!(s.take_import_outcomes().is_empty());
    }

    #[test]
    fn import_commands_handle_hostile_optional_parameters() {
        let mut s = Session::new(Arc::new(folder_fixture()));
        for p in [Value::Null, json!([]), json!({"job": -1, "cancel": [], "wait": "yes", "timeoutMs": -1}),
            json!({"job": {}, "cancel": true, "wait": true, "timeoutMs": u64::MAX})]
        {
            let out = s.execute("file.importStatus", p).unwrap();
            assert_eq!(out["pending"], 0);
            assert_eq!(out["jobs"], json!([]));
            assert_eq!(out["outcomes"], json!([]));
        }
        assert!(s.execute("file.import", json!({"paths": [null, false, 12], "background": true})).is_err());
        let out = s.execute("file.import", json!({"paths": ["/outside.wav"], "background": "yes"})).unwrap();
        assert!(out.get("job").is_none());
        assert_eq!(s.project.items.len(), 1);
    }

    #[test]
    fn ui_outcomes_drain_once_and_selected_status_leaves_other_outcomes() {
        let mut s = Session::new(Arc::new(folder_fixture()));
        let a = s.execute("file.import", json!({"paths": ["/outside.wav"], "background": true})).unwrap()["job"].as_u64().unwrap();
        let b = s.execute("file.import", json!({"paths": ["/root/A/a1.wav"], "background": true})).unwrap()["job"].as_u64().unwrap();
        let completed = wait(&mut s, a);
        assert_eq!(completed["outcomes"].as_array().unwrap().len(), 1);
        // Wait for the second worker without draining its outcome through the command.
        let job = s.jobs.iter().find(|job| job.id == b).unwrap();
        let began = Instant::now();
        while !job.progress.finished.load(std::sync::atomic::Ordering::Acquire) {
            assert!(began.elapsed() < Duration::from_secs(30));
            std::thread::yield_now();
        }
        crate::import_jobs::poll(&mut s);
        let selected = s.execute("file.importStatus", json!({"job": a})).unwrap();
        assert_eq!(selected["outcomes"], json!([]));
        let outcomes = s.take_import_outcomes();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].job, b);
        assert!(s.take_import_outcomes().is_empty());
        assert_eq!(s.execute("file.importStatus", json!({})).unwrap()["outcomes"], json!([]));
    }

    // Construct saved media metadata without touching the gated filesystem.
    fn saved_items(s: &mut Session, paths: &[String], used: usize) -> Vec<ItemId> {
        use filmcraft_project::{ItemKind, Label, MediaClip};
        let wav = filmcraft_media::wav::write_wav16(&[0.0; 48_000], 2, 48_000);
        let source = crate::MediaPool::default().open_bytes("clip.wav", wav.into()).unwrap();
        let p = Arc::make_mut(&mut s.project);
        let ids: Vec<_> = paths.iter().map(|path| p.add_item(path, Label::Iris, ItemKind::Media(Box::new(MediaClip {
            media: MediaRef::File { path: path.clone() }, info: source.info().clone(), interpret: Default::default(),
            mark_in: None, mark_out: None, markers: Vec::new(), offline: false, proxy: None, identity: None,
        })), None)).collect();
        s.execute("file.newSequence", json!({"width": 16, "height": 16})).unwrap();
        for (n, id) in ids.iter().take(used).enumerate() {
            s.execute("timeline.place", json!({"item": id.0, "track": "A1", "seconds": n})).unwrap();
        }
        ids
    }

    #[test]
    fn project_media_open_returns_while_gated_and_status_waits_without_import_outcomes() {
        let (mut s, gate) = gated();
        let ids = saved_items(&mut s, &["/outside.wav".into()], 1);
        let before = s.project.clone();
        let undo = s.undo_mark();
        let began = Instant::now();
        let out = s.execute("media.openInBackground", json!({})).unwrap();
        assert!(began.elapsed() < Duration::from_secs(5));
        assert_eq!(out["files"], 1);
        let job = out["job"].as_u64().unwrap();
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(s.execute("file.importStatus", json!({"job": job})).unwrap()["pending"], 1);
        assert!(s.media.cached_source(&s.project, ids[0]).is_none());
        gate.release.send(()).unwrap();
        let status = wait(&mut s, job);
        assert_eq!(status["outcomes"], json!([]));
        assert!(s.media.cached_source(&s.project, ids[0]).is_some());
        assert!(s.opening_items.is_empty());
        assert_eq!(s.undo_mark(), undo);
        assert_eq!(*s.project, *before);
    }

    #[test]
    fn project_media_scope_concurrency_and_cancel_keep_cached_files() {
        for (scope, expected) in [(None, 2), (Some("all"), 3)] {
            let (mut s, fs, paths) = pool_fixture(3, 0);
            let ids = saved_items(&mut s, &paths, 2);
            let mut params = json!({"openThreads": 2});
            if let Some(scope) = scope {
                params["scope"] = json!(scope);
            }
            let out = s.execute("media.openInBackground", params).unwrap();
            assert_eq!(out["files"], expected);
            fs.wait_for(2);
            fs.release();
            wait(&mut s, out["job"].as_u64().unwrap());
            let state = fs.state.lock().unwrap();
            assert_eq!(state.entered, expected as usize);
            assert_eq!(state.maximum, 2);
            assert!(ids.iter().take(expected as usize).all(|id| s.media.cached_source(&s.project, *id).is_some()));
        }
        let (mut s, fs, paths) = pool_fixture(8, 0);
        let ids = saved_items(&mut s, &paths, 0);
        let out = s.execute("media.openInBackground", json!({"scope": "all", "openThreads": 2})).unwrap();
        let job = out["job"].as_u64().unwrap();
        fs.wait_for(2);
        let status = s.execute("file.importStatus", json!({"job": job, "cancel": true})).unwrap();
        assert!(status["jobs"][0]["status"].as_str().unwrap().contains("2 files in progress"));
        fs.release();
        wait(&mut s, job);
        assert_eq!(fs.state.lock().unwrap().entered, 2);
        assert_eq!(ids.iter().filter(|id| s.media.cached_source(&s.project, **id).is_some()).count(), 2);
    }

    #[test]
    fn project_media_follows_nested_sequences_and_cached_reference_keys() {
        let (mut s, fs, paths) = pool_fixture(3, 1);
        let ids = saved_items(&mut s, &paths, 2);
        let inner = s.state.active_sequence.unwrap();
        let project = Arc::make_mut(&mut s.project);
        let mut clip = project.sequence(inner).unwrap().audio_tracks[0].items[0].clone();
        clip.item = inner;
        let outer = project.new_sequence("Outer", Default::default(), 1, 1, None);
        project.sequence_mut(outer).unwrap().audio_tracks[0].items.push(clip);
        s.state.open_sequences = vec![outer];
        let out = s.execute("media.openInBackground", json!({})).unwrap();
        assert_eq!(out["files"], 2);
        wait(&mut s, out["job"].as_u64().unwrap());
        assert_eq!(fs.state.lock().unwrap().entered, 2);
        assert!(s.media.cached_source(&s.project, ids[0]).is_some());
        let project = Arc::make_mut(&mut s.project);
        project.item_mut(ids[0]).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: paths[2].clone() };
        assert!(s.media.cached_source(&s.project, ids[0]).is_none(), "a stale reference is never returned");
        let project = Arc::make_mut(&mut s.project);
        project.item_mut(ids[1]).unwrap().as_media_mut().unwrap().proxy = Some(MediaRef::File { path: paths[2].clone() });
        s.media.set_use_proxies(true);
        assert!(s.media.cached_source(&s.project, ids[1]).is_none(), "an unopened preferred proxy is a miss");
        assert_eq!(fs.state.lock().unwrap().entered, 2);
        let out = s.execute("media.openInBackground", json!({"items": [ids[1].0]})).unwrap();
        wait(&mut s, out["job"].as_u64().unwrap());
        assert!(s.media.cached_source(&s.project, ids[1]).is_some());
    }

    #[test]
    fn cached_lookup_never_opens_and_requests_are_coalesced() {
        let (mut s, fs, paths) = pool_fixture(1, 0);
        let ids = saved_items(&mut s, &paths, 0);
        assert!(s.media.cached_source(&s.project, ids[0]).is_none());
        assert_eq!(fs.state.lock().unwrap().entered, 0);
        let began = Instant::now();
        for _ in 0..20 {
            assert!(s.source_if_open(ids[0]).is_none());
        }
        assert!(began.elapsed() < Duration::from_secs(5));
        fs.wait_for(1);
        assert_eq!(s.jobs.len(), 1);
        assert_eq!(fs.state.lock().unwrap().entered, 1);
        fs.release();
        let job = s.jobs[0].id;
        wait(&mut s, job);
        assert!(s.source_if_open(ids[0]).is_some());
    }

    #[test]
    fn project_media_counts_skips_and_creates_no_empty_job() {
        let (mut s, fs, paths) = pool_fixture(6, 1);
        let ids = saved_items(&mut s, &paths, 0);
        let source = crate::MediaPool::default().open_file(&paths[0], &fs.fs).unwrap();
        s.media.insert_file(ids[0], &paths[0], source);
        let project = Arc::make_mut(&mut s.project);
        project.item_mut(ids[1]).unwrap().as_media_mut().unwrap().media = MediaRef::Generator(filmcraft_media::Generator::ColorMatte { color: [0.0; 4] });
        project.item_mut(ids[2]).unwrap().as_media_mut().unwrap().offline = true;
        project.item_mut(ids[3]).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: "/missing.wav".into() };
        project.item_mut(ids[4]).unwrap().as_media_mut().unwrap().info.kind = filmcraft_media::MediaKind::ImageSequence;
        let out = s.execute("media.openInBackground", json!({"scope": "all"})).unwrap();
        assert_eq!(out["files"], 1);
        assert_eq!(out["skipped"], 5);
        wait(&mut s, out["job"].as_u64().unwrap());
        let out = s.execute("media.openInBackground", json!({"scope": "all"})).unwrap();
        assert_eq!(out, json!({"job": null, "pending": false, "files": 0, "skipped": 6}));
        assert_eq!(s.jobs.len(), 1);
        assert_eq!(fs.state.lock().unwrap().entered, 1);
        assert!(s.media.cached_source(&s.project, ids[1]).is_some());
        assert!(s.media.cached_source(&s.project, ids[2]).is_some());
    }

    #[test]
    fn failed_source_requests_retry_after_the_cooldown() {
        let (mut s, fs, paths) = pool_fixture(1, 1);
        let ids = saved_items(&mut s, &paths, 0);
        fs.fs.files.lock().unwrap().insert(paths[0].clone(), vec![1, 2, 3]);
        assert!(s.source_if_open(ids[0]).is_none());
        let job = s.jobs[0].id;
        wait(&mut s, job);
        assert!(s.source_if_open(ids[0]).is_none());
        assert_eq!(s.jobs.len(), 1);
        s.opening_times.insert(ids[0], web_time::Instant::now() - Duration::from_secs(5));
        assert!(s.source_if_open(ids[0]).is_none());
        let job = s.jobs[1].id;
        wait(&mut s, job);
        assert_eq!(fs.state.lock().unwrap().entered, 2);
    }

    #[test]
    fn warm_project_open_is_optional_and_returns_before_media_open() {
        let path = std::env::temp_dir().join("filmcraft-warm.wav").to_string_lossy().into_owned();
        let document = std::env::temp_dir().join("filmcraft-warm.fcproj").to_string_lossy().into_owned();
        let fs = folder_fixture();
        let wav = fs.files.lock().unwrap().get("/outside.wav").unwrap().clone();
        fs.files.lock().unwrap().insert(path.clone(), wav);
        let (release, receive) = mpsc::channel();
        let (entered, ready) = mpsc::channel();
        let services = Arc::new(GateFs { fs, gate: Mutex::new(Some(receive)), entered, path: path.clone() });
        let gate = Gate { release, entered: ready };
        let mut s = Session::new(services);
        let ids = saved_items(&mut s, &[path], 1);
        s.execute("file.saveAs", json!({"path": document})).unwrap();
        let out = s.execute("file.open", json!({"path": document})).unwrap();
        assert!(out.get("warm").is_none());
        assert!(s.jobs.is_empty());
        assert!(gate.entered.try_recv().is_err());
        let began = Instant::now();
        let out = s.execute("file.open", json!({"path": document, "warm": true})).unwrap();
        assert!(began.elapsed() < Duration::from_secs(5));
        let job = out["warm"]["job"].as_u64().unwrap();
        gate.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        gate.release.send(()).unwrap();
        wait(&mut s, job);
        assert!(s.media.cached_source(&s.project, ids[0]).is_some());
    }

    #[test]
    fn project_media_rejects_hostile_parameters_and_contains_panics() {
        let (mut s, fs, paths) = pool_fixture(1, 1);
        let ids = saved_items(&mut s, &paths, 0);
        for params in [json!({"items": true}), json!({"items": [-1]}), json!({"items": [null]}), json!({"scope": "unknown"})] {
            assert!(s.execute("media.openInBackground", params).is_err());
        }
        let project = Arc::make_mut(&mut s.project);
        project.item_mut(ids[0]).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: "/panic.wav".into() };
        fs.fs.files.lock().unwrap().insert("/panic.wav".into(), vec![0]);
        let out = s.execute("media.openInBackground", json!({"items": [ids[0].0]})).unwrap();
        let status = wait(&mut s, out["job"].as_u64().unwrap());
        assert_eq!(status["jobs"][0]["done"], 1);
        assert!(s.opening_items.is_empty());
    }

}

#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_ignores_background_import() {
    use std::sync::Arc;
    use serde_json::json;
    use crate::{Session, media_browser_tests::folder_fixture};

    let mut s = Session::new(Arc::new(folder_fixture()));
    let out = s.execute("file.import", json!({"paths": ["/outside.wav"], "background": true})).unwrap();
    assert!(out.get("job").is_none());
    assert_eq!(s.project.items.len(), 1);
}

#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_background_project_media_is_a_no_op() {
    let mut s = crate::Session::default();
    let out = s.execute("media.openInBackground", serde_json::json!({"scope": "all"})).unwrap();
    assert_eq!(out, serde_json::json!({"job": null, "pending": false, "files": 0, "skipped": 0}));
    s.request_open(filmcraft_project::ItemId(1));
    assert!(s.jobs.is_empty());
}

#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_relink_background_parameters_retain_synchronous_results() {
    let mut s = crate::Session::new(std::sync::Arc::new(crate::media_browser_tests::folder_fixture()));
    let imported = s.execute("file.import", serde_json::json!({"paths": ["/outside.wav"]})).unwrap();
    for (command, params) in [
        ("media.relink", serde_json::json!({"item": imported["items"][0], "path": "/outside.wav"})),
        ("media.autoRelink", serde_json::json!({"from": "/missing", "to": "/"})),
        ("media.autoRelink", serde_json::json!({"folder": "/missing"})),
    ] {
        let expected = s.execute(command, params.clone()).unwrap();
        let mut params = params;
        params["background"] = serde_json::json!(true);
        params["openThreads"] = serde_json::json!(4);
        assert_eq!(s.execute(command, params).unwrap(), expected);
        assert!(s.jobs.is_empty());
    }
}
