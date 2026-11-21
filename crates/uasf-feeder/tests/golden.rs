//! Fixed Python responses, replayed without importing or invoking Python.
mod support;
use support::Fixture;
use std::{collections::BTreeMap, fs, path::Path};
use uasf_json::{json, Value};
use uasf_feeder::{Error, Result, sha};
struct Normal {
    root: String,
    ids: BTreeMap<String, String>,
    records: BTreeMap<String, String>,
}
impl Normal {
    fn value(&mut self, v: &Value, key: &str) -> Value {
        if ["device", "inode", "mtime_ns", "ctime_ns",
                        "config_sha256"].contains(&key) {
            return json!(if
                    key=="config_sha256"{"$config-sha256".into()}else{format!("${key}")});
        }
        match v {
            Value::Object(o) => {
                let mut out = uasf_json::Map::new();
                for (k, v) in o { out.insert(k.clone(), self.value(v, k)); }
                Value::Object(out)
            }
            Value::Array(a) =>
                json!(a.iter().map(|x|self.value(x,"")).collect::<Vec<_>>()),
            Value::String(s) => {
                if ["generation", "previous"].contains(&key) && s.len() == 32
                    {
                    let n = self.ids.len() + 1;
                    return json!(self.ids.entry(s.clone()).or_insert(format!("$generation-{n}")));
                }
                if key == "record" {
                    let n = self.records.len() + 1;
                    return json!(self.records.entry(s.clone()).or_insert(format!("$record-{n}")));
                }
                let mut text = s.replace(&self.root, "$ROOT");
                for (raw, n) in &self.ids {
                    text =
                        text.replace(raw,
                                n).replace(&format!("{}-{}-{}-{}-{}",&raw[..8],&raw[8..12],&raw[12..16],&raw[16..20],&raw[20..]),
                            n);
                }
                json!(text)
            }
            _ => v.clone(),
        }
    }
}
fn copies(p: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(p).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            copies(&p, out);
        } else if p.extension().is_some_and(|s| s == "jsonl") {
            out.push(sha(&fs::read(p).unwrap()));
        }
    }
}
#[test]
fn frozen_owner_collector_workflows() {
    let raw = include_bytes!("golden.json");
    assert_eq!(sha(raw),include_str!("golden.sha256").trim());
    let golden = uasf_json::from_slice(raw).unwrap();
    let mut count = 0;
    for case in golden["cases"].as_array().unwrap() {
        let f =
            Fixture::new(if case["name"] == "claude" {
                    "claude"
                } else { "codex" },
                case["input"].as_str().unwrap().as_bytes());
        let mut normal =
            Normal {
                root: f.root.to_string_lossy().into(),
                ids: BTreeMap::new(),
                records: BTreeMap::new(),
            };
        let mut sent: Vec<Value> = vec![];
        for (i, step) in case["steps"].as_array().unwrap().iter().enumerate()
            {
            let op = step[0].as_str().unwrap();
            let result: Result<Value> =
                (||
                            {
                                let producer = f.endpoint("producer");
                                let collector = f.endpoint("collector");
                                let last = sent.last().cloned().unwrap_or(Value::Null);
                                let g = last["generation"].as_str().unwrap_or("");
                                match op {
                                    "emit" => producer.emit("s", &f.rel, &mut |_| Ok(())),
                                    "accept" =>
                                        collector.accept(last["record"].as_str().unwrap(),
                                            &mut |_| Ok(())),
                                    "accept_first" =>
                                        collector.accept(sent[0]["record"].as_str().unwrap(),
                                            &mut |_| Ok(())),
                                    "retry" => producer.retry(g),
                                    "materialize" => collector.materialize(g),
                                    "delete" => collector.delete("s", &f.rel),
                                    "purge" => producer.purge(),
                                    _ => Err(Error("invalid_arguments")),
                                }
                            })();
            let (v, code) =
                match result {
                    Ok(v) => {
                        let c = if v["status"] == "not_captured" { 2 } else { 0 };
                        (v, c)
                    }
                    Err(e) => (json!({"status":"rejected","reason":e.0}), 2),
                };
            if (op == "emit" || op == "retry") && v["status"] == "sent" {
                sent.push(v.clone());
            }
            let response = normal.value(&v, "");
            let mut data = vec![];
            copies(&f.root.join("collector/validated"), &mut data);
            data.sort();
            let actual =
                json!({"response":response,"exit_code":code,"validated":data});
            assert_eq!(actual,case["expected"][i],"{} step {i}",case["name"]);
            count += 1;
        }
    }
    assert_eq!(count,29);
}
