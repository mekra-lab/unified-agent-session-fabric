//! Frozen synthetic queue transitions, replayed without a Python interpreter.
use std::{
    collections::BTreeMap, fs, path::{Path, PathBuf},
    os::unix::fs::{MetadataExt, PermissionsExt},
};
use uasf_json::{json, Value};
use uasf_del::{Coordinator, Executor, Lease, Result, Error};
use uasf_core::model::sha;
use rusqlite::{Connection, params};
fn parse(path: &Path) -> Value {
    uasf_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn replace(v: &Value, from: &str, to: &str) -> Value {
    match v {
        Value::String(s) => json!(s.replace(from,to)),
        Value::Array(a) =>
            json!(a.iter().map(|v|replace(v,from,to)).collect::<Vec<_>>()),
        Value::Object(o) =>
            Value::Object(o.iter().map(|(k, v)|
                            (k.replace(from, to), replace(v, from, to))).collect()),
        _ => v.clone(),
    }
}
struct Effects {
    db: PathBuf,
    offline: bool,
}
impl Executor for Effects {
    fn erase(&mut self, ids: &Value, _: &Lease) -> Result<()> {
        let mut db = Connection::open(&self.db)?;
        db.execute_batch("PRAGMA secure_delete=ON;PRAGMA synchronous=FULL;")?;
        let tx = db.transaction()?;
        for id in ids.as_array().unwrap() {
            let id = id.as_str().unwrap();
            let mut query =
                tx.prepare("SELECT source,idem_key FROM observation WHERE receipt_id=?1")?;
            for row in
                query.query_map([id],
                        |r|
                            Ok((r.get::<_, String>(0)?,
                                    r.get::<_, Option<String>>(1)?)))? {
                let (s, k) = row?;
                tx.execute("INSERT OR IGNORE INTO deletion_tombstone VALUES (?1,?2,?3)",
                        params![id,s,k.map(|k|sha(k.as_bytes()))])?;
            }
            tx.execute("DELETE FROM observation WHERE receipt_id=?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }
    fn excise(&mut self, _: &str) -> Result<()> {
        if self.offline { Err(Error("ConnectionError")) } else { Ok(()) }
    }
}
fn normalize(v: &Value, key: &str, base: &Path, ids: &BTreeMap<u64, String>)
    -> Value {
    if key == "device" { return json!("<device>") }
    if key == "inode" { return json!(ids[&v.as_u64().unwrap()]) }
    if key == "inodes" || key == "blocked_inodes" {
        return json!(v.as_array().unwrap().iter().map(|r|json!(["<device>",ids[&r[1].as_u64().unwrap()]])).collect::<Vec<_>>())
    }
    match v {
        Value::String(s) =>
            json!(s.replace(base.to_str().unwrap(),"${ROOT}")),
        Value::Array(a) =>
            json!(a.iter().map(|v|normalize(v,"",base,ids)).collect::<Vec<_>>()),
        Value::Object(o) =>
            Value::Object(o.iter().map(|(k, v)|
                            (k.clone(), normalize(v, k, base, ids))).collect()),
        _ => v.clone(),
    }
}
#[test]
fn frozen_deletion_workflows() {
    let bytes = include_bytes!("golden.json");
    assert_eq!(sha(bytes),include_str!("golden.sha256").trim());
    let frozen = uasf_json::from_slice(bytes).unwrap();
    let mut comparisons = 0;
    for case in frozen["cases"].as_array().unwrap() {
        let base =
            std::env::temp_dir().join(format!("uasf-del-golden-{}",uasf_staging::uuid().unwrap()));
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base,
                fs::Permissions::from_mode(0o700)).unwrap();
        let mut state =
            replace(&case["expected"][0]["staging"], "${ROOT}",
                base.to_str().unwrap());
        state["blocked_paths"] = json!([]);
        state["blocked_inodes"] = json!([]);
        let original = base.join("original");
        let staging = base.join("staging");
        let mut ids = BTreeMap::new();
        let mut mapping = json!({});
        for i in 0..2 {
            let (session, word, name) =
                if i == 0 {
                    ("selected", "erased", "one")
                } else { ("other", "keep", "two") };
            let data =
                format!("{{\"type\": \"session_meta\", \"payload\": {{\"id\": \"{session}\"}}}}\n{{\"text\": \"{word}\"}}\n");
            let data =
                if case["escaped"] == true && i == 0 {
                    data.replace("erased", r"\u0065rased")
                } else { data };
            let source =
                original.join(format!("sessions/rollout-{name}.jsonl"));
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(&source, &data).unwrap();
            let m = source.metadata().unwrap();
            ids.insert(m.ino(), format!("<inode-{i}>"));
            let g = format!("generation-{i}");
            let r = &mut state["receipts"][&g];
            r["device"] = json!(m.dev());
            r["inode"] = json!(m.ino());
            assert_eq!(r["sha256"],sha(data.as_bytes()));
            let copy = staging.join(r["staging_relative"].as_str().unwrap());
            fs::create_dir_all(copy.parent().unwrap()).unwrap();
            fs::write(&copy, &data).unwrap();
            fs::set_permissions(&copy,
                    fs::Permissions::from_mode(0o400)).unwrap();
            mapping[copy.to_str().unwrap()] = json!([format!("input-{i}")]);
        }
        fs::set_permissions(&staging,
                fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(staging.join("state.json"), state.to_string()).unwrap();
        let db_path = base.join("intake.db");
        let db = Connection::open(&db_path).unwrap();
        db.execute_batch("CREATE TABLE observation(receipt_id TEXT PRIMARY KEY,source TEXT,idem_key TEXT,session TEXT,envelope TEXT,payload BLOB,receipt TEXT);CREATE TABLE deletion_tombstone(receipt_id TEXT PRIMARY KEY,source TEXT,key_hash TEXT);").unwrap();
        for (id, session, word) in
            [("api-one", "selected", "erased"), ("api-two", "other", "keep")]
            {
            let text =
                if case["escaped"] == true && id == "api-one" {
                    r"\u0065rased"
                } else { word };
            let data = format!("{{\"text\":\"{text}\"}}");
            db.execute("INSERT INTO observation VALUES (?1,'s',?2,?3,'{}',?4,'{}')",
                    params![id,format!("{id}-key"),session,data.as_bytes()]).unwrap();
        }
        drop(db);
        let registry =
            json!({"version":1,"sources":{"s":{"kind":"codex","root":original}}});
        let q =
            Coordinator::new(base.join("queue"), staging.clone(), registry,
                    db_path.clone()).unwrap();
        for (index, step) in
            case["steps"].as_array().unwrap().iter().enumerate() {
            let mut request = step.clone();
            if request["op"] == "request" {
                request["input_map"] =
                    if step["missing_map"] == true {
                        json!({})
                    } else { mapping.clone() };
            }
            let mut executor =
                Effects {
                    db: db_path.clone(),
                    offline: step["offline"] == true,
                };
            let result =
                match uasf_del::process::execute(&q, &request, &mut executor,
                        &mut |_| Ok(())) {
                    Ok(v) => v,
                    Err(e) =>
                        json!({"error":if
                                e.0=="ValueError"||e.0=="JSONDecodeError"{"ValueError"}else{"RuntimeError"}}),
                };
            let db = Connection::open(&db_path).unwrap();
            let mut query =
                db.prepare("SELECT receipt_id,source,idem_key,session,payload FROM observation ORDER BY rowid").unwrap();
            let observations =
                query.query_map([],
                                |r|
                                    Ok(json!({"receipt_id":r.get::<_,String>(0)?,"source":r.get::<_,String>(1)?,"key":r.get::<_,String>(2)?,"session":r.get::<_,String>(3)?,"payload_sha256":sha(&r.get::<_,Vec<u8>>(4)?)}))).unwrap().collect::<std::result::Result<Vec<_>,
                        _>>().unwrap();
            let mut query =
                db.prepare("SELECT receipt_id,source,key_hash FROM deletion_tombstone ORDER BY receipt_id").unwrap();
            let tombstones =
                query.query_map([],
                                |r|
                                    Ok(json!([r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?]))).unwrap().collect::<std::result::Result<Vec<_>,
                        _>>().unwrap();
            let mut copies = Vec::new();
            for r in state["receipts"].as_object().unwrap().values() {
                let relative = r["staging_relative"].as_str().unwrap();
                let p = staging.join(relative);
                if p.exists() {
                    copies.push(json!({"path":relative,"sha256":sha(&fs::read(p).unwrap())}));
                }
            }
            copies.sort_by_key(|v| v["path"].as_str().unwrap().to_owned());
            let mut cache: Vec<_> =
                fs::read_dir(base.join("queue/cache")).unwrap().map(|e|
                            e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            cache.sort();
            let snapshot =
                json!({"response":result,"queue":q.state().unwrap(),"staging":parse(&staging.join("state.json")),"observations":observations,"tombstones":tombstones,"copies":copies,"cache":cache});
            assert_eq!(normalize(&snapshot,"",&base,&ids),case["expected"][index],"{} step {index}",case["name"]);
            comparisons += 1;
        }
        fs::remove_dir_all(&base).unwrap();
    }
    assert_eq!(comparisons,45);
}
