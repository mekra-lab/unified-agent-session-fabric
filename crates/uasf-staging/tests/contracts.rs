use std::{
    fs, io::{Seek, SeekFrom, Write},
    os::unix::fs::{symlink, PermissionsExt, MetadataExt},
    path::PathBuf, process::Command, sync::{Arc, Barrier},
};
use uasf_json::{json, Value};
use uasf_staging::{Store, Error, files, policy, uuid, LIMIT};
use uasf_core::model::sha;
const REL: &str = "sessions/rollout-synthetic.jsonl";
struct Fixture {
    base: PathBuf,
    store: Store,
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base =
            std::env::temp_dir().join(format!("uasf-staging-unit-{}",uuid().unwrap()));
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base,
                fs::Permissions::from_mode(0o700)).unwrap();
        let original = base.join("original");
        let path = original.join(REL);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{}\n").unwrap();
        let store =
            Store::new(base.join("stage"),
                    json!({"version":1,"sources":{"s":{"kind":"codex","root":original}}})).unwrap();
        Self { base, store, path }
    }
    fn capture(&self) -> Value { self.store.capture("s", REL).unwrap() }
    fn queue(&self, status: &str) -> PathBuf {
        let q = self.base.join("queue");
        fs::create_dir_all(&q).unwrap();
        let marker = self.store.root.join("deletion-binding.json");
        fs::write(&marker,
                uasf_json::to_vec(&json!({"version":1,"kind":"staging","resource":self.store.root,"root":q,"owner_uid":unsafe{libc::geteuid()}})).unwrap()).unwrap();
        fs::set_permissions(marker,
                fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(q.join("jobs.json"),
                uasf_json::to_vec(&json!({"version":1,"binding":{"staging":self.store.root},"jobs":{"j":{"status":status,"paths":[],"inodes":[],"selector":{"kind":"text","length":6,"sha256":sha(b"erased")}}}})).unwrap()).unwrap();
        q
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.base); }
}
#[test]
fn exact_limit_and_over_limit() {
    let f = Fixture::new();
    let mut stream = fs::File::create(&f.path).unwrap();
    stream.set_len(LIMIT).unwrap();
    stream.seek(SeekFrom::End(-1)).unwrap();
    stream.write_all(b"\n").unwrap();
    let result = f.capture();
    assert_eq!(result["status"],"published");
    assert_eq!(result["receipt"]["length"],LIMIT);
    stream.set_len(LIMIT + 1).unwrap();
    assert_eq!(f.capture(),json!({"status":"unsupported","reason":"over_32_MiB"}));
}
#[test]
fn copy_mutation_is_never_published() {
    let f = Fixture::new();
    fs::write(&f.path, vec![b'x';600_000]).unwrap();
    let mut once = false;
    let r =
        f.store.capture_with_hook("s", REL,
                &mut |p|
                        {
                            if p == "copy_chunk" && !once {
                                once = true;
                                fs::write(&f.path, b"changed\n")?;
                            }
                            Ok(())
                        }).unwrap();
    assert_eq!(r["status"],"unstable");
    assert!(f.store.state().unwrap()["receipts"].as_object().unwrap().is_empty());
}
#[test]
fn opened_file_replacement_does_not_follow_symlink() {
    let f = Fixture::new();
    let other = f.base.join("other");
    fs::write(&other, b"must never publish\n").unwrap();
    let mut once = false;
    let r =
        f.store.capture_with_hook("s", REL,
                &mut |_|
                        {
                            if !once {
                                once = true;
                                fs::remove_file(&f.path)?;
                                symlink(&other, &f.path)?;
                            }
                            Ok(())
                        }).unwrap();
    assert_eq!(r["status"],"unstable");
    assert!(f.store.state().unwrap()["receipts"].as_object().unwrap().is_empty());
}
#[test]
fn root_symlink_and_fifo_fail_without_blocking() {
    let f = Fixture::new();
    let alias = f.base.join("alias");
    symlink(f.base.join("original"), &alias).unwrap();
    let store =
        Store::new(f.base.join("another-stage"),
                json!({"version":1,"sources":{"s":{"kind":"codex","root":alias}}})).unwrap();
    assert_eq!(store.capture("s",REL).unwrap()["status"],"rejected");
    fs::remove_file(&f.path).unwrap();
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(f.path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe{libc::mkfifo(name.as_ptr(),0o600)},0);
    assert_eq!(f.capture(),json!({"status":"rejected","reason":"not_regular"}));
}
#[test]
fn explicit_files_and_root_boundaries() {
    let f = Fixture::new();
    let mut registry = f.store.registry.clone();
    registry["sources"]["s"]["files"] = json!(["custom.jsonl"]);
    fs::write(f.base.join("original/custom.jsonl"), b"{}\n").unwrap();
    let s = Store::new(f.base.join("explicit"), registry).unwrap();
    assert_eq!(s.capture("s","custom.jsonl").unwrap()["status"],"published");
    assert_eq!(s.capture("s",REL).unwrap()["reason"],"not_registered");
    assert!(s.capture("s","../escape.jsonl").is_err());
    assert!(Store::new(f.base.join("original/overlap"),f.store.registry.clone()).is_err());
    let repo = f.base.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(repo.join(".git/HEAD"),
            b"ref: refs/heads/synthetic\n").unwrap();
    assert!(Store::new(repo.join("stage"),f.store.registry.clone()).is_err());
}
#[test]
fn original_facts_and_immutable_tail_receipt() {
    let f = Fixture::new();
    fs::write(&f.path, b"{}\ntail").unwrap();
    let first = f.capture()["receipt"].clone();
    assert_eq!(first["source_facts"],files::facts(&fs::metadata(&f.path).unwrap()));
    assert_eq!(first["source_sha256"],sha(b"{}\ntail"));
    assert_eq!(first["sha256"],sha(b"{}\n"));
    let copy = f.store.root.join(first["staging_relative"].as_str().unwrap());
    assert_eq!(fs::metadata(&copy).unwrap().mode()&0o777,0o400);
    assert_ne!(fs::metadata(&copy).unwrap().ino(),fs::metadata(&f.path).unwrap().ino());
    fs::write(&f.path, b"{}\ntail-longer").unwrap();
    assert_eq!(f.capture()["receipt"],first);
}
#[test]
fn publication_is_durable_before_visibility() {
    let f = Fixture::new();
    let root = f.store.root.clone();
    f.store.capture_with_hook("s", REL,
            &mut |point|
                    {
                        if point == "receipt_durable" {
                            let state = files::json(&root.join("state.json"))?;
                            let receipt =
                                state["receipts"].as_object().unwrap().values().next().unwrap();
                            assert_eq!(receipt["status"],"pending");
                            assert!(!root.join(receipt["staging_relative"].as_str().unwrap()).exists());
                            assert!(root.join(receipt["temporary"].as_str().unwrap()).exists());
                        }
                        Ok(())
                    }).unwrap();
}
#[test]
fn interrupted_publications_recover_and_integrity_failure_is_closed() {
    for point in ["receipt_durable", "published"] {
        let f = Fixture::new();
        assert!(f.store.capture_with_hook("s",REL,&mut|p|if
        p==point{Err(Error::Integrity)}else{Ok(())}).is_err());
        let restarted =
            Store::new(f.store.root.clone(),
                    f.store.registry.clone()).unwrap();
        assert_eq!(restarted.capture("s",REL).unwrap()["status"],"unchanged");
        assert_eq!(fs::read_dir(f.store.root.join("tmp")).unwrap().count(),0);
    }
    let f = Fixture::new();
    let _ =
        f.store.capture_with_hook("s", REL,
            &mut |p|
                    if p == "published" {
                        Err(Error::Integrity)
                    } else { Ok(()) });
    let state = files::json(&f.store.root.join("state.json")).unwrap();
    let r = state["receipts"].as_object().unwrap().values().next().unwrap();
    let path = f.store.root.join(r["staging_relative"].as_str().unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(path, b"bad\n").unwrap();
    assert!(f.store.state().is_err());
}
#[test]
fn interrupted_delete_recovers_minimal_tombstone() {
    let f = Fixture::new();
    let r = f.capture()["receipt"].clone();
    assert!(f.store.delete_with_hook("s",REL,&mut|_|Err(Error::Integrity)).is_err());
    let restarted =
        Store::new(f.store.root.clone(), f.store.registry.clone()).unwrap();
    assert_eq!(restarted.capture("s",REL).unwrap()["status"],"deleted");
    let state = restarted.state().unwrap();
    let tomb = &state["receipts"][r["generation"].as_str().unwrap()];
    assert_eq!(tomb.as_object().unwrap().len(),3);
    assert_eq!(tomb["status"],"deleted");
    assert!(!f.store.root.join(r["staging_relative"].as_str().unwrap()).exists());
}
#[test]
fn concurrent_publishers_make_one_generation() {
    let f = Fixture::new();
    let barrier = Arc::new(Barrier::new(4));
    let mut threads = Vec::new();
    for _ in 0..4 {
        let b = barrier.clone();
        let root = f.store.root.clone();
        let registry = f.store.registry.clone();
        threads.push(std::thread::spawn(move ||
                    {
                        let s = Store::new(root, registry).unwrap();
                        b.wait();
                        s.capture("s", REL).unwrap()
                    }));
    }
    let results: Vec<_> =
        threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r|r["status"]=="published").count(),1);
    assert_eq!(f.store.state().unwrap()["receipts"].as_object().unwrap().len(),1);
}
#[test]
fn deletion_queue_busy_pending_and_unavailable_fail_closed() {
    let f = Fixture::new();
    let q = f.queue("complete");
    let lock = files::Lock::acquire(&q.join("lock"), true).unwrap();
    assert_eq!(f.capture()["status"],"pending_deletion");
    assert_eq!(f.store.scan("s").unwrap(),json!([{"status":"pending_deletion"}]));
    drop(lock);
    assert_eq!(f.capture()["status"],"published");
    fs::write(q.join("jobs.json"), b"bad").unwrap();
    assert_eq!(f.capture()["status"],"deletion_state_unavailable");
    fs::remove_file(q.join("jobs.json")).unwrap();
    assert_eq!(f.capture()["status"],"deletion_state_unavailable");
    assert_eq!(f.store.delete("s",REL).unwrap()["status"],"coordinator_required");
}
#[test]
fn binding_identity_and_required_cli() {
    let f = Fixture::new();
    assert!(matches!(policy::binding(&f.store.root,None,true),Err(Error::Denied("deletion_binding_required"))));
    f.queue("complete");
    let marker = f.store.root.join("deletion-binding.json");
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(policy::binding(&f.store.root,None,true),Err(Error::Denied("invalid_deletion_binding"))));
    let config = f.base.join("sources.json");
    fs::write(&config,
            uasf_json::to_vec(&f.store.registry).unwrap()).unwrap();
    let out =
        Command::new(env!("CARGO_BIN_EXE_uasf-staging")).args(["--root",
                            f.store.root.to_str().unwrap(), "--registry",
                            config.to_str().unwrap(), "--unmanaged-test-mode",
                            "capture", "s", REL]).output().unwrap();
    assert_eq!(out.status.code(),Some(2));
    assert!(String::from_utf8(out.stdout).unwrap().contains("invalid_deletion_binding"));
}
#[test]
fn text_policy_checks_keys_escapes_and_unpublished_suffix() {
    let selector = json!({"length":6,"sha256":sha(b"erased")});
    for data in
        [br#"{"erased":0}"#.as_slice(), br#"{"v":"\u0065rased"}"#.as_slice(),
                b"{}\nerased".as_slice()] {
        assert_eq!(policy::contains(data,&selector),policy::Inspection::Match);
    }
    let f = Fixture::new();
    f.queue("complete");
    fs::write(&f.path, b"{}\nerased").unwrap();
    assert_eq!(f.capture()["status"],"deleted");
}
#[test]
fn process_death_then_fresh_cli_recovers() {
    if let Ok(root) = std::env::var("UASF_STAGING_CRASH_FIXTURE") {
        let root = PathBuf::from(root);
        let config = files::json(&root.join("sources.json")).unwrap();
        let s = Store::new(root.join("stage"), config).unwrap();
        s.capture_with_hook("s", REL,
                &mut |p|
                        {
                            if p == "receipt_durable" { std::process::exit(77) }
                            Ok(())
                        }).unwrap();
        panic!("crash hook did not run")
    }
    let f = Fixture::new();
    fs::write(f.base.join("sources.json"),
            uasf_json::to_vec(&f.store.registry).unwrap()).unwrap();
    let child =
        Command::new(std::env::current_exe().unwrap()).args(["--exact",
                                "process_death_then_fresh_cli_recovers"]).env("UASF_STAGING_CRASH_FIXTURE",
                    &f.base).output().unwrap();
    assert_eq!(child.status.code(),Some(77));
    let result =
        Command::new(env!("CARGO_BIN_EXE_uasf-staging")).arg("--root").arg(&f.store.root).arg("--registry").arg(f.base.join("sources.json")).args(["--unmanaged-test-mode",
                            "capture", "s", REL]).output().unwrap();
    assert!(result.status.success());
    assert_eq!(uasf_json::from_slice(&result.stdout).unwrap()["status"],"unchanged");
}
#[test]
fn legacy_partial_receipt_gets_explicit_boundary_generation() {
    let f = Fixture::new();
    let first = f.capture()["receipt"].clone();
    fs::write(&f.path, b"{}\npartial").unwrap();
    let path = f.store.root.join(first["staging_relative"].as_str().unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, b"{}\npartial").unwrap();
    let mut state = files::json(&f.store.root.join("state.json")).unwrap();
    let old = &mut state["receipts"][first["generation"].as_str().unwrap()];
    for field in
        ["source_length", "source_sha256", "unpublished_length",
                "publication_policy"] {
        old.as_object_mut().unwrap().remove(field);
    }
    old["length"] = json!(10);
    old["sha256"] = json!(sha(b"{}\npartial"));
    f.store.save(&state).unwrap();
    let r = f.capture();
    assert_eq!(r["receipt"]["relation"],"publication_boundary_changed");
    assert_ne!(r["receipt"]["file_generation"],first["file_generation"]);
    assert_eq!(fs::read(path).unwrap(),b"{}\npartial");
}
#[test]
fn malformed_completed_job_cannot_admit_bytes() {
    let f=Fixture::new();let q=f.queue("complete");let path=q.join("jobs.json");
    let mut state=files::json(&path).unwrap();state["jobs"]["j"].as_object_mut().unwrap().remove("paths");
    fs::write(&path,uasf_json::to_vec(&state).unwrap()).unwrap();
    assert!(matches!(f.store.capture("s",REL),Err(Error::Key)));
    assert!(!f.store.root.join("state.json").exists());
}

fn staged_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { found.extend(staged_files(&path)); }
        else if path.extension().is_some_and(|ext| ext == "jsonl") { found.push(path); }
    }
    found
}

#[test]
fn deletion_inspection_synthetic_contracts_without_python() {
    let cases = uasf_json::from_str(include_str!("inspection.json")).unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let f = Fixture::new();
        let data = case["input"].as_str().unwrap().as_bytes();
        fs::write(&f.path, data).unwrap();
        if !case["selector"].is_null() {
            let q = f.queue("complete");
            let mut queue = files::json(&q.join("jobs.json")).unwrap();
            let mut selector = case["selector"].clone();
            if selector["kind"] == "text" {
                let text = selector.as_object_mut().unwrap().remove("text").unwrap();
                let text = text.as_str().unwrap().as_bytes();
                selector["length"] = json!(text.len());
                selector["sha256"] = json!(sha(text));
            }
            queue["jobs"]["j"]["selector"] = selector;
            fs::write(q.join("jobs.json"), uasf_json::to_vec(&queue).unwrap()).unwrap();
        }
        let result = f.capture();
        assert_eq!(result["status"],case["rust_status"],"{}",case["name"]);
        assert_eq!(fs::read(&f.path).unwrap(),data);
        // A new Store and scan must retain the admission decision without tombstones.
        let restarted = Store::new(f.store.root.clone(), f.store.registry.clone()).unwrap();
        let scanned = restarted.scan("s").unwrap();
        let state = restarted.state().unwrap();
        assert_eq!(state["blocked_paths"],json!([]));
        assert_eq!(state["blocked_inodes"],json!([]));
        if result["status"] == "published" {
            assert_eq!(state["receipts"].as_object().unwrap().len(),1);
            assert_eq!(staged_files(&f.store.root).len(),1);
        } else {
            assert_eq!(scanned[0]["status"],case["rust_status"]);
            assert_eq!(state["receipts"],json!({}));
            assert!(staged_files(&f.store.root).is_empty());
            assert_eq!(fs::read_dir(f.store.root.join("tmp")).unwrap().count(),0);
            if result["status"] == "not_captured" {
                assert_eq!(result,json!({"status":"not_captured","reason":"deletion_inspection_unavailable"}));
                assert_eq!(state["events"].as_array().unwrap().last().unwrap()["reason"],"deletion_inspection_unavailable");
            }
        }
    }
}

#[test]
fn deletion_inspection_invalid_encoding_and_depth_boundary() {
    let selector = json!({"length":6,"sha256":sha(b"erased")});
    for (depth, expected) in [(128,policy::Inspection::Match),(129,policy::Inspection::Unavailable)] {
        let data = format!("{}\"\\u0065rased\"{}\n","[".repeat(depth),"]".repeat(depth));
        assert_eq!(policy::contains(data.as_bytes(),&selector),expected);
    }
    for data in [b"{\"sessionId\":\"\\u0065rased\",\"extra\":\"\xff\"}\n".as_slice(),
                 b"{\"sessionId\":\"\\u0065rased\",\"extra\":\"\\ud800\"}\n".as_slice()] {
      for session_rule in [false,true] {
        let f = Fixture::new();
        let q = f.queue("complete");
        if session_rule {
            let path = q.join("jobs.json");
            let mut queue = files::json(&path).unwrap();
            queue["jobs"]["j"]["selector"] = json!({"kind":"session","source":"s","session":"erased"});
            fs::write(path,uasf_json::to_vec(&queue).unwrap()).unwrap();
        }
        fs::write(&f.path,data).unwrap();
        assert_eq!(f.capture(),json!({"status":"not_captured","reason":"deletion_inspection_unavailable"}));
        assert_eq!(f.store.state().unwrap()["receipts"],json!({}));
        assert!(staged_files(&f.store.root).is_empty());
      }
    }
}

#[test]
fn unavailable_rule_survives_other_nonmatches_but_proven_match_wins() {
    let f = Fixture::new();
    let q = f.queue("complete");
    let path = q.join("jobs.json");
    let mut queue = files::json(&path).unwrap();
    queue["jobs"]["other"] = json!({"status":"complete","paths":[],"inodes":[],
        "selector":{"kind":"session","source":"other","session":"erased"}});
    fs::write(&path,uasf_json::to_vec(&queue).unwrap()).unwrap();
    fs::write(&f.path,b"{\"v\":\"\\u0065rased\",\"n\":1e9999}\n").unwrap();
    assert_eq!(f.capture()["reason"],"deletion_inspection_unavailable");
    queue["jobs"]["other"]["paths"] = json!([f.path]);
    fs::write(&path,uasf_json::to_vec(&queue).unwrap()).unwrap();
    assert_eq!(f.capture()["status"],"deleted");
    assert_eq!(f.store.state().unwrap()["receipts"],json!({}));
    assert!(staged_files(&f.store.root).is_empty());
}

#[test]
fn unavailable_inspection_preserves_old_copy_and_allows_corrected_retry() {
    let f = Fixture::new();
    f.queue("complete");
    let first = f.capture()["receipt"].clone();
    let old = f.store.root.join(first["staging_relative"].as_str().unwrap());
    fs::write(&f.path,b"{\"v\":\"\\u0065rased\",\"n\":1e9999}\n").unwrap();
    assert_eq!(f.capture()["reason"],"deletion_inspection_unavailable");
    assert_eq!(fs::read(&old).unwrap(),b"{}\n");
    assert_eq!(staged_files(&f.store.root),vec![old]);
    assert_eq!(f.store.state().unwrap()["receipts"][first["generation"].as_str().unwrap()],first);
    fs::write(&f.path,b"{\"safe\":1}\n").unwrap();
    let restarted = Store::new(f.store.root.clone(),f.store.registry.clone()).unwrap();
    assert_eq!(restarted.capture("s",REL).unwrap()["status"],"published");
    let state = restarted.state().unwrap();
    assert_eq!(state["blocked_paths"],json!([]));
    assert_eq!(state["blocked_inodes"],json!([]));
}

#[test]
fn shared_json_corpus_never_publishes_or_creates_active_receipt() {
    let cases = uasf_json::from_str(include_str!("../../uasf-json/tests/inspection.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let f = Fixture::new();
        fs::write(&f.path, c["input"].as_str().unwrap()).unwrap();
        let q = f.queue("complete");
        let mut state = files::json(&q.join("jobs.json")).unwrap();
        let mut selector = c["selector"].clone();
        if selector["kind"] == "text" {
            let text = selector.as_object_mut().unwrap().remove("text").unwrap();
            selector["length"] = json!(text.as_str().unwrap().len());
            selector["sha256"] = json!(sha(text.as_str().unwrap().as_bytes()));
        }
        state["jobs"]["j"]["selector"] = selector;
        fs::write(q.join("jobs.json"),state.to_string()).unwrap();
        for _ in 0..2 {
            let store = Store::new(f.store.root.clone(), f.store.registry.clone()).unwrap();
            let response = store.capture("s",REL).unwrap();
            assert_eq!(response["status"], if c["decode_ok"] == true { "deleted" } else { "not_captured" }, "{}", c["name"]);
            assert_eq!(store.state().unwrap()["receipts"],json!({}));
            assert!(staged_files(&store.root).is_empty());
        }
    }
}
