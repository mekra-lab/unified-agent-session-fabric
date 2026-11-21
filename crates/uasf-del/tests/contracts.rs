use std::{
    fs, io::Write, path::PathBuf, process::Command,
    os::unix::fs::PermissionsExt,
};
use uasf_json::{json, Value};
use rusqlite::Connection;
use uasf_del::{Coordinator, Executor, Lease, Result, Error, STEPS};
use uasf_core::model::sha;
use uasf_staging::Store;
const REL: &str = "sessions/rollout-one.jsonl";
const BYTES: &[u8] =
    b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"session\"}}\n{\"text\":\"erase\"}\n";
static FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Fixture {
    root: PathBuf,
    q: Coordinator,
    map: Value,
    _guard: std::sync::MutexGuard<'static, ()>,
}
impl Fixture {
    fn new() -> Self {
        let guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root =
            std::env::temp_dir().join(format!("uasf-del-test-{}",uasf_staging::uuid().unwrap()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root,
                fs::Permissions::from_mode(0o700)).unwrap();
        let source = root.join("original");
        fs::create_dir_all(source.join("sessions")).unwrap();
        fs::write(source.join(REL), BYTES).unwrap();
        let registry =
            json!({"version":1,"sources":{"s":{"kind":"codex","root":source}}});
        let store =
            Store::new(root.join("staging"), registry.clone()).unwrap();
        let receipt = store.capture("s", REL).unwrap()["receipt"].clone();
        let map =
            json!({store.root.join(receipt["staging_relative"].as_str().unwrap()).to_str().unwrap():["input-one","input-two"]});
        let db = Connection::open(root.join("intake.db")).unwrap();
        db.execute_batch("PRAGMA secure_delete=ON;CREATE TABLE observation(receipt_id TEXT PRIMARY KEY,source TEXT,idem_key TEXT,session TEXT,payload BLOB);CREATE TABLE deletion_tombstone(receipt_id TEXT PRIMARY KEY,source TEXT,key_hash TEXT);").unwrap();
        db.execute("INSERT INTO observation VALUES ('receipt','s','key','session',?1)",
                [BYTES]).unwrap();
        drop(db);
        let cfg =
            json!({"root":root.join("queue"),"staging":root.join("staging"),"registry":registry,"intake":root.join("intake.db"),"intake_command":["/bin/false"],"archive_command":["/bin/false","{scope}"]});
        fs::write(root.join("config.json"), cfg.to_string()).unwrap();
        let q = uasf_del::process::config(&cfg, true).unwrap();
        q.cache("key", Some(BYTES)).unwrap();
        Self { root, q, map, _guard: guard }
    }
    fn request(&self, kind: &str) -> Value {
        let sel =
            match kind {
                "text" => json!({"kind":"text","text":"erase"}),
                "file" => json!({"kind":"file","source":"s","relative":REL}),
                "receipt" =>
                    json!({"kind":"receipt","source":"s","receipt_id":"receipt"}),
                _ =>
                    json!({"kind":"session","source":"s","session":"session"}),
            };
        self.q.request(&sel, &self.map, Some("job"), &mut |_| Ok(())).unwrap()
    }
    fn executor(&self) -> Effects {
        Effects { root: self.root.clone(), fail: false }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
}
struct Effects {
    root: PathBuf,
    fail: bool,
}
impl Executor for Effects {
    fn erase(&mut self, ids: &Value, _: &Lease) -> Result<()> {
        let mut db = Connection::open(self.root.join("intake.db"))?;
        db.execute_batch("PRAGMA secure_delete=ON;PRAGMA synchronous=FULL;")?;
        let tx = db.transaction()?;
        for id in ids.as_array().unwrap() {
            let id = id.as_str().unwrap();
            let mut query =
                tx.prepare("SELECT source,idem_key FROM observation WHERE receipt_id=?1")?;
            let rows =
                query.query_map([id],
                        |r|
                            Ok((r.get::<_, String>(0)?,
                                    r.get::<_, Option<String>>(1)?)))?;
            for row in rows {
                let (s, k) = row?;
                tx.execute("INSERT OR IGNORE INTO deletion_tombstone VALUES (?1,?2,?3)",
                        rusqlite::params![id,s,k.map(|k|sha(k.as_bytes()))])?;
            }
            tx.execute("DELETE FROM observation WHERE receipt_id=?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }
    fn excise(&mut self, scope: &str) -> Result<()> {
        if self.fail { return Err(Error("ConnectionError")); }
        let mut f =
            fs::OpenOptions::new().create(true).append(true).open(self.root.join("effects"))?;
        writeln!(f,"{scope}")?;
        f.sync_all()?;
        Ok(())
    }
}
#[test]
fn selectors_freeze_intent_and_complete_idempotently() {
    for kind in ["session", "file", "text", "receipt"] {
        let f = Fixture::new();
        if kind == "file" { f.q.link_receipt("receipt", "s", REL).unwrap(); }
        let job = f.request(kind);
        assert_eq!(job["status"],"pending");
        assert_eq!(f.q.store.capture("s",REL).unwrap()["status"],"pending_deletion");
        assert_eq!(f.q.cache("key",None).unwrap()["status"],"pending_deletion");
        assert_eq!(f.q.resume(&mut f.executor(),&mut
        |_|Ok(())).unwrap()["status"],"complete");
        let effects = fs::read(f.root.join("effects")).unwrap_or_default();
        assert_eq!(f.q.resume(&mut Effects{root:f.root.clone(),fail:true},&mut
        |_|Ok(())).unwrap()["status"],"complete");
        assert_eq!(fs::read(f.root.join("effects")).unwrap_or_default(),effects);
        let db = Connection::open(&f.q.intake).unwrap();
        assert_eq!(db.query_row("SELECT count(*) FROM observation",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(db.query_row("SELECT count(*) FROM deletion_tombstone",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(f.q.cache("stale",Some(BYTES)).unwrap()["status"],"disabled_after_deletion");
        let state = f.q.state().unwrap();
        assert_eq!(state["jobs"]["job"]["steps"],json!(STEPS));
        assert!(state["jobs"]["job"].get("pending_text").is_none());
        if kind != "receipt" {
            let s = f.q.store.state().unwrap();
            for r in s["receipts"].as_object().unwrap().values() {
                assert_eq!(r.as_object().unwrap().len(),3);
                assert_eq!(r["status"],"deleted");
                assert!(!f.q.store.root.join(r["staging_relative"].as_str().unwrap()).exists());
            }
        }
    }
}
#[test]
fn offline_archive_keeps_barrier_after_local_purge() {
    let f = Fixture::new();
    f.request("session");
    let mut ex = f.executor();
    ex.fail = true;
    assert_eq!(f.q.resume(&mut ex,&mut
    |_|Ok(())).unwrap()["status"],"pending");
    let job = f.q.state().unwrap()["jobs"]["job"].clone();
    assert_eq!(job["steps"],json!(["staging","intake","cache"]));
    assert_eq!(job["error"],"ConnectionError");
    assert_eq!(f.q.store.capture("s",REL).unwrap()["status"],"pending_deletion");
    assert_eq!(f.q.resume(&mut f.executor(),&mut
    |_|Ok(())).unwrap()["status"],"complete");
}
#[test]
fn missing_mapping_corruption_and_unknown_inspection_never_write_intent() {
    let f = Fixture::new();
    assert!(f.q.request(&json!({"kind":"session","source":"s","session":"session"}),&json!({}),Some("bad"),&mut
    |_|Ok(())).is_err());
    assert_eq!(f.q.state().unwrap()["jobs"],json!({}));
    let staged =
        PathBuf::from(f.map.as_object().unwrap().keys().next().unwrap());
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&staged, b"bad\n").unwrap();
    assert!(f.q.request(&json!({"kind":"text","text":"erase"}),&f.map,None,&mut
    |_|Ok(())).is_err());
    assert_eq!(f.q.state().unwrap()["jobs"],json!({}));
}
#[test]
fn request_retry_conflict_and_missing_journal() {
    let f = Fixture::new();
    let first = f.request("session");
    assert_eq!(f.request("session"),first);
    assert!(f.q.request(&json!({"kind":"text","text":"other"}),&f.map,Some("job"),&mut
    |_|Ok(())).is_err());
    fs::remove_file(f.q.root.join("jobs.json")).unwrap();
    let cfg =
        uasf_json::from_slice(&fs::read(f.root.join("config.json")).unwrap()).unwrap();
    assert!(uasf_del::process::config(&cfg,true).is_err());
}
#[test]
fn linked_receipt_follows_file_and_orphan_temporary_is_removed() {
    let f = Fixture::new();
    f.q.link_receipt("receipt", "s", REL).unwrap();
    fs::write(f.q.root.join(".pending-abandoned"), b"erase").unwrap();
    f.q.cache("key", None).unwrap();
    assert!(!f.q.root.join(".pending-abandoned").exists());
    assert_eq!(f.request("file")["receipts"],json!(["receipt"]));
    f.q.resume(&mut f.executor(), &mut |_| Ok(())).unwrap();
    assert_eq!(f.q.state().unwrap()["links"],json!({}));
}
fn crash_worker() {
    let Ok(folder) = std::env::var("UASF_DEL_TEST_ROOT") else { return };
    let root = PathBuf::from(folder);
    let cfg =
        uasf_json::from_slice(&fs::read(root.join("config.json")).unwrap()).unwrap();
    let q = uasf_del::process::config(&cfg, false).unwrap();
    let point = std::env::var("UASF_DEL_TEST_POINT").unwrap();
    let mut hook =
        |p: &str| { if p == point { std::process::exit(77) } Ok(()) };
    if point == "intent" {
        let map =
            uasf_json::from_slice(&fs::read(root.join("map.json")).unwrap()).unwrap();
        q.request(&json!({"kind":"session","source":"s","session":"session"}),
                &map, Some("job"), &mut hook).unwrap();
    } else {
        q.resume(&mut Effects { root: root.clone(), fail: false },
                &mut hook).unwrap();
    }
}
#[test]
fn real_process_death_between_every_effect_and_ack() {
    if std::env::var_os("UASF_DEL_TEST_ROOT").is_some() {
        crash_worker();
        return;
    }
    let mut points = vec!["intent".to_owned()];
    for step in STEPS {
        points.extend([format!("{step}-effect"), step.into()]);
    }
    points.extend(["archive-effect-0", "archive-ack-0", "archive-effect-1",
                    "archive-ack-1"].map(String::from));
    for point in points {
        let f = Fixture::new();
        fs::write(f.root.join("map.json"), f.map.to_string()).unwrap();
        if point != "intent" { f.request("session"); }
        let child =
            |point: &str|
                Command::new(std::env::current_exe().unwrap()).args(["--exact",
                                            "real_process_death_between_every_effect_and_ack",
                                            "--nocapture"]).env("UASF_DEL_TEST_ROOT",
                                &f.root).env("UASF_DEL_TEST_POINT",
                            point).output().unwrap();
        assert_eq!(child(&point).status.code(),Some(77),"{point}");
        let out =
            Command::new(env!("CARGO_BIN_EXE_uasf-del")).arg("--config").arg(f.root.join("config.json")).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().map(|mut c|
                        {
                            c.stdin.take().unwrap().write_all(b"{\"op\":\"state\"}").unwrap();
                            c.wait_with_output().unwrap()
                        }).unwrap();
        assert!(out.status.success());
        let state = uasf_json::from_slice(&out.stdout).unwrap();
        assert_eq!(state["jobs"]["job"]["status"]=="complete",point=="blocklist");
        assert!(child("never").status.success());
        assert_eq!(f.q.state().unwrap()["jobs"]["job"]["status"],"complete");
        assert_eq!(f.q.store.capture("s",REL).unwrap()["status"],"deleted");
    }
}

#[test]
fn uncertain_inspection_rejects_the_whole_plan() {
    let f = Fixture::new();
    let db = Connection::open(&f.q.intake).unwrap();
    db.execute("UPDATE observation SET payload=?1",
            [br#"{"text":"\u0065rased","n":1e9999}"#.as_slice()]).unwrap();
    let e =
        f.q.request(&json!({"kind":"text","text":"erased"}), &f.map,
                Some("uncertain"), &mut |_| Ok(())).unwrap_err();
    assert_eq!(e.0,"deletion_inspection_unavailable");
    assert_eq!(f.q.state().unwrap()["jobs"],json!({}));
    assert_eq!(f.q.store.state().unwrap()["blocked_paths"],json!([]));
}
#[test]
fn file_plan_includes_hardlink_generations_without_content_merging() {
    let f = Fixture::new();
    let alias = "sessions/rollout-alias.jsonl";
    fs::hard_link(f.root.join("original").join(REL),
            f.root.join("original").join(alias)).unwrap();
    let r = f.q.store.capture("s", alias).unwrap()["receipt"].clone();
    let mut mapping = f.map.clone();
    mapping[f.q.store.root.join(r["staging_relative"].as_str().unwrap()).to_str().unwrap()]
        = json!(["alias"]);
    let job =
        f.q.request(&json!({"kind":"file","source":"s","relative":REL}),
                &mapping, Some("alias-job"), &mut |_| Ok(())).unwrap();
    assert_eq!(job["paths"].as_array().unwrap().len(),2);
    assert_eq!(job["inputs"],json!(["alias","input-one","input-two"]));
    f.q.resume(&mut f.executor(), &mut |_| Ok(())).unwrap();
    assert_eq!(f.q.store.capture("s",alias).unwrap()["status"],"deleted");
}
#[test]
fn subprocess_excision_requires_actual_report_and_failure_keeps_intent() {
    let f = Fixture::new();
    f.request("text");
    let intake = f.root.join("intake-stub");
    let archive = f.root.join("archive-stub");
    fs::write(&intake,
            "#!/bin/sh\nread request\ntest -f /proc/self/fd/$1 || exit 3\nprintf '%s\\n' '{\"status\":\"deleted\"}'\n").unwrap();
    fs::set_permissions(&intake, fs::Permissions::from_mode(0o700)).unwrap();
    let cfg =
        |report: &str|
            {
                fs::write(&archive,
                        format!("#!/bin/sh\nprintf '%s\\n' '{report}'\n")).unwrap();
                fs::set_permissions(&archive,
                        fs::Permissions::from_mode(0o700)).unwrap();
                uasf_del::process::Commands { completion:None,
                    timeout: std::time::Duration::from_secs(2),
                    intake: vec![intake.to_string_lossy().into_owned(),"{lock_fd}".into()],
                    archive: vec![archive.to_string_lossy().into_owned(),"{scope}".into()],
                }
            };
    for report in
        [r#"{"dry_run":true}"#, r#"{"status":"queued","dry_run":false}"#,
                r#"{"error":"payload must not be retained","dry_run":false}"#]
        {
        assert_eq!(f.q.resume(&mut cfg(report),&mut
        |_|Ok(())).unwrap()["status"],"pending");
        let state = f.q.state().unwrap();
        assert_eq!(state["jobs"]["job"]["error"],"RuntimeError");
        assert_eq!(state["jobs"]["job"]["pending_text"],"erase");
        assert!(!state.to_string().contains("payload must not"));
    }
    assert_eq!(f.q.resume(&mut cfg(r#"{"dry_run":false}"#),&mut
    |_|Ok(())).unwrap()["status"],"complete");
}

#[test]
fn shared_json_corpus_planning_never_misses_deleted_text() {
    let cases = uasf_json::from_str(include_str!("../../uasf-json/tests/inspection.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let f = Fixture::new();
        let data = c["input"].as_str().unwrap().as_bytes();
        let staged = PathBuf::from(f.map.as_object().unwrap().keys().next().unwrap());
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&staged,data).unwrap();
        let mut state = f.q.store.state().unwrap();
        for receipt in state["receipts"].as_object_mut().unwrap().values_mut() {
            receipt["length"] = json!(data.len()); receipt["sha256"] = json!(sha(data));
        }
        f.q.store.save(&state).unwrap();
        let db = Connection::open(&f.q.intake).unwrap();
        db.execute("UPDATE observation SET payload=?1",[data]).unwrap();
        let result = f.q.request(&c["selector"], &f.map, Some("corpus"), &mut |_| Ok(()));
        if c["decode_ok"] == true {
            let job = result.unwrap();
            assert_eq!(job["receipts"],json!(["receipt"]));
            assert_eq!(job["targets"].as_array().unwrap().len(),1);
        } else {
            assert_eq!(result.unwrap_err().0,"deletion_inspection_unavailable","{}",c["name"]);
            assert_eq!(f.q.state().unwrap()["jobs"],json!({}));
            assert_eq!(f.q.store.state().unwrap()["blocked_paths"],json!([]));
        }
    }
}

#[test]
fn hanging_intake_and_archive_are_bounded_reaped_and_retryable() {
    use std::{os::fd::AsRawFd, time::{Duration, Instant}};
    for phase in ["intake", "archive", "stdin", "descendant"] {
        let f = Fixture::new();
        f.request("text");
        let pidfile = f.root.join("pids");
        let hang = f.root.join("hang.py");
        fs::write(&hang, format!("import os,time\npid=os.fork()\nwith open({:?},'a') as f: f.write(str(os.getpid())+'\\n')\n{}\ntime.sleep(60)\n",pidfile.to_str().unwrap(), if phase == "descendant" { "if pid: os._exit(0)" } else { "pass" })).unwrap();
        let good = f.root.join("ok.py");
        fs::write(&good,"import sys,json\nif len(sys.argv)>1:\n print('{\"dry_run\":false}')\nelse:\n json.load(sys.stdin)\n print('{\"status\":\"deleted\"}')\n").unwrap();
        let good_intake = vec!["/usr/bin/python3".into(),good.to_str().unwrap().into()];
        let good_archive = vec!["/usr/bin/python3".into(),good.to_str().unwrap().into(),"{scope}".into()];
        let mut commands = uasf_del::process::Commands { completion:None,
            intake: if phase == "archive" { good_intake.clone() } else { vec!["/usr/bin/python3".into(),hang.to_str().unwrap().into(),"{lock_fd}".into()] },
            archive: if phase == "archive" { vec!["/usr/bin/python3".into(),hang.to_str().unwrap().into(),"{scope}".into()] } else { good_archive.clone() },
            timeout: Duration::from_millis(300),
        };
        if phase == "stdin" {
            // Exceed pipe capacity while the receiver never reads a byte.
            let mut state = f.q.state().unwrap();
            state["jobs"]["job"]["receipts"] = json!(["r".repeat(512*1024)]);
            uasf_del::atomic(&f.q.root.join("jobs.json"), &state).unwrap();
        }
        let start = Instant::now();
        assert_eq!(f.q.resume(&mut commands,&mut |_|Ok(())).unwrap()["status"],"pending");
        assert!(start.elapsed() < Duration::from_secs(3),"{phase}");
        let state = f.q.state().unwrap();
        assert_eq!(state["jobs"]["job"]["error"],"TimeoutError","{phase}");
        assert_eq!(f.q.store.capture("s",REL).unwrap()["status"],"pending_deletion");
        // Both original and forked child held the capability/pipe. A separate
        // open can now acquire the lock, and every process is gone or reaped zombie.
        let lock = fs::OpenOptions::new().read(true).write(true).open(f.q.root.join("lock")).unwrap();
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB) },0);
        drop(lock);
        for pid in fs::read_to_string(&pidfile).unwrap().lines() {
            let path = format!("/proc/{pid}/stat");
            let deadline = Instant::now()+Duration::from_secs(2);
            while fs::read_to_string(&path).is_ok_and(|s| !s.split_once(") ").unwrap().1.starts_with('Z')) {
                assert!(Instant::now()<deadline,"child still running: {pid}");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        commands.intake = good_intake; commands.archive = good_archive;
        commands.timeout = Duration::from_secs(2);
        let cfg = uasf_json::from_slice(&fs::read(f.root.join("config.json")).unwrap()).unwrap();
        let reopened = uasf_del::process::config(&cfg,false).unwrap();
        assert_eq!(reopened.resume(&mut commands,&mut |_|Ok(())).unwrap()["status"],"complete");
    }
}

#[test]
fn blocklist_hook_failure_and_all_steps_pending_recover_without_effects() {
    let f = Fixture::new(); f.request("text");
    assert_eq!(f.q.resume(&mut f.executor(),&mut |p| if p=="blocklist" {Err(Error("OSError"))} else {Ok(())}).unwrap()["status"],"pending");
    let state=f.q.state().unwrap();
    assert_eq!(state["jobs"]["job"]["steps"],json!(STEPS));
    let before=fs::read(f.root.join("effects")).unwrap();
    assert_eq!(f.q.resume(&mut Effects{root:f.root.clone(),fail:true},&mut |_|Ok(())).unwrap()["status"],"complete");
    assert_eq!(fs::read(f.root.join("effects")).unwrap(),before);
    assert!(f.q.state().unwrap()["jobs"]["job"].get("error").is_none());
}

#[test]
fn integrated_completion_remains_pending_across_restart_and_cannot_be_removed() {
    let f=Fixture::new();
    // A fresh integrated journal is required: an old binding cannot be upgraded silently.
    let mut cfg:Value=uasf_json::from_slice(&fs::read(f.root.join("config.json")).unwrap()).unwrap();
    cfg["integration"]=json!({"core":{"index":f.root.join("core/index.sqlite"),"persistent":f.root.join("core/persistent.sqlite")},"completion_command":["/bin/false"]});
    assert!(uasf_del::process::config(&cfg,true).is_err());
    cfg["root"]=json!(f.root.join("integrated"));cfg["staging"]=json!(f.root.join("integrated-staging"));cfg["intake"]=json!(f.root.join("integrated-intake.db"));
    fs::copy(f.root.join("intake.db"),cfg["intake"].as_str().unwrap()).unwrap();
    let q=uasf_del::process::config(&cfg,true).unwrap();
    q.request(&json!({"kind":"session","source":"s","session":"session"}),&json!({}),Some("j"),&mut |_|Ok(())).unwrap();
    let core=uasf_core::Core::open(f.root.join("new-index.sqlite"),f.root.join("core/persistent.sqlite")).unwrap();
    assert_eq!(core.query(&json!({"op":"sessions"})).unwrap()["reasons"],json!(["pending_deletion"]));
    assert_eq!(q.resume(&mut f.executor(),&mut |_|Ok(())).unwrap()["status"],"pending");
    let q=uasf_del::process::config(&cfg,false).unwrap();
    let mut removed=cfg.clone();removed.as_object_mut().unwrap().remove("integration");
    assert!(uasf_del::process::config(&removed,false).unwrap().resume(&mut f.executor(),&mut |_|Ok(())).is_err());
    struct Complete;
    impl Executor for Complete {
        fn erase(&mut self,_:&Value,_:&Lease)->Result<()>{panic!("acknowledged effect repeated")}
        fn excise(&mut self,_:&str)->Result<()>{panic!("acknowledged effect repeated")}
        fn complete_integration(&mut self,job:&Value)->Result<()>{assert_eq!(job["id"],"j");Ok(())}
    }
    let done=q.resume(&mut Complete,&mut |_|Ok(())).unwrap();assert_eq!(done["status"],"complete");
    assert_eq!(done["jobs"]["j"]["steps"],json!(["staging","intake","cache","archive","integration","blocklist"]));
    assert_ne!(core.query(&json!({"op":"sessions"})).unwrap()["reasons"],json!(["pending_deletion"]));
}
