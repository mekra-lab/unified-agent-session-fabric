//! Frozen synthetic workflows run in new Rust processes, without an interpreter.
use std::{
    collections::BTreeMap, fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};
use uasf_json::{json, Value};
use uasf_core::{json::from_slice, model::{sha, canonical}};
use uasf_staging::{Store, uuid};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn read(path: &Path) -> Value {
    from_slice(&fs::read(path).unwrap()).unwrap()
}
struct Normalize {
    base: PathBuf,
    root: PathBuf,
    ids: BTreeMap<u64, String>,
    generations: Vec<(String, String)>,
}
impl Normalize {
    fn inode(&mut self, i: u64) -> String {
        let n = self.ids.len();
        self.ids.entry(i).or_insert_with(|| format!("inode-{n}")).clone()
    }
    fn norm(&mut self, v: &Value, key: &str) -> Value {
        match key {
            "captured_at" | "at" => {
                let s = v.as_str().unwrap();
                assert!(s.ends_with("+00:00")&&s.contains('T'));
                return json!("<timestamp>")
            }
            "device" => return json!("<device>"),
            "inode" => return json!(self.inode(v.as_u64().unwrap())),
            "source_facts" =>
                return json!(["<device>",self.inode(v[1].as_u64().unwrap()),v[2],"<mtime_ns>","<ctime_ns>"]),
            "blocked_inodes" =>
                return Value::Array(v.as_array().unwrap().iter().map(|x|
                                    json!(["<device>",self.inode(x[1].as_u64().unwrap())])).collect()),
            _ => {}
        }
        match v {
            Value::String(s) => {
                let mut s = s.clone();
                for (g, n) in &self.generations { s = s.replace(g, n) }
                json!(s.replace(self.root.to_str().unwrap(),"${STAGING}").replace(self.base.to_str().unwrap(),"${ROOT}"))
            }
            Value::Array(a) =>
                Value::Array(a.iter().map(|v| self.norm(v, "")).collect()),
            Value::Object(o) => {
                let mut map = uasf_json::Map::new();
                for (k, v) in o {
                    let name =
                        self.norm(&json!(k), "").as_str().unwrap().to_string();
                    map.insert(name, self.norm(v, k));
                }
                Value::Object(map)
            }
            _ => v.clone(),
        }
    }
    fn snapshot(&mut self, result: Value) -> Value {
        let state =
            if self.root.join("state.json").exists() {
                read(&self.root.join("state.json"))
            } else {
                json!({"version":1,"receipts":{},"blocked_paths":[],"blocked_inodes":[],"events":[]})
            };
        self.generations =
            state["receipts"].as_object().unwrap().keys().enumerate().map(|(i,
                            g)| (g.clone(), format!("generation-{i}"))).collect();
        fn walk(path: &Path, found: &mut Vec<PathBuf>) {
            for e in fs::read_dir(path).unwrap() {
                let e = e.unwrap();
                if e.file_type().unwrap().is_dir() {
                    walk(&e.path(), found)
                } else if e.path().extension().is_some_and(|s| s == "jsonl") {
                    found.push(e.path())
                }
            }
        }
        let response = self.norm(&result, "");
        let state = self.norm(&state, "");
        let mut found = Vec::new();
        walk(&self.root, &mut found);
        let mut copies = Vec::new();
        for p in found {
            let m = fs::metadata(&p).unwrap();
            let path =
                self.norm(&json!(p.strip_prefix(&self.root).unwrap()), "");
            copies.push(json!({"path":path,"length":m.len(),"sha256":sha(&fs::read(p).unwrap()),"mode":m.mode()&0o7777}));
        }
        copies.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        json!({"response":response,"state":state,"copies":copies})
    }
}
fn run_cli(root: &Path, registry: &Path, action: &str, source: &str,
    rel: Option<&str>) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_uasf-staging"));
    cmd.arg("--root").arg(root).arg("--registry").arg(registry).arg("--unmanaged-test-mode").arg(action).arg(source);
    if let Some(r) = rel { cmd.arg(r); }
    let out = cmd.output().unwrap();
    let value = from_slice(&out.stdout).unwrap();
    let good =
        |v: &Value|
            v["status"] == "published" || v["status"] == "unchanged" ||
                action == "delete" && v["status"] == "deleted";
    let ok =
        if let Some(a) = value.as_array() {
            a.iter().all(good)
        } else { good(&value) };
    assert_eq!(out.status.code(),Some(if ok{0}else{2}));
    value
}
#[test]
fn frozen_staging_workflows_without_python() {
    assert_eq!(sha(include_bytes!("golden.json")), include_str!("golden.sha256").trim());
    let frozen =
        read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden.json"));
    let base =
        std::env::temp_dir().join(format!("uasf-staging-golden-{}",uuid().unwrap()));
    fs::create_dir(&base).unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    let _cleanup = Scratch(base.clone());
    let mut comparisons = 0;
    for case in frozen["cases"].as_array().unwrap() {
        let dir = base.join(case["name"].as_str().unwrap());
        let original = dir.join("original");
        fs::create_dir_all(&original).unwrap();
        let root = dir.join("rust");
        let registry =
            json!({"version":1,"sources":{"c":{"kind":"codex","root":original},"a":{"kind":"claude","root":original}}});
        Store::new(root.clone(), registry.clone()).unwrap();
        let config = dir.join("sources.json");
        fs::write(&config, uasf_json::to_vec(&registry).unwrap()).unwrap();
        let mut norm =
            Normalize {
                base: dir.clone(),
                root: root.clone(),
                ids: BTreeMap::new(),
                generations: Vec::new(),
            };
        let mut index = 0;
        for step in case["steps"].as_array().unwrap() {
            let op = step["op"].as_str().unwrap();
            let path = original.join(step["path"].as_str().unwrap_or(""));
            let wire =
                ||
                    step["bytes"].as_array().unwrap().iter().map(|v|
                                u8::try_from(v.as_u64().unwrap()).unwrap()).collect::<Vec<_>>();
            let result =
                match op {
                    "write" | "replace" | "repeat" | "sparse" | "hardlink" |
                        "symlink" => {
                        fs::create_dir_all(path.parent().unwrap()).unwrap();
                        match op {
                            "write" => fs::write(&path, wire()).unwrap(),
                            "replace" => {
                                let temp = path.with_extension("replacement");
                                fs::write(&temp, wire()).unwrap();
                                fs::rename(temp, &path).unwrap();
                            }
                            "repeat" => {
                                let n = step["size"].as_u64().unwrap() as usize;
                                let mut data = vec![b'z';n];
                                data[0] = step["head"].as_u64().unwrap() as u8;
                                data[n - 1] = b'\n';
                                fs::write(path, data).unwrap();
                            }
                            "sparse" =>
                                fs::File::create(path).unwrap().set_len(step["size"].as_u64().unwrap()).unwrap(),
                            "hardlink" =>
                                fs::hard_link(original.join(step["target"].as_str().unwrap()),
                                        path).unwrap(),
                            _ =>
                                symlink(original.join(step["target"].as_str().unwrap()),
                                        path).unwrap(),
                        }
                        continue
                    }
                    "queue" => {
                        let queue = dir.join("rust-queue");
                        fs::create_dir_all(&queue).unwrap();
                        let mut sel = step["selector"].clone();
                        if sel["kind"] == "text" {
                            let text =
                                sel.as_object_mut().unwrap().remove("text").unwrap();
                            let data = text.as_str().unwrap().as_bytes();
                            sel["length"] = json!(data.len());
                            sel["sha256"] = json!(sha(data));
                        }
                        let marker = root.join("deletion-binding.json");
                        fs::write(&marker,
                                uasf_json::to_vec(&json!({"version":1,"kind":"staging","resource":root,"root":queue,"owner_uid":unsafe{libc::geteuid()}})).unwrap()).unwrap();
                        fs::set_permissions(&marker,
                                fs::Permissions::from_mode(0o600)).unwrap();
                        fs::write(queue.join("jobs.json"),
                                uasf_json::to_vec(&json!({"version":1,"binding":{"staging":root},"jobs":{"synthetic":{"status":step["status"],"paths":[],"inodes":[],"selector":sel}}})).unwrap()).unwrap();
                        Value::Null
                    }
                    "remove_copy" | "corrupt_copy" => {
                        let state = read(&root.join("state.json"));
                        let r =
                            state["receipts"].as_object().unwrap().values().last().unwrap();
                        let path =
                            root.join(r["staging_relative"].as_str().unwrap());
                        if op == "remove_copy" {
                            fs::remove_file(path).unwrap();
                        } else {
                            fs::set_permissions(&path,
                                    fs::Permissions::from_mode(0o600)).unwrap();
                            fs::write(&path, b"bad\n").unwrap();
                            fs::set_permissions(path,
                                    fs::Permissions::from_mode(0o400)).unwrap();
                        }
                        Value::Null
                    }
                    _ =>
                        run_cli(&root, &config, op,
                            step["source"].as_str().unwrap(), step["path"].as_str()),
                };
            let actual = norm.snapshot(result);
            let expected = &case["expected"][index];
            assert_eq!(canonical(&actual),canonical(expected),"{} step {}",case["name"],index);
            index += 1;
            comparisons += 1;
        }
        assert_eq!(index,case["expected"].as_array().unwrap().len());
    }
    assert_eq!(comparisons,60);
    assert_eq!(frozen["cases"].as_array().unwrap().len(),21);
}
