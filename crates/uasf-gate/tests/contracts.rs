use std::{
    cell::RefCell, fs, path::PathBuf, os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};
use uasf_json::{json, Value};
use uasf_core::model::sha;
use uasf_gate::{files, Bridge, Gate, Result, LIMIT, transport};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Mock {
    responses: RefCell<Value>,
    calls: RefCell<Vec<String>>,
}
impl Bridge for Mock {
    fn call(&self, r: &Value) -> Result<Value> {
        self.calls.borrow_mut().push(r["op"].as_str().unwrap().into());
        Ok(self.responses.borrow()[r["op"].as_str().unwrap()].clone())
    }
}
struct Fixture {
    root: PathBuf,
    gate: Gate<Mock>,
    data: Vec<u8>,
    reference: Value,
    path: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
}
fn write(p: &std::path::Path, v: &Value) {
    fs::write(p, v.to_string()).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}
fn dir(p: &std::path::Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("uasf-gate-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        dir(&root);
        let staging = root.join("staging");
        for d in
            ["", "tmp", "claude", "codex", "codex/sessions",
                    "codex/sessions/g"] {
            dir(&staging.join(d));
        }
        let archive = root.join("archive");
        dir(&archive);
        write(&archive.join("archive.db"), &json!({}));
        let data =
            b"{\"ordinal\":0,\"text\":\"synthetic\"}\n{\"ordinal\":1}\n".to_vec();
        let path = staging.join("codex/sessions/g/fixture.jsonl");
        fs::write(&path, &data).unwrap();
        fs::set_permissions(&path,
                fs::Permissions::from_mode(0o400)).unwrap();
        let receipt =
            json!({"status":"active","generation":"g","source":"original","kind":"codex","original_path":root.join("originals/sessions/fixture.jsonl"),
                    "staging_relative":"codex/sessions/g/fixture.jsonl","length":data.len(),"sha256":sha(&data),"file_generation":"file-1","relation":"initial"});
        write(&staging.join("state.json"),
            &json!({"version":1,"receipts":{"g":receipt}}));
        let registry = root.join("registry.json");
        write(&registry,
            &json!({"version":1,"sources":{"archive":{"archive":archive,"collection_roots":[staging.join("claude"),staging.join("codex")]}}}));
        let reference =
            json!({"source":"archive","archive_id":"archive-1","source_binding":"binding","input":"input","revision":1,"chain":1,"start":0,"end":data.len()});
        let end = data.iter().position(|b| *b == b'\n').unwrap() + 1;
        let mut anchor = reference.clone();
        anchor["end"] = json!(end);
        let responses =
            json!({"list":{"status":"ok","archive_id":"archive-1","source_binding":"binding","observations":[{"path":path,"input":"input","revision":1,"class":"first"}],"inputs":[{"input":"input","revisions":[{"number":1,"chain":1,"length":data.len()}]}]},
                    "verify_capture":{"status":"ok","reference":reference,"length":data.len(),"sha256":sha(&data)},
                    "read":{"status":"ok","reference":reference,"hex":uasf_gate::encode_hex(&data)},
                    "anchor":{"status":"ok","reference":anchor,"ordinal":0,"sha256":sha(&data[..end]),"window_id":"window"},
                    "excisions":{"status":"ok","invalidate_all":false}});
        let gate =
            Gate {
                bridge: Mock {
                    responses: RefCell::new(responses),
                    calls: RefCell::new(vec![]),
                },
                registry,
                staging,
                sources: json!({"version":1,"sources":{"original":{"root":root.join("originals"),"kind":"codex"}}}),
                source: "archive".into(),
            };
        Self { root, gate, data, reference, path }
    }
    fn request(&self, extra: Value) -> Value {
        let mut r = json!({"op":"read","generation":"g"});
        r.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.gate.handle(&r)
    }
    fn receipt(&self, k: &str, v: Value) {
        let path = self.gate.staging.join("state.json");
        let mut s = files::json(&path).unwrap();
        s["receipts"]["g"][k] = v;
        write(&path, &s);
    }
    fn queue(&self, status: &str) {
        let root = self.root.join("queue");
        dir(&root);
        write(&self.gate.staging.join("deletion-binding.json"),
            &json!({"version":1,"kind":"staging","resource":self.gate.staging,"root":root,"owner_uid":files::uid()}));
        write(&root.join("jobs.json"),
            &json!({"version":1,"binding":{"staging":self.gate.staging},"jobs":{"one":{"status":status}}}));
    }
}
fn denied(v: Value, status: &str) {
    assert_eq!(v["status"],status,"{v}");
    assert!(v.get("hex").is_none());
    assert!(v.get("reference").is_none());
}
#[test]
fn full_read_resolve_anchor() {
    let f = Fixture::new();
    let r = f.request(json!({"reference":f.reference}));
    assert_eq!(r["status"],"verified");
    assert_eq!(uasf_gate::decode_hex(r["hex"].as_str().unwrap()).unwrap(),f.data);
    assert_eq!(*f.gate.bridge.calls.borrow(),vec!["list","verify_capture","read","excisions"]);
    assert_eq!(r["capture_time_verified"],false);
    assert_eq!(f.request(json!({"op":"resolve","expected_sha256":sha(&f.data)}))["status"],"verified");
    assert_eq!(f.request(json!({"op":"anchor","ordinal":0}))["status"],"verified");
    denied(f.request(json!({"op":"anchor","ordinal":true})), "unsupported");
}
#[test]
fn every_saved_reference_field_bound() {
    let f = Fixture::new();
    for k in f.reference.as_object().unwrap().keys() {
        let mut r = f.reference.clone();
        r[k] = json!("forged");
        denied(f.request(json!({"reference":r})), "mismatch");
    }
    assert!(f.gate.bridge.calls.borrow().iter().all(|c|c=="list"));
}
#[test]
fn consumer_cannot_inject_authority() {
    let f = Fixture::new();
    for k in
        ["receipt", "sha256", "path", "registry", "source",
                "observation_path", "allow_unverified_raw"] {
        let mut v = json!({});
        v[k] = json!("forged");
        denied(f.request(v), "unsupported");
    }
    for r in
        [Value::Null, json!([]), json!({}), json!({"op":"list"}),
                json!({"op":"read","generation":true})] {
        denied(f.gate.handle(&r), "unsupported");
    }
    assert!(f.gate.bridge.calls.borrow().is_empty());
}
#[test]
fn absent_deleted_and_inactive_receipts() {
    let f = Fixture::new();
    denied(f.request(json!({"generation":"missing"})), "unverified");
    f.receipt("status", json!("deleted"));
    denied(f.request(json!({})), "deleted");
    f.receipt("status", json!("inactive"));
    denied(f.request(json!({})), "unverified");
    assert!(f.gate.bridge.calls.borrow().is_empty());
}
#[test]
fn receipt_source_path_kind_length_binding() {
    for (k, v, status) in
        [("source", json!("missing"), "out_of_scope"),
                ("original_path", json!("/outside/file"), "out_of_scope"),
                ("staging_relative", json!("../outside"), "out_of_scope"),
                ("kind", json!("claude"), "out_of_scope"),
                ("length", json!(LIMIT+1), "unsupported"),
                ("length", json!(true), "unsupported"),
                ("generation", json!("another"), "mismatch")] {
        let f = Fixture::new();
        f.receipt(k, v);
        denied(f.request(json!({})), status);
        assert!(f.gate.bridge.calls.borrow().is_empty());
    }
}
#[test]
fn full_prefix_and_returned_bytes_rehashed() {
    let f = Fixture::new();
    fs::set_permissions(&f.path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut bad = f.data.clone();
    bad[0] = b'X';
    fs::write(&f.path, &bad).unwrap();
    denied(f.request(json!({})), "mismatch");
    assert!(f.gate.bridge.calls.borrow().is_empty());
    fs::write(&f.path, &f.data).unwrap();
    f.gate.bridge.responses.borrow_mut()["read"]["hex"] =
        json!(uasf_gate::encode_hex(&bad));
    denied(f.request(json!({})), "mismatch");
}
#[test]
fn verified_response_binding_is_required() {
    for op in ["verify_capture", "read"] {
        let f = Fixture::new();
        f.gate.bridge.responses.borrow_mut()[op]["reference"]["input"] =
            json!("other");
        denied(f.request(json!({})), "mismatch");
    }
}
#[test]
fn bridge_failure_statuses_fail_closed() {
    for (state, want) in
        [("excised", "deleted"), ("mis_captured", "mismatch"),
                ("content_changed", "mismatch"),
                ("out_of_scope", "out_of_scope"),
                ("unsupported", "unsupported"), ("not_found", "unverified"),
                ("surprise", "unverified")] {
        let f = Fixture::new();
        f.gate.bridge.responses.borrow_mut()["read"] =
            json!({"status":state,"hex":"00"});
        denied(f.request(json!({})), want);
    }
}
#[test]
fn excision_after_read_blocks_delivery() {
    let f = Fixture::new();
    for state in [json!(true), Value::Null, json!(0)] {
        f.gate.bridge.responses.borrow_mut()["excisions"]["invalidate_all"] =
            state;
        denied(f.request(json!({})), "deleted");
    }
}
#[test]
fn foreign_anchor_and_invalid_ranges() {
    for r in
        [json!({"start":true,"end":1}), json!({"start":0,"end":999}),
                json!({"start":2,"end":1})] {
        let f = Fixture::new();
        let mut values = f.gate.bridge.responses.borrow_mut();
        values["anchor"]["reference"].as_object_mut().unwrap().extend(r.as_object().unwrap().clone());
        drop(values);
        denied(f.request(json!({"op":"anchor","ordinal":0})), "mismatch");
    }
}
#[test]
fn missing_ambiguous_and_excised_observations() {
    for case in 0..3 {
        let f = Fixture::new();
        let mut v = f.gate.bridge.responses.borrow_mut();
        match case {
            0 => v["list"]["observations"] = json!([]),
            1 => {
                let mut o = v["list"]["observations"][0].clone();
                o["input"] = json!("other");
                v["list"]["observations"].as_array_mut().unwrap().push(o);
            }
            _ => v["list"]["observations"][0]["class"] = json!("excised"),
        };
        drop(v);
        denied(f.request(json!({})),
            if case == 2 { "deleted" } else { "unverified" });
    }
}
#[test]
fn private_modes_and_symlink_components() {
    let f = Fixture::new();
    fs::set_permissions(&f.gate.registry,
            fs::Permissions::from_mode(0o644)).unwrap();
    denied(f.request(json!({})), "unsupported");
    fs::set_permissions(&f.gate.registry,
            fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(&f.path).unwrap();
    std::os::unix::fs::symlink(&f.gate.registry, &f.path).unwrap();
    denied(f.request(json!({})), "unsupported");
    assert!(files::snapshot(&f.gate.staging,std::path::Path::new("codex/sessions/g/fixture.jsonl")).is_err());
}
#[test]
fn pending_busy_and_completed_deletion_queue() {
    let f = Fixture::new();
    f.queue("pending");
    denied(f.request(json!({})), "deleted");
    assert!(f.gate.bridge.calls.borrow().is_empty());
    f.queue("complete");
    assert_eq!(f.request(json!({}))["status"],"verified");
    let lock =
        files::Lock::acquire(&f.root.join("queue/lock"), true).unwrap();
    denied(f.request(json!({})), "deleted");
    drop(lock);
    assert_eq!(f.request(json!({}))["status"],"verified");
}
#[test]
fn pending_publication_and_deletion_recovery() {
    let f = Fixture::new();
    let temp = f.gate.staging.join("tmp/pending");
    fs::rename(&f.path, &temp).unwrap();
    f.receipt("temporary", json!("tmp/pending"));
    f.receipt("status", json!("pending"));
    assert_eq!(f.request(json!({}))["status"],"verified");
    assert!(f.path.exists());
    let s = files::json(&f.gate.staging.join("state.json")).unwrap();
    assert!(s["receipts"]["g"].get("temporary").is_none());
    f.receipt("status", json!("deleting"));
    denied(f.request(json!({})), "deleted");
    assert!(!f.path.exists());
    assert_eq!(files::json(&f.gate.staging.join("state.json")).unwrap()["receipts"]["g"].as_object().unwrap().len(),3);
}
#[test]
fn eof_framing_size_limit_and_socket_directory() {
    assert_eq!(transport::receive(&b"{\"op\":\"read\"}"[..],100).unwrap(),json!({"op":"read"}));
    assert!(transport::receive(&b"   "[..],2).is_err());
    assert!(transport::receive(&b"{bad"[..],100).is_err());
    let f = Fixture::new();
    let socket = f.root.join("test.sock");
    assert!(transport::socket_parent(&socket).is_ok());
    fs::write(&socket, b"existing").unwrap();
    assert!(transport::socket_parent(&socket).is_err());
    fs::remove_file(&socket).unwrap();
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o770)).unwrap();
    assert!(transport::socket_parent(&socket).is_err());
}
#[test]
fn reference_json_numbers_follow_existing_wire_equality() {
    let f = Fixture::new();
    let mut r = f.reference.clone();
    r["revision"] = json!(1.0);
    r["chain"] = json!(true);
    assert_eq!(f.request(json!({"reference":r}))["status"],"verified");
}

#[test]
fn shared_json_corpus_at_wire_and_file_boundaries() {
    let f = Fixture::new();
    let cases = uasf_json::from_str(include_str!("../../uasf-json/tests/inspection.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let data = c["input"].as_str().unwrap().as_bytes();
        fs::write(f.root.join("wire.json"),data).unwrap();
        let wire = transport::receive(data, 1024*1024);
        let file = files::json(&f.root.join("wire.json"));
        assert_eq!(wire.is_ok(), c["decode_ok"] == true, "{}",c["name"]);
        assert_eq!(file.is_ok(), c["decode_ok"] == true);
        if let Ok(v) = wire {
            assert!(v.is_object());
            assert!(v.to_string().contains("12345"));
            assert_eq!(v,file.unwrap());
        }
    }
}

#[test]
fn admission_refuses_before_bridge_and_recovers() {
    let f = Fixture::new();
    let first = uasf_core::admission::acquire_gate(&f.gate.staging).unwrap();
    let second = uasf_core::admission::acquire_gate(&f.gate.staging).unwrap();
    let request = json!({"op":"read","generation":"g"});
    assert_eq!(f.gate.handle(&request), json!({"status":"unsupported","reason":"query_capacity_exceeded"}));
    assert!(f.gate.bridge.calls.borrow().is_empty());
    drop(first); drop(second);
    assert_eq!(f.gate.handle(&request)["status"], "verified");
}
