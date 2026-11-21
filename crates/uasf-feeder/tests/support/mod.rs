#![allow(dead_code)]
use std::{fs, io::Read, path::PathBuf, os::unix::fs::PermissionsExt};
use uasf_json::{json, Value};
use uasf_feeder::{Endpoint, initialize, io, canonical, uuid};
pub struct Fixture {
    pub root: PathBuf,
    pub rel: String,
    pub cfg: Value,
    pub key: Vec<u8>,
}
impl Fixture {
    pub fn new(kind: &str, data: &[u8]) -> Self {
        let root =
            std::env::temp_dir().join(format!("feeder-{}",uuid().unwrap()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root,
                fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir_all(root.join("original/sessions")).unwrap();
        fs::create_dir_all(root.join("original/projects")).unwrap();
        let rel =
            if kind == "claude" {
                    "projects/example.jsonl"
                } else { "sessions/rollout-one.jsonl" }.to_owned();
        fs::write(root.join("original").join(&rel), data).unwrap();
        let cfg =
            json!({"version":1,"stream":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","producer_uid":unsafe{libc::geteuid()},"collector_uid":unsafe{libc::geteuid()},"sources":{"s":{"root":root.join("original"),"kind":kind,"pattern":"native-jsonl"}}});
        let mut key = vec![0;32];
        fs::File::open("/dev/urandom").unwrap().read_exact(&mut key).unwrap();
        io::atomic(&root.join("key"), &key, 0o600, false, false).unwrap();
        io::atomic(&root.join("config.json"), &canonical(&cfg), 0o600, false,
                false).unwrap();
        for d in ["producer", "collector", "inbox", "policy"] {
            fs::create_dir(root.join(d)).unwrap();
            fs::set_permissions(root.join(d),
                    fs::Permissions::from_mode(if d == "inbox" {
                            0o3730
                        } else { 0o700 })).unwrap();
        }
        io::atomic(&root.join("policy/lock"), b"", 0o600, false,
                false).unwrap();
        io::atomic(&root.join("policy/deleted.json"),
                &canonical(&json!({"version":1,"stream":cfg["stream"],"revision":0,"paths":[],"inodes":[]})),
                0o640, false, false).unwrap();
        for r in ["producer", "collector"] {
            initialize(&root.join(r), &cfg, &key, r).unwrap();
        }
        Self { root, rel, cfg, key }
    }
    pub fn endpoint(&self, role: &str) -> Endpoint {
        Endpoint::new(&self.root.join("config.json"), &self.root.join("key"),
                &self.root.join(role), &self.root.join("policy"),
                &self.root.join("inbox"), role).unwrap()
    }
    pub fn emit(&self) -> Value {
        self.endpoint("producer").emit("s", &self.rel,
                &mut |_| Ok(())).unwrap()
    }
    pub fn accept(&self, sent: &Value) -> Value {
        self.endpoint("collector").accept(sent["record"].as_str().unwrap(),
                &mut |_| Ok(())).unwrap()
    }
    pub fn wire(&self, sent: &Value) -> Vec<u8> {
        fs::read(self.root.join("inbox").join(sent["record"].as_str().unwrap())).unwrap()
    }
    pub fn altered(&self, sent: &Value, mutate: impl FnOnce(&mut Value),
        body: Option<&[u8]>) -> String {
        let (mut m, p) =
            uasf_feeder::unseal(&self.wire(sent), &self.key,
                    &self.cfg).unwrap();
        mutate(&mut m);
        let raw =
            uasf_feeder::seal(&m, body.unwrap_or(&p), &self.key).unwrap();
        let name =
            format!("{}-{}.rec",m["generation"].as_str().unwrap(),uuid().unwrap());
        io::atomic(&self.root.join("inbox").join(&name), &raw, 0o640, false,
                true).unwrap();
        name
    }
    pub fn source(&self) -> PathBuf {
        self.root.join("original").join(&self.rel)
    }
    pub fn cli(&self, role: &str, op: &str, args: &[&str])
        -> std::process::Command {
        let mut c =
            std::process::Command::new(env!("CARGO_BIN_EXE_uasf-feeder"));
        for (k, p) in
            [("config", "config.json"), ("key", "key"), ("state", role),
                    ("policy", "policy"), ("inbox", "inbox")] {
            c.arg(format!("--{k}")).arg(self.root.join(p));
        }
        c.args(["--role", role, op]).args(args);
        c
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
}
