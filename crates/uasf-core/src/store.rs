use std::{
    collections::BTreeMap, path::{Path, PathBuf},
    time::Duration,
};
use rusqlite::{params, Connection, OptionalExtension};
use uasf_json::{json, Value};
use crate::{
    model::*, parser::{self, VERSION},
    provider::ByteProvider,
};

pub struct Core {
    pub(crate) persistent_path: PathBuf,
    pub(crate) db: Connection,
    pub(crate) spool_directory: PathBuf,
    pub(crate) scope_active: std::cell::Cell<bool>,
    pub(crate) scope_metrics: std::cell::RefCell<crate::scope::ScopeMetrics>,
}
fn resolved(path: &Path) -> Result<PathBuf> {
    let path =
        if path.is_absolute() {
            path.to_path_buf()
        } else { std::env::current_dir()?.join(path) };
    if path.exists() { return Ok(path.canonicalize()?); }
    let parent = path.parent().ok_or(Error::Invalid("store_path"))?;
    std::fs::create_dir_all(parent)?;
    Ok(parent.canonicalize()?.join(path.file_name().ok_or(Error::Invalid("store_path"))?))
}
impl Core {
    pub fn open(index: impl AsRef<Path>, persistent: impl AsRef<Path>)
        -> Result<Self> {
        let index = resolved(index.as_ref())?;
        let persistent = resolved(persistent.as_ref())?;
        if index == persistent {
            return Err(Error::Invalid("separate_stores_required"));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(a), Ok(b)) =
                    (std::fs::metadata(&index), std::fs::metadata(&persistent))
                {
                if a.dev() == b.dev() && a.ino() == b.ino() {
                    return Err(Error::Invalid("separate_stores_required"));
                }
            }
        }
        let db = Connection::open(&index)?;
        db.busy_timeout(Duration::from_secs(30))?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA secure_delete=ON; PRAGMA recursive_triggers=ON;")?;
        db.execute("ATTACH DATABASE ? AS durable",
                params![persistent.to_string_lossy()])?;
        db.execute_batch("PRAGMA durable.journal_mode=DELETE; PRAGMA durable.secure_delete=ON; BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS durable.deletion_binding(id INTEGER PRIMARY KEY CHECK(id=1),root TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS durable.query_generation(id INTEGER PRIMARY KEY CHECK(id=1),epoch INTEGER NOT NULL CHECK(typeof(epoch)='integer' AND epoch>=0));
            INSERT OR IGNORE INTO durable.query_generation VALUES(1,0);
            CREATE TABLE IF NOT EXISTS query_budget_generation(id INTEGER PRIMARY KEY CHECK(id=1),epoch INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS objects(kind TEXT NOT NULL,id TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(kind,id));
            CREATE TABLE IF NOT EXISTS metadata(id TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS durable.query_records(id TEXT PRIMARY KEY,records INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS query_budget(id INTEGER PRIMARY KEY,records INTEGER NOT NULL,edges INTEGER NOT NULL,depth INTEGER NOT NULL,bytes INTEGER NOT NULL,policy_rows INTEGER NOT NULL,output INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS durable.sources(id TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS durable.captures(id TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS durable.names(id TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS durable.evidence(id TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS durable.provenance(id TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS durable.tombstones(id TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
        for table in ["sources", "captures", "names", "evidence", "provenance", "tombstones", "query_records"] {
            for action in ["INSERT", "UPDATE", "DELETE"] {
                db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS durable.budget_{table}_{action} AFTER {action} ON {table} BEGIN UPDATE query_generation SET epoch=epoch+1 WHERE id=1; END;"))?;
            }
        }
        db.execute_batch("COMMIT")?;
        let core = Self { persistent_path: persistent, db, spool_directory: index.parent().unwrap().to_owned(), scope_active: Default::default(), scope_metrics: Default::default() };
        core.scope_schema()?;
        Ok(core)
    }
    fn transaction<T>(&self, f: impl FnOnce(&Self) -> Result<T>)
        -> Result<T> {
        self.db.execute_batch("BEGIN IMMEDIATE")?;
        match f(self) {
            Ok(v) => { self.db.execute_batch("COMMIT")?; Ok(v) }
            Err(e) => { self.db.execute_batch("ROLLBACK")?; Err(e) }
        }
    }
    pub(crate) fn durable(&self, table: &str) -> Result<Vec<Value>> {
        if !["sources", "captures", "names", "evidence",
                            "provenance", "tombstones"].contains(&table) {
            return Err(Error::Invalid("table"));
        }
        let mut q =
            self.db.prepare(&if self.scope_active.get() {format!("SELECT d.value FROM temp.scope_ids s JOIN durable.{table} d ON d.id=s.id WHERE s.kind='{table}' ORDER BY d.id")} else {format!("SELECT value FROM durable.{table} ORDER BY id")})?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| {let raw=r?; if self.scope_active.get() {let mut m=self.scope_metrics.borrow_mut();m.response_rows+=1;m.response_decode_bytes+=raw.len() as u64;} Ok(uasf_json::from_str(&raw)?) }).collect()
    }
    fn insert(&self, table: &str, id: &str, value: &Value, immutable: bool)
        -> Result<()> {
        if !["sources", "captures", "names", "evidence",
                            "provenance", "tombstones"].contains(&table) {
            return Err(Error::Invalid("table"));
        }
        let old: Option<String> =
            self.db.query_row(&format!("SELECT value FROM durable.{table} WHERE id=?"),
                        [id], |r| r.get(0)).optional()?;
        if immutable {
            if let Some(old) = old {
                // Object order is irrelevant. Canonical numbers retain int/float
                // and signed zero, but erase equivalent floating spellings.
                if canonical(&uasf_json::from_str(&old)?) != canonical(value) {
                    return Err(Error::Invalid("immutable_registration_conflict"));
                }
            }
        }
        self.db.execute(&format!("INSERT OR REPLACE INTO durable.{table} VALUES (?,?)"),
                params![id,uasf_json::to_string(value)?])?;
        Ok(())
    }
    pub fn register_source(&self, source: &Source) -> Result<()> {
        if source.id.is_empty() || source.engine.is_empty() {
            return Err(Error::Invalid("source_identity"));
        }
        self.transaction(|s|
                s.insert("sources", &source.id,
                    &uasf_json::to_value(source)?, true))
    }
    pub fn register_capture(&self, c: &Capture) -> Result<()> {
        c.validate()?;
        self.transaction(|s|
                {
                    if !s.db.query_row("SELECT EXISTS(SELECT 1 FROM durable.sources WHERE id=?)",
                                    [&c.source], |r| r.get::<_, bool>(0))? {
                        return Err(Error::Invalid("unregistered_source"));
                    }
                    s.insert("captures", &c.id(), &uasf_json::to_value(c)?,
                            true)?;
                    if !s.db.query_row("SELECT EXISTS(SELECT 1 FROM durable.provenance WHERE id=?)",
                                    [c.id()], |r| r.get::<_, bool>(0))? {
                        s.insert("provenance", &c.id(),
                                &json!({"id":c.id(),"source":c.source,"session_id":c.session_id(),"window_id":c.window,"anchor":c.anchor(),"ownership_basis":"explicit_capture_registration","observation":"unobserved","trusted":null}),
                                false)?;
                    }
                    s.redact_anchor(&c.anchor())
                })
    }
    pub fn set_display_name(&self, source: &str, session: &str, name: &str)
        -> Result<()> {
        let sid = session_key(source, session);
        self.transaction(|s| {
                s.insert("names", &sid,
                    &json!({"id":sid,"source":source,"session":session,"display_name":if
                                name.trim().is_empty(){Value::Null}else{json!(name.trim())}}),
                    false)?;
                if s.session_deleted(&sid)? { s.db.execute("UPDATE durable.names SET value=json_set(value,'$.display_name',null) WHERE id=?",[&sid])?; }
                Ok(())
        })
    }
    pub fn add_evidence(&self, input: &Value) -> Result<String> {
        let fields =
            ["evidence_id", "source", "session", "fact", "assessment",
                    "method", "observed_at", "anchors", "body", "target_turn",
                    "target_call"];
        if !input.as_object().is_some_and(|o|
                        o.keys().all(|k| fields.contains(&k.as_str()))) {
            return Err(Error::Invalid("evidence_fields"));
        }
        if !["planned", "attempted", "applied", "received", "exposed", "used",
                                "outcome_unknown"].contains(&input["fact"].as_str().unwrap_or(""))
                ||
                !["reported", "candidate", "observed",
                                "unknown"].contains(&input["assessment"].as_str().unwrap_or(""))
            {
            return Err(Error::Invalid("evidence_fact_or_assessment"));
        }
        if ["evidence_id", "source", "session", "method",
                                "observed_at"].iter().any(|k| !nonempty(&input[k])) ||
                arr(&input["anchors"]).is_empty() {
            return Err(Error::Invalid("evidence_provenance_required"));
        }
        for a in arr(&input["anchors"]) { validate_anchor(a)?; }
        let source = input["source"].as_str().unwrap();
        let id = key("evidence", json!([source,input["evidence_id"]]));
        let mut value =
            json!({"id":id,"source":source,"session_id":session_key(source,input["session"].as_str().unwrap()),"fact":input["fact"],"assessment":input["assessment"],"method":input["method"],"observed_at":input["observed_at"],"anchors":input["anchors"],"body":input["body"],"target_turn":input["target_turn"],"target_call":input["target_call"],"status":if
                    input["assessment"]=="unknown"{"unknown"}else{"complete"},"reasons":[],"current_context":"unknown","automatic_promotion":false});
        self.transaction(|s|
                    {
                        if !s.db.query_row("SELECT EXISTS(SELECT 1 FROM durable.sources WHERE id=?)",
                                        [source], |r| r.get::<_, bool>(0))? {
                            return Err(Error::Invalid("unregistered_source"));
                        }
                        if s.explicitly_deleted_indexed(&value, "evidence")? {
                            erase_body(&mut value);
                        }
                        s.insert("evidence", &id, &value, true)
                    })?;
        Ok(id)
    }
    fn put(&self, kind: &str, v: &Value) -> Result<()> {
        let mut erased;
        let v=if self.explicitly_deleted_indexed(v,kind)? {erased=v.clone();erase_body(&mut erased); &erased} else {v};
        self.db.execute("INSERT OR REPLACE INTO objects VALUES (?,?,?)",
                params![kind,v["id"].as_str().ok_or(Error::Invalid("object_id"))?,uasf_json::to_string(v)?])?;
        Ok(())
    }
    pub(crate) fn visit_objects(&self, kind: &str, filter: Option<(&str, &[String])>, mut visit: impl FnMut(Value) -> Result<()>) -> Result<()> {
        let (mut sql, ids) = if let Some((field, ids)) = filter {
            let column = match field { "id" => "id", "capture_id" => "json_extract(value, '$.capture_id')", _ => return Err(Error::Invalid("object_filter")) };
            (format!("SELECT value FROM objects WHERE kind=?1 AND {column} IN (SELECT value FROM json_each(?2)) ORDER BY id"), uasf_json::to_string(ids)?)
        } else { ("SELECT value FROM objects WHERE kind=?1 AND ?2 IS NOT NULL ORDER BY id".into(), "[]".into()) };
        if self.scope_active.get() && filter.is_none() {
            let field=if kind=="session" { "id" } else { "json_extract(value, '$.capture_id')" };
            let table=if kind=="session" { "sessions" } else { "captures" };
            sql=format!("SELECT value FROM objects WHERE kind=?1 AND {field} IN (SELECT id FROM temp.scope_ids WHERE kind='{table}') AND ?2 IS NOT NULL ORDER BY id");
        }
        if filter.is_some_and(|(field,_)|field=="capture_id") || (self.scope_active.get() && filter.is_none() && kind!="session") {sql=sql.replace("FROM objects WHERE","FROM objects INDEXED BY scope_objects_capture WHERE");}
        // Select only rowids through the existing membership index, then read
        // bodies in physical order. Sorting full JSON rows by hashed IDs causes
        // random reads and a large temporary sorter. The View restores logical
        // ID order after policy handling and body spooling, before projection.
        let membership = sql.trim_end_matches(" ORDER BY id").replacen("SELECT value FROM", "SELECT rowid FROM", 1);
        let sql = format!("SELECT value FROM objects WHERE rowid IN ({membership}) ORDER BY rowid");
        let mut statement = self.db.prepare(&sql)?;
        let mut rows = statement.query(params![kind, ids])?;
        while let Some(row) = rows.next()? {
            let raw=row.get_ref(0)?.as_str().map_err(|_|Error::Invalid("stored_json_text"))?;
            if self.scope_active.get(){let mut m=self.scope_metrics.borrow_mut();m.response_rows+=1;m.response_decode_bytes+=raw.len() as u64;}
            visit(uasf_json::from_str_with_spare(raw,if kind=="edge" {5}else{2})?)?;
        }
        drop(rows);
        if self.scope_active.get(){let mut m=self.scope_metrics.borrow_mut();m.vm_steps+=statement.get_status(rusqlite::StatementStatus::VmStep) as i64;m.fullscan_steps+=statement.get_status(rusqlite::StatementStatus::FullscanStep) as i64;}
        Ok(())
    }
    pub fn rebuild(&self, provider: &impl ByteProvider) -> Result<Value> {
        self.rebuild_version(provider, VERSION)
    }
    pub fn rebuild_version(&self, provider: &impl ByteProvider, version: &str)
        -> Result<Value> {
        self.transaction(|s|
                {
                    s.db.execute("DELETE FROM objects", [])?;
                    let mut indexed_version = version;
                    let sources: BTreeMap<_, _> =
                        s.durable("sources")?.into_iter().map(|v|
                                    (v["id"].as_str().unwrap().to_owned(), v)).collect();
                    let mut sessions: BTreeMap<String, Value> = BTreeMap::new();
                    for raw in s.durable("captures")? {
                        let c: Capture = uasf_json::from_value(raw)?;
                        c.validate()?;
                        let engine = sources[&c.source]["engine"].as_str().unwrap();
                        let result = provider.read(&c, 0, c.length);
                        let count = result.data.as_ref().filter(|d| d.len() as u64 == c.length && sha(d) == c.sha256)
                            .map(|d| d.split_inclusive(|b| *b == b'\n').count() as u64).unwrap_or(c.length.min(crate::limits::RECORDS as u64 + 1));
                        let mut streamed_compaction = false;
                        let mut emit = |kind: &str, value: &Value| {
                            streamed_compaction |= kind == "event" && value.get("compaction").is_some();
                            s.put(kind, value)
                        };
                        let bundle = parser::parse_into(&c, engine, &result, version,
                            if engine == "claude" { Some(&mut emit) } else { None })?;
                        let cap = &bundle["capture"][0];
                        let old: String =
                            s.db.query_row("SELECT value FROM durable.provenance WHERE id=?",
                                    [c.id()], |r| r.get(0))?;
                        let mut p: Value = uasf_json::from_str(&old)?;
                        // A non-authoritative scalar cache survives index removal.
                        // It never changes provenance, bodies or deletion authority.
                        let prior: Option<i64> = s.db.query_row("SELECT records FROM durable.query_records WHERE id=?", [c.id()], |r| r.get(0)).optional()?;
                        let count = count.min(prior.map(|n| n as u64).unwrap_or(count));
                        s.db.execute("INSERT OR REPLACE INTO durable.query_records VALUES (?,?)", params![c.id(), count as i64])?;
                        p["observation"] =
                            json!(if
                                arr(&cap["reasons"]).contains(&json!("provider_capture_mismatch")){"mismatch"}else{&result.verification});
                        if p["observation"] == "verified" && result.data.is_some() {
                            let w =
                                bundle["window"].first().cloned().unwrap_or(json!({}));
                            let events = &bundle["event"];
                            let mut bounds: Vec<Value> =
                                if !bundle["window"].is_empty() {
                                    events.iter().map(|e|
                                                json!([e["anchors"][0]["start"],e["ordinal"]])).collect()
                                } else { vec![] };
                            if let Some(n) =
                                    bounds.last().and_then(|b|
                                                b[1].as_u64()).and_then(|n| n.checked_add(1)) {
                                bounds.push(json!([c.length,n]));
                            }
                            p["trusted"] =
                                json!({"parser_version":cap["parser_version"],"shape_status":cap["status"],"history_base":w["history_base"],"forked_from_id":w["forked_from_id"],"forked_from_ordinal_exclusive":w["forked_from_ordinal_exclusive"],"boundaries":bounds,"has_compaction":streamed_compaction || events.iter().any(|e|e.get("compaction").is_some())});
                        }
                        s.insert("provenance", &c.id(), &p, false)?;
                        for (kind, values) in &bundle {
                            for v in values { s.put(kind, v)?; }
                        }
                        let session =
                            sessions.entry(c.session_id()).or_insert_with(||
                                    json!({"id":c.session_id(),"source":c.source,"session":c.session,"native_id":if
                                            c.native_session
                                            {json!(c.session)}else{Value::Null},"engine":engine,"capture_ids":[],"anchors":[],"status":"complete","reasons":[],"parser_version":format!("{engine}/{version}")}));
                        if cap["parser_version"] == format!("claude/{}", parser::PERMISSION_MODE_VERSION) {
                            session["parser_version"] = cap["parser_version"].clone();
                            indexed_version = parser::PERMISSION_MODE_VERSION;
                        }
                        session["capture_ids"].as_array_mut().unwrap().push(json!(c.id()));
                        session["anchors"].as_array_mut().unwrap().push(c.anchor());
                        session["status"] =
                            json!(worst([session["status"].as_str().unwrap(),cap["status"].as_str().unwrap()]));
                        session["reasons"] =
                            json!(sorted_unique(strings(&session["reasons"]).into_iter().chain(strings(&cap["reasons"])).collect()));
                    }
                    for v in sessions.values() { s.put("session", v)?; }
                    s.db.execute("INSERT OR REPLACE INTO metadata VALUES ('parser_version',?)",
                            [indexed_version])?;
                    s.refresh_scope_summary()?;
                    Ok(json!({"status":"complete","captures_indexed":s.durable("captures")?.len(),"meaning":"rebuild_finished_not_history_completeness","parser_version":indexed_version}))
                })
    }
    /// Erase explicitly selected indexed bodies and durable evidence. Provider
    /// bytes are outside this operation's scope. Validate the entire batch first.
    pub fn delete(&self, deletions: &Value) -> Result<Value> {
        let items = deletions.as_array().filter(|v| !v.is_empty())
            .ok_or(Error::Invalid("deletion_list_required"))?;
        let mut values = Vec::new();
        for d in items {
            let o = d.as_object().ok_or(Error::Invalid("invalid_deletion_selector"))?;
            let v = if d["kind"] == "session" && o.len() == 3 && nonempty(&d["source"]) && nonempty(&d["session"]) {
                json!({"kind":"session","session_id":session_key(d["source"].as_str().unwrap(), d["session"].as_str().unwrap()),"reason":"explicit_session_deletion"})
            } else if d["kind"] == "range" && o.len() == 2 && o.contains_key("anchor") {
                validate_anchor(&d["anchor"])?;
                let mut v = d["anchor"].clone();
                v["kind"] = json!("range");
                v
            } else {
                return Err(Error::Invalid("invalid_deletion_selector"));
            };
            values.push(v);
        }
        self.transaction(|s| {
            for v in &values { s.insert("tombstones", &key("tombstone", json!([v])), v, false)?; }
            for v in &values {s.redact_selector(v)?;}
            Ok(json!({"status":"deleted","requested":values.len(),"body_policy":"explicit_erasure_and_reversible_dependency_blocking","scope":"core_index_and_durable_evidence_only"}))
        })
    }
    fn redact_rows(&self, objects: Vec<(String,String)>, evidence: Vec<String>)->Result<()> {
        for (kind,id) in objects {
            let raw:String=self.db.query_row("SELECT value FROM objects WHERE kind=? AND id=?",params![kind,id],|r|r.get(0))?;
            let mut v:Value=uasf_json::from_str(&raw)?;
            if self.explicitly_deleted_indexed(&v,&kind)? {erase_body(&mut v);self.put(&kind,&v)?;}
        }
        for id in evidence {
            let raw:String=self.db.query_row("SELECT value FROM durable.evidence WHERE id=?",[&id],|r|r.get(0))?;
            let mut v:Value=uasf_json::from_str(&raw)?;
            if self.explicitly_deleted_indexed(&v,"evidence")? {erase_body(&mut v);self.insert("evidence",&id,&v,false)?;}
        }
        Ok(())
    }
    fn redact_anchor(&self,a:&Value)->Result<()> {
        let source=a["source"].as_str().unwrap_or("");let generation=a["generation"].as_str().unwrap_or("");let capture=a["capture"].as_str().unwrap_or("");
        let objects={let mut q=self.db.prepare("SELECT kind,id FROM scope_anchors WHERE source=? AND generation=? AND capture=?")?;
            let rows=q.query_map(params![source,generation,capture],|r|Ok((r.get(0)?,r.get(1)?)))?;rows.collect::<std::result::Result<Vec<_>,_>>()?};
        let evidence={let mut q=self.db.prepare("SELECT id FROM durable.scope_evidence_anchors WHERE source=? AND generation=? AND capture=?")?;
            let rows=q.query_map(params![source,generation,capture],|r|r.get(0))?;rows.collect::<std::result::Result<Vec<_>,_>>()?};
        self.redact_rows(objects,evidence)
    }
    fn redact_selector(&self,v:&Value)->Result<()> {
        if v["kind"]=="range" {return self.redact_anchor(v);}
        let sid=v["session_id"].as_str().ok_or(Error::Invalid("tombstone_session"))?;
        for kind in ["capture","event","window","turn","edge","session"] {
            let field=if kind=="session" {"id"}else{"json_extract(value,'$.session_id')"};
            let ids={let mut q=self.db.prepare(&format!("SELECT kind,id FROM objects WHERE kind=? AND {field}=?"))?;
                let rows=q.query_map(params![kind,sid],|r|Ok((r.get(0)?,r.get(1)?)))?;rows.collect::<std::result::Result<Vec<_>,_>>()?};
            self.redact_rows(ids,vec![])?;
        }
        let evidence={let mut q=self.db.prepare("SELECT id FROM durable.evidence WHERE json_extract(value,'$.session_id')=?")?;
            let rows=q.query_map([sid],|r|r.get(0))?;rows.collect::<std::result::Result<Vec<_>,_>>()?};
        self.redact_rows(vec![],evidence)?;
        // A session erasure also affects evidence anchored to its captures in
        // another session. Provenance stores the exact session identity.
        let anchors={let mut q=self.db.prepare("SELECT json_extract(value,'$.anchor') FROM durable.provenance WHERE json_extract(value,'$.session_id')=?")?;
            let rows=q.query_map([sid],|r|r.get::<_,String>(0))?;rows.collect::<std::result::Result<Vec<_>,_>>()?};
        for a in anchors {self.redact_anchor(&uasf_json::from_str(&a)?)?;}
        self.db.execute("UPDATE durable.names SET value=json_set(value,'$.display_name',null) WHERE id=?",[sid])?;
        Ok(())
    }
    pub fn query(&self, request: &Value) -> Result<Value> {
        let _deletion = match self.deletion_lease() { Ok(v)=>v, Err(_)=>return Ok(crate::deletion_binding::pending()) };
        { let mut metrics = self.scope_metrics.borrow_mut(); metrics.output_estimate_bytes = None; metrics.serialized_output_bytes = None; }
        self.db.execute_batch("BEGIN")?;
        let result = crate::query::query(self, request).and_then(|v| {
            let size = crate::output::value_size(&v)?;
            self.scope_output_size(size)?; Ok(v)
        });
        self.scope_reset();
        self.db.execute_batch("ROLLBACK")?;
        result
    }
}

fn erase_body(v: &mut Value) {
    if v.get("body").is_some() { v["body"] = Value::Null; }
    v["status"] = json!("deleted");
    v["reasons"] = json!(["explicit_deletion"]);
}
