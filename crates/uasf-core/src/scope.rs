//! Bounded policy closure. Response membership remains the explicit boundary.
//! These indexes accelerate observation lookup; tombstones remain authoritative.
use std::collections::{BTreeSet, VecDeque};
use rusqlite::{params, OptionalExtension, StatementStatus};
use uasf_json::{json, Value};
use crate::{Core, model::*, limits::*};

#[derive(Default, Clone, serde::Serialize)]
pub struct ScopeMetrics { pub rows: u64, pub decoded_bytes: u64, pub vm_steps: i64, pub fullscan_steps: i64, pub records: i64, pub edges: i64, pub response_rows: u64, pub response_decode_bytes: u64, pub output_estimate_bytes: Option<u64>, pub serialized_output_bytes: Option<u64> }
impl Core {
    pub(crate) fn scope_schema(&self) -> Result<()> {
        self.db.execute_batch("BEGIN IMMEDIATE;
        CREATE INDEX IF NOT EXISTS durable.scope_capture_session ON captures(json_extract(value,'$.source'),json_extract(value,'$.session'),id);
        CREATE INDEX IF NOT EXISTS durable.scope_capture_window ON captures(json_extract(value,'$.source'),json_extract(value,'$.window'),id);
        CREATE INDEX IF NOT EXISTS durable.scope_capture_anchor ON captures(json_extract(value,'$.source'),json_extract(value,'$.generation'),json_extract(value,'$.capture'),id);
        CREATE INDEX IF NOT EXISTS durable.scope_provenance_session ON provenance(json_extract(value,'$.session_id'),id);
        CREATE INDEX IF NOT EXISTS durable.scope_provenance_base ON provenance(json_extract(value,'$.source'),json_extract(value,'$.trusted.history_base.thread_id'),id);
        CREATE INDEX IF NOT EXISTS durable.scope_evidence_session ON evidence(json_extract(value,'$.session_id'),id);
        CREATE INDEX IF NOT EXISTS durable.scope_tombstone_session ON tombstones(json_extract(value,'$.session_id'),id);
        CREATE INDEX IF NOT EXISTS durable.scope_tombstone_anchor ON tombstones(json_extract(value,'$.source'),json_extract(value,'$.generation'),json_extract(value,'$.capture'),id);
        CREATE INDEX IF NOT EXISTS scope_objects_capture ON objects(kind,json_extract(value,'$.capture_id'),id);
        CREATE INDEX IF NOT EXISTS scope_objects_session ON objects(kind,json_extract(value,'$.session_id'),id);
        CREATE TABLE IF NOT EXISTS scope_summary(id TEXT PRIMARY KEY,rows INTEGER NOT NULL,bytes INTEGER NOT NULL,edges INTEGER NOT NULL,anchor INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS scope_ready(id TEXT PRIMARY KEY,provenance TEXT NOT NULL);
        COMMIT;
        CREATE TEMP TABLE IF NOT EXISTS scope_ids(kind TEXT,id TEXT,PRIMARY KEY(kind,id));")?;
        let initialized:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE id='scope_schema_v1')",[],|r|r.get(0))?;
        self.db.execute_batch("BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS scope_anchors(kind TEXT,id TEXT,source TEXT,generation TEXT,capture TEXT,PRIMARY KEY(kind,id,source,generation,capture));
        CREATE INDEX IF NOT EXISTS scope_anchor_lookup ON scope_anchors(source,generation,capture,kind,id);
        CREATE TABLE IF NOT EXISTS durable.scope_evidence_anchors(id TEXT,source TEXT,generation TEXT,capture TEXT,PRIMARY KEY(id,source,generation,capture));
        CREATE INDEX IF NOT EXISTS durable.scope_evidence_anchor_lookup ON scope_evidence_anchors(source,generation,capture,id);
        CREATE INDEX IF NOT EXISTS scope_native_candidates ON objects(kind,json_extract(value,'$.source'),json_extract(value,'$.native_id'),id);
        CREATE INDEX IF NOT EXISTS scope_relation_keys ON objects(kind,json_extract(value,'$.source'),json_extract(value,'$.target'),id);
        ")?;
        for (schema,table,target,kind) in [("main","objects","scope_anchors",true),("durable","evidence","scope_evidence_anchors",false)] {
            let columns=if kind {"kind,id,source,generation,capture"}else{"id,source,generation,capture"};
            let prefix=if kind {"NEW.kind,NEW.id"}else{"NEW.id"};
            let predicate=if kind {"kind=OLD.kind AND id=OLD.id"}else{"id=OLD.id"};
            for action in ["INSERT","UPDATE","DELETE"] {
                let remove=if action!="INSERT" {format!("DELETE FROM {target} WHERE {predicate};")} else {String::new()};
                let insert=if action!="DELETE" {format!("INSERT OR IGNORE INTO {target} ({columns}) SELECT {prefix},json_extract(value,'$.source'),json_extract(value,'$.generation'),json_extract(value,'$.capture') FROM json_each(NEW.value,'$.anchors');")} else {String::new()};
                self.db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS {schema}.scope_{table}_{action} AFTER {action} ON {table} BEGIN {remove}{insert} END;"))?;
            }
        }
        if !initialized {
            self.db.execute_batch("INSERT OR IGNORE INTO scope_anchors SELECT o.kind,o.id,json_extract(a.value,'$.source'),json_extract(a.value,'$.generation'),json_extract(a.value,'$.capture') FROM objects o,json_each(o.value,'$.anchors') a;
            INSERT OR IGNORE INTO durable.scope_evidence_anchors SELECT o.id,json_extract(a.value,'$.source'),json_extract(a.value,'$.generation'),json_extract(a.value,'$.capture') FROM durable.evidence o,json_each(o.value,'$.anchors') a;
            INSERT OR REPLACE INTO metadata VALUES ('scope_schema_v1','1');")?;
        }
        let sizes_exist:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM durable.sqlite_master WHERE type='table' AND name='scope_sizes')",[],|r|r.get(0))?;
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS durable.scope_sizes(kind TEXT,id TEXT,bytes INTEGER NOT NULL,PRIMARY KEY(kind,id));")?;
        for table in ["sources","captures","names","evidence","provenance","tombstones"] {
            for action in ["INSERT","UPDATE","DELETE"] {
                let remove=if action!="INSERT" {format!("DELETE FROM scope_sizes WHERE kind='{table}' AND id=OLD.id;")} else {String::new()};
                let insert=if action!="DELETE" {format!("INSERT OR REPLACE INTO scope_sizes VALUES ('{table}',NEW.id,length(CAST(NEW.value AS BLOB)));")} else {String::new()};
                self.db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS durable.scope_sizes_{table}_{action} AFTER {action} ON {table} BEGIN {remove}{insert} END;"))?;
            }
            if !sizes_exist {self.db.execute_batch(&format!("INSERT INTO durable.scope_sizes SELECT '{table}',id,length(CAST(value AS BLOB)) FROM durable.{table};"))?;}
        }
        self.db.execute_batch("COMMIT")?;
        Ok(())
    }
    /// Closure rows and response decode counts are separate. VM counters cover
    /// candidate lookups, decoded policy lookups and projection cursor statements;
    /// scalar readiness checks and transaction setup are not included.
    pub fn scope_metrics(&self) -> ScopeMetrics { self.scope_metrics.borrow().clone() }
    pub(crate) fn scope_reset(&self) { self.scope_active.set(false); }
    fn scoped_ids(&self, table: &str, predicate: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<String>> {
        let mut q=self.db.prepare(&format!("SELECT id FROM {table} WHERE {predicate} ORDER BY id LIMIT 10001"))?;
        let mut out=Vec::new();
        { let mut rows=q.query(args)?; while let Some(r)=rows.next()? {
            self.scope_metrics.borrow_mut().rows+=1;
            if self.scope_metrics.borrow().rows>RECORDS as u64 {return Err(Error::Invalid("query_policy_limit"));}
            out.push(r.get(0)?);
        }}
        self.scope_metrics.borrow_mut().vm_steps+=q.get_status(StatementStatus::VmStep) as i64;
        self.scope_metrics.borrow_mut().fullscan_steps+=q.get_status(StatementStatus::FullscanStep) as i64;
        Ok(out)
    }
    fn scope_row(&self, table:&str,id:&str) -> Result<Option<Value>> {
        if self.db.query_row("SELECT EXISTS(SELECT 1 FROM temp.scope_ids WHERE kind=? AND id=?)",params![table,id],|r|r.get::<_,bool>(0))? {return Ok(None);}
        let length:Option<i64>=self.db.query_row("SELECT bytes FROM durable.scope_sizes WHERE kind=? AND id=?",params![table,id],|r|r.get(0)).optional()?;
        let Some(length)=length else {
            let exists:bool=self.db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM durable.{table} WHERE id=?)"),[id],|r|r.get(0))?;
            return if exists {Err(Error::Invalid("query_scope_unavailable"))} else {Ok(None)};
        };
        {let mut m=self.scope_metrics.borrow_mut();m.rows+=1;m.decoded_bytes=m.decoded_bytes.saturating_add(length as u64);
        if m.rows>RECORDS as u64{return Err(Error::Invalid("query_policy_limit"));}
        if m.decoded_bytes>BYTES as u64{return Err(Error::Invalid("query_materialization_limit"));}}
        let mut q=self.db.prepare(&format!("SELECT value FROM durable.{table} WHERE id=?"))?;
        let mut rows=q.query([id])?;
        let row=rows.next()?.ok_or(Error::Invalid("query_scope_unavailable"))?;
        let raw=row.get_ref(0)?.as_str().map_err(|_|Error::Invalid("stored_json_text"))?;
        let value=uasf_json::from_str(raw)?;
        drop(rows);
        self.scope_metrics.borrow_mut().vm_steps+=q.get_status(StatementStatus::VmStep) as i64;
        self.db.execute("INSERT INTO temp.scope_ids VALUES (?,?)",params![table,id])?;
        Ok(Some(value))
    }
    pub(crate) fn scope_prepare(&self,r:&Value)->Result<()> {
        self.scope_active.set(false); *self.scope_metrics.borrow_mut()=ScopeMetrics::default();
        self.db.execute("DELETE FROM temp.scope_ids",[])?;
        // Pin the two databases in the same read transaction, without COUNT scans.
        self.db.query_row("SELECT (SELECT value FROM metadata WHERE id='parser_version'),(SELECT epoch FROM durable.query_generation WHERE id=1)",[],|_|Ok(()))?;
        let op=r["op"].as_str().unwrap_or("");
        if op=="sources" {
            for id in self.scoped_ids("durable.sources","1",&[])? {self.scope_row("sources",&id)?;}
            self.scope_active.set(true);return Ok(());
        }
        let source=r["source"].as_str().unwrap_or(""); let session=r["session"].as_str().unwrap_or("");
        if arr(&r["boundary"]).len()>RECORDS as usize{return Err(Error::Invalid("query_policy_limit"));}
        let mut pending=VecDeque::new(); let mut sessions=BTreeSet::new();
        if op=="sessions" {
            let ids=if r.get("source").is_some(){ self.scoped_ids("durable.captures","json_extract(value,'$.source')=?",&[&source])? }
                else { self.scoped_ids("durable.captures","1",&[])? };
            pending.extend(ids.into_iter().map(|id|(id,0)));
        } else {
            sessions.insert(session_key(source,session));
            pending.extend(self.scoped_ids("durable.captures","json_extract(value,'$.source')=? AND json_extract(value,'$.session')=?",&[&source,&session])?.into_iter().map(|id|(id,0)));
            pending.extend(strings(&r["boundary"]).into_iter().map(|id|(id,0)));
        }
        let mut anchors=BTreeSet::new();
        if op=="evidence" {
            let sid=session_key(source,session);
            for id in self.scoped_ids("durable.evidence","json_extract(value,'$.session_id')=?",&[&sid])? {
                if let Some(e)=self.scope_row("evidence",&id)? {
                    for a in arr(&e["anchors"]) {
                        let key=(a["source"].as_str().unwrap_or("").to_owned(),a["generation"].as_str().unwrap_or("").to_owned(),a["capture"].as_str().unwrap_or("").to_owned());
                        for id in self.scoped_ids("durable.captures","json_extract(value,'$.source')=? AND json_extract(value,'$.generation')=? AND json_extract(value,'$.capture')=?",&[&key.0,&key.1,&key.2])? { pending.push_back((id,0)); }
                        anchors.insert(key);
                    }
                }
            }
        }
        let mut seen=BTreeSet::new(); let mut parents=std::collections::BTreeMap::new();
        let mut material=0i64;
        while let Some((id,depth))=pending.pop_front() {
            if depth>DEPTH {return Err(Error::Invalid("query_depth_limit"));}
            if !seen.insert(id.clone()){continue;}
            let Some(raw)=self.scope_row("captures",&id)? else {continue};
            let c:Capture=uasf_json::from_value(raw)?;
            sessions.insert(c.session_id());anchors.insert((c.source.clone(),c.generation.clone(),c.capture.clone()));
            self.scope_row("sources",&c.source)?;
            let p=self.scope_row("provenance",&id)?.ok_or(Error::Invalid("query_scope_unavailable"))?;
            let count:i64=self.db.query_row("SELECT records FROM durable.query_records WHERE id=?",[&id],|r|r.get(0)).optional()?.unwrap_or(c.length.min(RECORDS as u64+1) as i64).max(1);
            self.scope_metrics.borrow_mut().records+=count;
            if self.scope_metrics.borrow().records>RECORDS {return Err(Error::Invalid("query_record_limit"));}
            let summary:Option<(i64,i64,i64)>=self.db.query_row("SELECT bytes,edges,anchor FROM scope_summary WHERE id=?",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((bytes,edges,_anchor))=summary {material=material.saturating_add(bytes);self.scope_metrics.borrow_mut().edges+=edges;}
            if material>BYTES {return Err(Error::Invalid("query_materialization_limit"));}
            if self.scope_metrics.borrow().edges+self.scope_metrics.borrow().records>EDGES {return Err(Error::Invalid("query_edge_limit"));}
            // Readiness applies to response members, not to unobserved policy candidates.
            if op!="evidence" && (arr(&r["boundary"]).contains(&json!(id)) || op=="sessions" || (c.source==source && c.session==session)) {
                let fresh:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM scope_ready i JOIN durable.provenance p ON p.id=i.id AND p.value=i.provenance WHERE i.id=?)",[&id],|r|r.get(0))?;
                if !fresh || summary.is_none(){return Err(Error::Invalid("index_requires_rebuild"));}
            }
            if let Some(base)=p["trusted"]["history_base"]["thread_id"].as_str() {
                let owners=self.scoped_ids("durable.captures","json_extract(value,'$.source')=? AND json_extract(value,'$.window')=?",&[&c.source,&base])?;
                parents.insert(id.clone(),owners.clone());
                for parent in owners {pending.push_back((parent,depth+1));}
            }
        }
        // Check maximum path depth independently of seed order. Cycles retain
        // existing projection diagnostics, rather than expanding forever.
        let mut work=0;
        for id in &seen {
            let mut todo=vec![(id.clone(),BTreeSet::new())];
            while let Some((id,mut path))=todo.pop(){ if !path.insert(id.clone()){continue;}
                if path.len()>DEPTH as usize+1{return Err(Error::Invalid("query_depth_limit"));}
                work+=1;if work>EDGES{return Err(Error::Invalid("query_edge_limit"));}
                for p in parents.get(&id).into_iter().flatten(){todo.push((p.clone(),path.clone()));}
            }
        }
        for sid in &sessions {
            self.scope_row("names",sid)?;
            self.db.execute("INSERT OR IGNORE INTO temp.scope_ids VALUES ('sessions',?)",[sid])?;
            for id in self.scoped_ids("durable.tombstones","json_extract(value,'$.session_id')=?",&[sid])? {self.scope_row("tombstones",&id)?;}
        }
        for (source,generation,capture) in anchors {
            for id in self.scoped_ids("durable.tombstones","json_extract(value,'$.source')=? AND json_extract(value,'$.generation')=? AND json_extract(value,'$.capture')=?",&[&source,&generation,&capture])? {self.scope_row("tombstones",&id)?;}
        }
        let m=self.scope_metrics.borrow();
        let bytes=material.saturating_add(m.decoded_bytes as i64);
        if bytes>BYTES{return Err(Error::Invalid("query_materialization_limit"));}
        // Output is charged from this operation's single resolved projection,
        // before serialization, rather than multiplying stored objects. The
        // shared renderer counts each actual body/anchor reference occurrence.
        // Material, traversal, policy and snapshot limits still apply here.
        if op!="evidence" && !self.db.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE id='parser_version')",[],|r|r.get::<_,bool>(0))? {return Err(Error::Invalid("index_requires_rebuild"));}
        self.scope_active.set(true); Ok(())
    }
    pub(crate) fn scope_output_size(&self, bytes: u64) -> Result<()> {
        self.scope_metrics.borrow_mut().output_estimate_bytes = Some(bytes);
        if bytes > OUTPUT as u64 { return Err(Error::Invalid("query_output_limit")); }
        Ok(())
    }
    pub(crate) fn explicitly_deleted_indexed(&self,v:&Value,kind:&str)->Result<bool> {
        let sid=if kind=="session" {v["id"].as_str()} else {v["session_id"].as_str()};
        if let Some(sid)=sid {if self.session_deleted(sid)? {return Ok(true);}}
        if matches!(kind,"capture"|"session") {return Ok(false);}
        for a in arr(&v["anchors"]) {
            let source=a["source"].as_str().unwrap_or("");let generation=a["generation"].as_str().unwrap_or("");let capture=a["capture"].as_str().unwrap_or("");
            let mut ranges=self.db.prepare("SELECT value FROM durable.tombstones WHERE json_extract(value,'$.source')=? AND json_extract(value,'$.generation')=? AND json_extract(value,'$.capture')=?")?;
            let rows=ranges.query_map(params![source,generation,capture],|r|r.get::<_,String>(0))?;
            for raw in rows {
                let t:Value=uasf_json::from_str(&raw?)?;
                if t["kind"]=="range" && a["start"].as_u64().zip(t["end"].as_u64()).is_some_and(|(a,b)|a<b) && t["start"].as_u64().zip(a["end"].as_u64()).is_some_and(|(a,b)|a<b) {return Ok(true);}
            }
            let mut q=self.db.prepare("SELECT json_extract(value,'$.session') FROM durable.captures WHERE json_extract(value,'$.source')=? AND json_extract(value,'$.generation')=? AND json_extract(value,'$.capture')=?")?;
            let rows=q.query_map(params![source,generation,capture],|r|r.get::<_,String>(0))?;
            for session in rows {if self.session_deleted(&session_key(source,&session?))? {return Ok(true);}}
        }
        Ok(false)
    }
    pub(crate) fn session_deleted(&self,sid:&str)->Result<bool> {
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM durable.tombstones WHERE json_extract(value,'$.session_id')=? AND json_extract(value,'$.kind')='session' AND json_extract(value,'$.reason')='explicit_session_deletion')",[sid],|r|r.get(0))?)
    }
    pub(crate) fn refresh_scope_summary(&self)->Result<()> {
        self.db.execute_batch("DELETE FROM scope_summary;
        INSERT INTO scope_summary SELECT json_extract(value,'$.capture_id'),count(*),sum(length(CAST(value AS BLOB))),sum(kind='edge'),coalesce(max(CASE WHEN kind IN ('event','window') THEN length(CAST(json_extract(value,'$.anchors') AS BLOB)) ELSE 0 END),0) FROM objects WHERE json_extract(value,'$.capture_id') IS NOT NULL GROUP BY json_extract(value,'$.capture_id');
        INSERT OR IGNORE INTO scope_summary SELECT id,0,0,0,0 FROM durable.captures;
        DELETE FROM scope_ready; INSERT INTO scope_ready SELECT id,value FROM durable.provenance;")?; Ok(())
    }
}
