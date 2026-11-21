mod support;
use support::Fixture;
use std::{
    fs, os::unix::fs::{symlink, PermissionsExt, MetadataExt},
    path::Path,
};
use uasf_json::{json, Value};
use uasf_feeder::{io, sha, unseal, seal, LIMIT, canonical};
const DATA: &[u8] = b"{\"x\":1}\n";
#[test]
fn prefix_original_hash_and_metadata() {
    let f = Fixture::new("codex", b"{\"x\":1}\n{\"tail\":");
    let sent = f.emit();
    let got = f.accept(&sent);
    assert_eq!(fs::read(got["path"].as_str().unwrap()).unwrap(),DATA);
    let m = &got["receipt"];
    let original = fs::metadata(f.source()).unwrap();
    assert_eq!(m["device"], original.dev());
    assert_eq!(m["inode"], original.ino());
    assert_eq!(m["source_length"], original.len());
    assert_eq!(m["mtime_ns"], json!(original.mtime() as i128 * 1_000_000_000 + original.mtime_nsec() as i128));
    assert_eq!(m["ctime_ns"], json!(original.ctime() as i128 * 1_000_000_000 + original.ctime_nsec() as i128));
    assert_eq!(m["config_sha256"], sha(&canonical(&f.cfg)));
    assert_eq!(m["source_sha256"],sha(&fs::read(f.source()).unwrap()));
    assert_eq!(m["withheld_length"],8);
    assert_eq!(m["sha256"],sha(DATA));
}
#[test]
fn object_lines_only_and_no_complete_line() {
    for data in
        [b"[]\n".as_slice(), b"null\n", b"\n", b"{bad}\n", b"{\"x\":NaN}\n",
                b"{\"x\":1,\"x\":2}\n", b"{\"n\":1e9999}\n", b"\xff\n"] {
        let f = Fixture::new("codex", data);
        assert!(f.endpoint("producer").emit("s",&f.rel,&mut
        |_|Ok(())).is_err());
        assert_eq!(fs::read_dir(f.root.join("inbox")).unwrap().count(),0);
    }
    let f = Fixture::new("codex", b"{\"tail\":");
    assert_eq!(f.emit()["status"],"not_captured");
    let f = Fixture::new("codex", b"");
    assert_eq!(f.accept(&f.emit())["receipt"]["length"],0);
}
#[test]
fn registered_paths_and_exact_normalization() {
    let f = Fixture::new("codex", DATA);
    for p in
        ["sessions//rollout-one.jsonl",
                "sessions/../sessions/rollout-one.jsonl",
                "./sessions/rollout-one.jsonl", "/sessions/x.jsonl",
                "sessions\\x.jsonl", "sessions/rollout-\u{7f}.jsonl",
                "other.jsonl"] {
        assert!(f.endpoint("producer").emit("s",p,&mut |_|Ok(())).is_err());
    }
    assert!(f.endpoint("producer").emit("unknown",&f.rel,&mut
    |_|Ok(())).is_err());
    let mut cfg = f.cfg.clone();
    cfg["sources"]["s"].as_object_mut().unwrap().remove("pattern");
    cfg["sources"]["s"]["files"] = json!([f.rel]);
    io::atomic(&f.root.join("config.json"), &canonical(&cfg), 0o600, true,
            false).unwrap();
    assert!(f.endpoint("producer").emit("s",&f.rel,&mut |_|Ok(())).is_err());
}
#[test]
fn symlinks_fifo_and_component_replacement() {
    let f = Fixture::new("codex", DATA);
    let root = f.root.join("original");
    symlink(f.source(), root.join("sessions/rollout-link.jsonl")).unwrap();
    symlink(root.join("sessions"), root.join("sessions/link")).unwrap();
    for p in
        ["sessions/rollout-link.jsonl", "sessions/link/rollout-one.jsonl"] {
        assert!(f.endpoint("producer").emit("s",p,&mut |_|Ok(())).is_err());
    }
    let fifo = root.join("sessions/rollout-fifo.jsonl");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe{libc::mkfifo(c.as_ptr(),0o600)},0);
    assert_eq!(f.endpoint("producer").emit("s","sessions/rollout-fifo.jsonl",&mut
    |_|Ok(())).unwrap_err().0,"not_regular");
}
#[test]
fn mutation_and_named_replacement_between_reads() {
    for replace in [false, true] {
        let f = Fixture::new("codex", DATA);
        let r =
            f.endpoint("producer").emit("s", &f.rel,
                &mut |point|
                        {
                            if point == "read" {
                                if replace {
                                    fs::remove_file(f.source())?;
                                    symlink(f.root.join("key"), f.source())?;
                                } else { fs::write(f.source(), b"{\"y\":2}\n")?; }
                            }
                            Ok(())
                        });
        assert_eq!(r.unwrap_err().0,"unstable");
        assert_eq!(fs::read_dir(f.root.join("inbox")).unwrap().count(),0);
    }
}
#[test]
fn full_prefix_rewrite_creates_ordered_generation() {
    let f = Fixture::new("codex", DATA);
    let a = f.emit();
    f.accept(&a);
    fs::write(f.source(), b"{\"x\":2}\n{\"append\":1}\n").unwrap();
    let b = f.emit();
    let got = f.accept(&b);
    assert_eq!(b["sequence"],2);
    assert_eq!(got["receipt"]["previous"],a["generation"]);
    assert_eq!(got["receipt"]["source_sha256"],sha(&fs::read(f.source()).unwrap()));
}
#[test]
fn authenticated_metadata_rejects_forgery() {
    let f = Fixture::new("codex", DATA);
    let a = f.emit();
    for (k, v) in
        [("path", json!("../outside")), ("source", json!("unknown")),
                ("kind", json!("claude")), ("length", json!(99)),
                ("sha256", json!("0".repeat(64))),
                ("source_length", json!(LIMIT+1)), ("sequence", json!(true)),
                ("config_sha256", json!("0".repeat(64))),
                ("policy_revision", json!(1))] {
        let n = f.altered(&a, |m| m[k] = v, None);
        assert!(f.endpoint("collector").accept(&n,&mut
        |_|Ok(())).is_err(),"{k}");
    }
    let n = f.altered(&a, |_| {}, Some(b"{\"x\":2}\n"));
    assert!(f.endpoint("collector").accept(&n,&mut |_|Ok(())).is_err());
    let mut raw = f.wire(&a);
    *raw.last_mut().unwrap() ^= 1;
    assert_eq!(unseal(&raw,&f.key,&f.cfg).unwrap_err().0,"authentication_failed");
}
#[test]
fn order_replay_previous_and_restart() {
    let f = Fixture::new("codex", DATA);
    let a = f.emit();
    let b = f.emit();
    assert_eq!(f.endpoint("collector").accept(b["record"].as_str().unwrap(),&mut
    |_|Ok(())).unwrap_err().0,"replay_or_out_of_order");
    f.accept(&a);
    let replay =
        f.endpoint("producer").retry(a["generation"].as_str().unwrap()).unwrap();
    assert!(f.endpoint("collector").accept(replay["record"].as_str().unwrap(),&mut
    |_|Ok(())).is_err());
    let bad = f.altered(&b, |m| m["previous"] = json!("f".repeat(32)), None);
    assert!(f.endpoint("collector").accept(&bad,&mut |_|Ok(())).is_err());
    f.accept(&b);
    assert_eq!(f.endpoint("collector").materialize(b["generation"].as_str().unwrap()).unwrap()["status"],"validated");
}
#[test]
fn bounded_source_and_inclusive_record_limit() {
    let f = Fixture::new("codex", DATA);
    let a = f.emit();
    let (m, _) = unseal(&f.wire(&a), &f.key, &f.cfg).unwrap();
    let mut body = vec![b'x';LIMIT as usize];
    assert_eq!(seal(&m,&body,&f.key).unwrap_err().0,"record_over_32_MiB");
    body.truncate(LIMIT as usize - f.wire(&a).len() + DATA.len());
    assert_eq!(seal(&m,&body,&f.key).unwrap().len()as u64,LIMIT);
    body.push(b'x');
    assert!(seal(&m,&body,&f.key).is_err());
    fs::File::create(f.source()).unwrap().set_len(LIMIT + 1).unwrap();
    assert_eq!(f.endpoint("producer").emit("s",&f.rel,&mut
    |_|Ok(())).unwrap_err().0,"over_size_limit");
}
#[test]
fn incoming_symlink_hardlink_mode_and_validated_tampering() {
    let f = Fixture::new("codex", DATA);
    let a = f.emit();
    let p = f.root.join("inbox").join(a["record"].as_str().unwrap());
    fs::hard_link(&p, f.root.join("alias")).unwrap();
    assert_eq!(f.endpoint("collector").accept(a["record"].as_str().unwrap(),&mut
    |_|Ok(())).unwrap_err().0,"record_mode_or_links");
    fs::remove_file(f.root.join("alias")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(f.endpoint("collector").accept(a["record"].as_str().unwrap(),&mut
    |_|Ok(())).is_err());
    fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
    let got = f.accept(&a);
    let path = Path::new(got["path"].as_str().unwrap());
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(path, b"changed").unwrap();
    assert_eq!(f.endpoint("collector").materialize(a["generation"].as_str().unwrap()).unwrap_err().0,"validated_copy_tampered");
}
#[test]
fn deletion_purges_body_new_inode_alias_retry_and_stale_record() {
    let f = Fixture::new("codex", DATA);
    let a = f.emit();
    let got = f.accept(&a);
    let queued =
        f.endpoint("producer").retry(a["generation"].as_str().unwrap()).unwrap();
    let alias = "sessions/rollout-alias.jsonl";
    fs::hard_link(f.source(), f.root.join("original").join(alias)).unwrap();
    let r = f.endpoint("collector").delete("s", &f.rel).unwrap();
    assert_eq!(r["producer_purge"],"required");
    assert_eq!(r["downstream_deletion"],"required");
    assert!(!Path::new(got["path"].as_str().unwrap()).exists());
    assert!(!f.root.join("inbox").join(queued["record"].as_str().unwrap()).exists());
    assert_eq!(f.endpoint("producer").emit("s",alias,&mut
    |_|Ok(())).unwrap_err().0,"deleted");
    assert_eq!(f.endpoint("producer").retry(a["generation"].as_str().unwrap()).unwrap_err().0,"deleted");
    fs::remove_file(f.source()).unwrap();
    fs::write(f.source(), DATA).unwrap();
    assert_eq!(f.endpoint("producer").emit("s",&f.rel,&mut
    |_|Ok(())).unwrap_err().0,"deleted");
    let s = io::json(&f.root.join("producer/state.json")).unwrap();
    assert!(s["records"][a["generation"].as_str().unwrap()]["meta"].get("sha256").is_none());
}
#[test]
fn policy_rollback_missing_state_and_outbox_integrity() {
    let f = Fixture::new("codex", DATA);
    let old = fs::read(f.root.join("policy/deleted.json")).unwrap();
    f.endpoint("collector").delete("s", &f.rel).unwrap();
    f.endpoint("producer").purge().unwrap();
    io::atomic(&f.root.join("policy/deleted.json"), &old, 0o640, true,
            false).unwrap();
    assert_eq!(f.endpoint("producer").purge().unwrap_err().0,"policy_rollback");
    let f = Fixture::new("codex", DATA);
    let a = f.emit();
    let p =
        f.root.join("producer/records").join(format!("{}.rec",a["generation"].as_str().unwrap()));
    fs::write(p, b"bad").unwrap();
    assert!(f.endpoint("producer").retry(a["generation"].as_str().unwrap()).is_err());
    fs::remove_file(f.root.join("producer/state.json")).unwrap();
    assert!(f.endpoint("producer").purge().is_err());
    assert!(uasf_feeder::initialize(&f.root.join("producer"),&f.cfg,&f.key,"producer").is_err());
}
#[test]
fn interrupted_reservation_and_accept_recover_in_new_process() {
    if let Ok(root) = std::env::var("FEEDER_CRASH_ROOT") {
        let point = std::env::var("FEEDER_CRASH_POINT").unwrap();
        let r = Path::new(&root);
        let role = if point == "reserved" { "producer" } else { "collector" };
        let e =
            uasf_feeder::Endpoint::new(&r.join("config.json"), &r.join("key"),
                    &r.join(role), &r.join("policy"), &r.join("inbox"),
                    role).unwrap();
        let mut hook =
            |p: &str| { if p == point { std::process::exit(77); } Ok(()) };
        if role == "producer" {
            e.emit("s", "sessions/rollout-one.jsonl", &mut hook).unwrap();
        } else {
            e.accept(&std::env::var("FEEDER_CRASH_RECORD").unwrap(),
                    &mut hook).unwrap();
        }
        return;
    }
    for point in ["reserved", "copied", "committed"] {
        let f = Fixture::new("codex", DATA);
        let sent = if point == "reserved" { Value::Null } else { f.emit() };
        let out =
            std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact",
                                            "interrupted_reservation_and_accept_recover_in_new_process"]).env("FEEDER_CRASH_ROOT",
                                &f.root).env("FEEDER_CRASH_POINT",
                            point).env("FEEDER_CRASH_RECORD",
                        sent["record"].as_str().unwrap_or("")).output().unwrap();
        assert_eq!(out.status.code(),Some(77));
        let state = io::json(&f.root.join("producer/state.json")).unwrap();
        let g = state["records"].as_object().unwrap().keys().next().unwrap();
        let out =
            if point == "reserved" {
                f.cli("producer", "retry", &[g]).output().unwrap()
            } else if point == "copied" {
                f.cli("collector", "accept",
                            &[sent["record"].as_str().unwrap()]).output().unwrap()
            } else {
                f.cli("collector", "materialize", &[g]).output().unwrap()
            };
        assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stdout));
    }
}
#[test]
fn concurrent_producers_reserve_distinct_ordered_generations() {
    let f = Fixture::new("codex", DATA);
    let mut a =
        f.cli("producer", "emit",
                        &["s",
                                    &f.rel]).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut b =
        f.cli("producer", "emit",
                        &["s",
                                    &f.rel]).stdout(std::process::Stdio::piped()).spawn().unwrap();
    assert!(a.wait().unwrap().success());
    assert!(b.wait().unwrap().success());
    let s = io::json(&f.root.join("producer/state.json")).unwrap();
    assert_eq!(s["records"].as_object().unwrap().len(),2);
    assert_eq!(s["heads"].as_object().unwrap().values().next().unwrap()["sequence"],2);
}
#[test]
fn staging_bridge_preserves_original_and_holds_deletion_admission() {
    for kind in ["claude", "codex"] {
        let f = Fixture::new(kind, DATA);
        let sent = f.emit();
        let g = sent["generation"].as_str().unwrap();
        f.accept(&sent);
        let e = f.endpoint("collector");
        let stage = f.root.join("staging");
        assert!(e.publish(g,&stage,false).is_err());
        let r = e.publish(g, &stage, true).unwrap();
        assert_eq!(r["status"],"published");
        assert_eq!(r["link"]["original_provenance"]["sha256"],sha(DATA));
        assert_eq!(fs::read(stage.join(r["link"]["staging_relative"].as_str().unwrap())).unwrap(),DATA);
        assert_eq!(e.publish(g,&stage,true).unwrap()["staging"]["status"],"unchanged");
        e.delete("s", &f.rel).unwrap();
        assert_eq!(e.publish(g,&stage,true).unwrap_err().0,"deleted");
        assert!(stage.join(r["link"]["staging_relative"].as_str().unwrap()).exists());
    }
}
#[test]
fn marker_json_and_oversized_depth_fail_closed() {
    let f =
        Fixture::new("codex",
            b"{\"$serde_json::private::Number\":\"\\u0031\"}\n");
    assert_eq!(f.accept(&f.emit())["status"],"validated");
    let raw = format!("{{\"x\":{}0{}}}\n","[".repeat(130),"]".repeat(130));
    let f = Fixture::new("codex", raw.as_bytes());
    assert_eq!(f.endpoint("producer").emit("s",&f.rel,&mut
    |_|Ok(())).unwrap_err().0,"invalid_json");
}
#[test]
fn bound_staging_pending_barrier_and_byte_admission() {
    let f = Fixture::new("codex", DATA);
    let sent = f.emit();
    f.accept(&sent);
    let e = f.endpoint("collector");
    let stage = f.root.join("staging");
    let store =
        uasf_staging::Store::new(stage.clone(), e.registry()).unwrap();
    let queue = f.root.join("queue");
    fs::create_dir(&queue).unwrap();
    io::atomic(&queue.join("lock"), b"", 0o600, false, false).unwrap();
    io::atomic(&stage.join("deletion-binding.json"),
            &canonical(&json!({"version":1,"kind":"staging","resource":stage,"root":queue,"owner_uid":unsafe{libc::geteuid()}})),
            0o600, false, false).unwrap();
    let mut journal =
        json!({"version":1,"binding":{"staging":stage},"jobs":{"j":{"status":"pending"}}});
    io::atomic(&queue.join("jobs.json"), &canonical(&journal), 0o600, true,
            false).unwrap();
    let g = sent["generation"].as_str().unwrap();
    assert_eq!(e.publish(g,&stage,false).unwrap_err().0,"pending_deletion");
    journal["jobs"] = json!({});
    io::atomic(&queue.join("jobs.json"), &canonical(&journal), 0o600, true,
            false).unwrap();
    let accepted = e.materialize(g).unwrap();
    let path = Path::new(accepted["path"].as_str().unwrap());
    let root = e.root.join("validated/s");
    let rel = path.strip_prefix(&root).unwrap().to_str().unwrap();
    let r: uasf_staging::Result<Value> =
        store.with_capture(|lease| lease.capture("s", rel, 99, &sha(DATA)));
    assert_eq!(r.unwrap()["reason"],"admission_mismatch");
    assert!(store.state().unwrap()["receipts"].as_object().unwrap().is_empty());
    assert_eq!(e.publish(g,&stage,false).unwrap()["status"],"published");
}
#[test]
fn malformed_state_refused_and_untracked_copies_scrubbed() {
    let f = Fixture::new("codex", DATA);
    fs::write(f.root.join("producer/records/orphan.rec"),
            b"sensitive-synthetic").unwrap();
    fs::write(f.root.join("producer/tmp/orphan"),
            b"sensitive-synthetic").unwrap();
    f.endpoint("producer").purge().unwrap();
    assert_eq!(fs::read_dir(f.root.join("producer/records")).unwrap().count(),0);
    assert_eq!(fs::read_dir(f.root.join("producer/tmp")).unwrap().count(),0);
    let mut state = io::json(&f.root.join("producer/state.json")).unwrap();
    state["records"]["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"] = json!(false);
    io::atomic(&f.root.join("producer/state.json"), &canonical(&state), 0o600,
            true, false).unwrap();
    assert_eq!(f.endpoint("producer").purge().unwrap_err().0,"state_binding");
}
