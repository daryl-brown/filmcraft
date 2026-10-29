//! Background imports against the same fake filesystem as the browser tests.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{Arc, Mutex};
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
        let out = s.execute("file.import", json!({"paths": ["/outside.wav", "/root/A/a1.wav", "/root/A/sub/s1.wav"], "background": true})).unwrap();
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
