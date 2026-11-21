//! Synthetic-only feasibility probe. No scheduler, consumer access or deletion API.
//! Immutable observations are loaded separately from request-scoped resolutions.
use std::{io::{Write,Seek,BufWriter},path::Path};
use rusqlite::{Connection,params,OptionalExtension};
use uasf_json::{Value,json};
use uasf_core::model::{key,strings,sorted_unique,worst};
pub type Result<T> = std::result::Result<T,Box<dyn std::error::Error>>;
fn text(v:&Value,k:&str)->String {v[k].as_str().unwrap_or("").to_owned()}
fn raw(v:&Value)->String {uasf_json::to_string(v).unwrap()}
fn err<T>(message:&str)->Result<T> {Err(message.to_owned().into())}
fn open(path:&Path)->Result<Connection> {
    let db=Connection::open(path)?;
    db.execute_batch("PRAGMA temp_store=FILE; PRAGMA cache_size=-32768; PRAGMA mmap_size=0; PRAGMA journal_mode=DELETE;")?;Ok(db)
}
/// Load parsed observations, never completed query responses. The input database
/// must be a disposable synthetic copy. Policy-bearing stores are out of scope.
pub fn load(index:&Path,persistent:&Path,destination:&Path)->Result<()> {
    if destination.exists(){return err("probe_destination_exists");}
    let input=Connection::open_with_flags(index,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    input.execute("ATTACH DATABASE ? AS durable",[persistent.to_string_lossy()])?;
    input.execute_batch("BEGIN")?;
    let tombstones:i64=input.query_row("SELECT count(*) FROM durable.tombstones",[],|r|r.get(0))?;
    if tombstones!=0{return err("probe_policy_scope_unsupported");}
    let db=open(destination)?;
    db.execute_batch("BEGIN;
      CREATE TABLE objects(kind TEXT,id TEXT PRIMARY KEY,capture TEXT,pos INTEGER,ty TEXT,native TEXT,turn_id TEXT,status TEXT,value TEXT,anchors TEXT,start INTEGER,end INTEGER,ordinal INTEGER,context TEXT);
      CREATE INDEX by_kind ON objects(kind,capture,pos,id);
      CREATE INDEX by_native ON objects(native);
      CREATE TABLE refs(capture TEXT,kind TEXT,target TEXT,node TEXT);
      CREATE INDEX refs_key ON refs(capture,kind,target);
      CREATE TABLE edge(id TEXT PRIMARY KEY,capture TEXT,origin TEXT,kind TEXT,target TEXT,ref_kind TEXT);
      CREATE INDEX edge_capture ON edge(capture,id);
      CREATE INDEX edge_origin ON edge(origin,kind,id);
      CREATE TABLE member(turn_id TEXT,event TEXT,PRIMARY KEY(turn_id,event));
      CREATE TABLE captures(id TEXT PRIMARY KEY,source TEXT,session TEXT,engine TEXT,length INTEGER,value TEXT);
      CREATE TABLE windows(id TEXT PRIMARY KEY,capture TEXT,source TEXT,session TEXT,value TEXT);
      CREATE TABLE info(id TEXT PRIMARY KEY,value TEXT);")?;
    let mut q=input.prepare("SELECT kind,value FROM objects ORDER BY kind,id")?;
    let mut rows=q.query([])?;
    let mut insert=db.prepare("INSERT INTO objects VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")?;
    while let Some(row)=rows.next()? {
        let kind:String=row.get(0)?;let mut v=uasf_json::from_str(&row.get::<_,String>(1)?)?;
        if v["status"]=="deleted" {return err("probe_deleted_scope_unsupported");}
        v["body_access"]=json!("available");
        let id=text(&v,"id");let cap=text(&v,"capture_id");
        let anchors=raw(&v["anchors"]);
        insert.execute(params![kind,id,cap,v["position"].as_i64().unwrap_or(0),text(&v,"type"),raw(&v["native_id"]),text(&v,"turn_id"),text(&v,"status"),raw(&v),anchors,v["anchors"][0]["start"].as_i64(),v["anchors"][0]["end"].as_i64(),v["ordinal"].as_i64(),raw(&v["compaction"]["context_window_id"])])?;
        if kind=="capture" {
            let engine:String=input.query_row("SELECT json_extract(value,'$.engine') FROM durable.sources WHERE id=?",[text(&v,"source")],|r|r.get(0))?;
            if engine=="codex" {
                let good:bool=input.query_row("SELECT json_extract(value,'$.observation')='verified' AND json_extract(value,'$.trusted.shape_status')='complete' FROM durable.provenance WHERE id=?",[&id],|r|r.get(0))?;
                if !good{return err("probe_unverified_codex_scope_unsupported");}
            }
            db.execute("INSERT INTO captures VALUES(?,?,?,?,?,?)",params![id,text(&v,"source"),text(&v,"session_id"),engine,v["length"].as_i64(),raw(&v)])?;
        }
        if kind=="window" {db.execute("INSERT INTO windows VALUES(?,?,?,?,?)",params![text(&v,"window_id"),cap,text(&v,"source"),text(&v,"session_id"),raw(&v)])?;}
        if kind=="event" {
            for (field,ref_kind) in [("native_id","uuid"),("request_id","request"),("reference_hash",if v["type"]=="api-request-blob" {"blob"} else {"shape"})] {
                if uasf_core::model::truth(&v[field]) {db.execute("INSERT INTO refs VALUES(?,?,?,?)",params![cap,ref_kind,raw(&v[field]),id])?;}
            }
        }
        if kind=="edge" {
            let k=text(&v,"kind");let ref_kind=match k.as_str(){"requestRef"=>"request","requestBlob"=>"blob","requestShape"=>"shape",_=>"uuid"};
            db.execute("INSERT INTO edge VALUES(?,?,?,?,?,?)",params![id,cap,text(&v,"event_id"),k,raw(&v["target"]),ref_kind])?;
        }
        if kind=="turn" {for event in strings(&v["event_ids"]) {db.execute("INSERT OR IGNORE INTO member VALUES(?,?)",params![id,event])?;}}
    }
    drop(insert);drop(rows);drop(q);
    db.execute("INSERT INTO info VALUES('format','probe/1')",[])?;
    db.execute_batch("COMMIT; PRAGMA optimize;")?; input.execute_batch("ROLLBACK")?;Ok(())
}
struct Output<'a,W:Write>{out:&'a mut W,first:bool}
impl<W:Write> Output<'_,W>{fn item(&mut self,bytes:&str)->Result<()> {if !self.first {self.out.write_all(b",")?;} self.first=false;self.out.write_all(bytes.as_bytes())?;Ok(())}}
fn array(db:&Connection,sql:&str,out:&mut impl Write)->Result<()> {
    out.write_all(b"[")?;let mut a=Output{out,first:true};let mut q=db.prepare(sql)?;let mut rows=q.query([])?;
    while let Some(r)=rows.next()?{a.item(r.get_ref(0)?.as_str()?)?;}out.write_all(b"]")?;Ok(())
}
fn anchors(db:&Connection,sql:&str,out:&mut impl Write)->Result<()> {
    out.write_all(b"[")?;let mut a=Output{out,first:true};let mut q=db.prepare(sql)?;let mut rows=q.query([])?;
    while let Some(r)=rows.next()? {let s:String=r.get(0)?;let inner=&s[1..s.len()-1];if !inner.is_empty(){a.item(inner)?;}}
    out.write_all(b"]")?;Ok(())
}
fn quote(s:&str)->String {format!("'{}'",s.replace('\'',"''"))}
fn envelope_start(out:&mut impl Write,status:&str,reasons:Vec<String>)->Result<()> {
    write!(out,"{{\"schema\":\"uasf-core.query/1\",\"op\":\"history\",\"status\":{},\"reasons\":{},\"data\":",raw(&json!(status)),raw(&json!(sorted_unique(reasons))))?;Ok(())
}
fn prepare(db:&Connection,r:&Value)->Result<(String,String)> {
    if r["op"]!="history" {return err("probe_operation_unsupported");}
    db.execute_batch("DROP TABLE IF EXISTS temp.chosen; CREATE TEMP TABLE chosen(id TEXT PRIMARY KEY,ord INTEGER);")?;
    for (i,id) in r["boundary"].as_array().ok_or("boundary")?.iter().enumerate(){db.execute("INSERT INTO chosen VALUES(?,?)",params![id.as_str().ok_or("capture id")?,i as i64])?;}
    let sid=uasf_core::model::session_key(r["source"].as_str().ok_or("source")?,r["session"].as_str().ok_or("session")?);
    let bad:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM chosen x LEFT JOIN captures c ON c.id=x.id WHERE c.id IS NULL OR c.source!=?)",[text(r,"source")],|r|r.get(0))?;
    if bad{return err("probe_boundary_unsupported");}
    let engine:String=db.query_row("SELECT engine FROM captures c JOIN chosen x ON c.id=x.id WHERE c.session=? LIMIT 1",[&sid],|r|r.get(0))?;
    Ok((engine,sid))
}
/// Complete serialization precedes delivery. Scratch DB tables and the output
/// file are disposable; this is not a production authorization boundary.
pub fn query(path:&Path,r:&Value,out:&mut impl Write)->Result<()> {
    let db=open(path)?;db.execute_batch("BEGIN")?;
    let (engine,sid)=prepare(&db,r)?;
    // A named scratch file is intentional for measurement. Owner-only fixture
    // directories and exclusive creation prevent replacement; Drop removes it.
    let scratch=path.with_extension(format!("{}.output",std::process::id()));
    let file=std::fs::OpenOptions::new().write(true).read(true).create_new(true).open(&scratch)?;
    struct Cleanup(std::path::PathBuf);impl Drop for Cleanup{fn drop(&mut self){let _=std::fs::remove_file(&self.0);}}
    let _cleanup=Cleanup(scratch);let mut spool=BufWriter::with_capacity(65536,file);
    match engine.as_str(){"claude"=>claude(&db,r,&sid,&mut spool)?,"codex"=>codex(&db,r,&sid,&mut spool)?,_=>return err("probe_engine_unsupported")};
    spool.flush()?;spool.get_mut().rewind()?;db.execute_batch("ROLLBACK")?;std::io::copy(spool.get_mut(),out)?;Ok(())
}
fn claude(db:&Connection,r:&Value,sid:&str,out:&mut impl Write)->Result<()> {
    let caps:Vec<String>=db.prepare("SELECT c.value FROM captures c JOIN chosen x ON x.id=c.id WHERE c.session=?")?.query_map([sid],|r|r.get(0))?.collect::<std::result::Result<_,_>>()?;
    if caps.len()!=1{return err("probe_claude_selection_unsupported");}
    let cap=uasf_json::from_str(&caps[0])?;let capid=text(&cap,"id");
    if ["unsupported","incomplete"].contains(&cap["status"].as_str().unwrap_or("")){return err("probe_capture_shape_unsupported");}
    db.execute_batch("DROP TABLE IF EXISTS temp.candidates; DROP TABLE IF EXISTS temp.resolved; DROP TABLE IF EXISTS temp.parent; DROP TABLE IF EXISTS temp.chain;
      CREATE TEMP TABLE candidates(kind TEXT,target TEXT,n INTEGER,node TEXT,PRIMARY KEY(kind,target));
      CREATE TEMP TABLE resolved(id TEXT PRIMARY KEY,node TEXT);
      CREATE TEMP TABLE parent(id TEXT PRIMARY KEY,parent TEXT); CREATE INDEX temp.parent_reverse ON parent(parent);
      CREATE TEMP TABLE chain(seq INTEGER PRIMARY KEY,node TEXT UNIQUE);")?;
    db.execute("INSERT INTO candidates SELECT kind,target,count(*),min(node) FROM refs WHERE capture=? GROUP BY kind,target",[&capid])?;
    db.execute("INSERT INTO resolved SELECT e.id,CASE WHEN c.n=1 THEN c.node END FROM edge e LEFT JOIN candidates c ON c.kind=e.ref_kind AND c.target=e.target WHERE e.capture=?",[&capid])?;
    db.execute("INSERT INTO parent SELECT o.id,(SELECT rr.node FROM edge e JOIN resolved rr ON rr.id=e.id WHERE e.origin=o.id AND e.kind='parent' ORDER BY e.id DESC LIMIT 1) FROM objects o WHERE o.kind='event' AND o.capture=?",[&capid])?;
    let mut status=if cap["status"]=="complete" {"unknown".to_owned()}else{text(&cap,"status")};let mut reasons=strings(&cap["reasons"]);reasons.extend(["native_authority_not_compared".into(),"model_input_unknown".into()]);
    let duplicate:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM candidates WHERE n>1)",[],|r|r.get(0))?;
    if duplicate{status=worst([status.as_str(),"unsupported"]).into();reasons.push("duplicate_native_reference".into());}
    for kind in db.prepare("SELECT DISTINCT e.kind FROM edge e JOIN resolved rr ON rr.id=e.id WHERE rr.node IS NULL ORDER BY e.kind")?.query_map([],|r|r.get::<_,String>(0))? {
        let k=kind?;if k=="parent" {status=worst([status.as_str(),"incomplete"]).into();}reasons.push(format!("unresolved_{k}"));
    }
    let cycle:bool=db.query_row("WITH RECURSIVE rooted(id) AS (SELECT id FROM parent WHERE parent IS NULL UNION SELECT p.id FROM parent p JOIN rooted r ON p.parent=r.id) SELECT (SELECT count(*) FROM rooted)!=(SELECT count(*) FROM parent)",[],|r|r.get(0))?;
    if cycle {status=worst([status.as_str(),"unsupported"]).into();reasons.push("parent_cycle".into());}
    if r["leaf"].is_null(){reasons.push("observed_graph_without_leaf_selection".into());}else{
        let start:Option<String>=db.query_row("SELECT node FROM candidates WHERE kind='uuid' AND target=? AND n=1",[raw(&r["leaf"])],|r|r.get(0)).optional()?;
        if start.is_none(){status=worst([status.as_str(),"incomplete"]).into();reasons.push("leaf_missing_or_ambiguous".into());}
        let mut node=start;let mut seq=0;let mut get=db.prepare("SELECT parent FROM parent WHERE id=?")?;let mut put=db.prepare("INSERT OR IGNORE INTO chain VALUES(?,?)")?;
        while let Some(n)=node {if put.execute(params![seq,n])?==0{break;}seq+=1;node=get.query_row([&n],|r|r.get::<_,Option<String>>(0))?;}
    }
    envelope_start(out,&status,reasons)?;
    write!(out,"{{\"projection\":\"claude_observed_graph\",\"leaf\":{},\"uuidless_event_ids\":",raw(&r["leaf"]))?;
    array(db,&format!("SELECT json_quote(id) FROM objects WHERE kind='event' AND capture={} AND native='null' ORDER BY pos,id",quote(&capid)),out)?;
    out.write_all(b",\"observed_chain\":")?;array(db,"SELECT json_quote(node) FROM chain ORDER BY seq",out)?;
    out.write_all(b",\"comparison\":\"not_compared\",\"active_context\":\"unknown\",\"model_input\":\"unknown\",\"exposed\":\"unknown\",\"outside_chain_is_excluded\":false,\"events\":")?;
    array(db,&format!("SELECT value FROM objects WHERE kind='event' AND capture={} ORDER BY pos,id",quote(&capid)),out)?;
    out.write_all(b",\"edges\":[")?;let mut a=Output{out,first:true};
    let mut q=db.prepare("SELECT o.value,t.value FROM edge e JOIN resolved rr ON rr.id=e.id JOIN objects o ON o.id=e.id LEFT JOIN objects t ON t.id=rr.node ORDER BY e.id")?;let mut rows=q.query([])?;
    while let Some(row)=rows.next()? {
        let mut edge=uasf_json::from_str(&row.get::<_,String>(0)?)?;
        if let Some(target)=row.get::<_,Option<String>>(1)? {let target=uasf_json::from_str(&target)?;edge["target_event_id"]=target["id"].clone();edge["target_anchors"]=target["anchors"].clone();edge["status"]=json!("complete");edge["reasons"]=json!([]);if edge["kind"]=="compaction"{edge["assessment"]=json!("boundary_reference_only");edge["input_membership"]=json!("unknown");}}
        else{edge["status"]=json!("unknown");edge["reasons"]=json!(["unresolved_or_ambiguous_reference"]);}
        a.item(&raw(&edge))?;
    }
    out.write_all(b"]}}")?;Ok(())
}
fn codex(db:&Connection,r:&Value,sid:&str,out:&mut impl Write)->Result<()> {
    let bad:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM captures c JOIN chosen x ON x.id=c.id WHERE json_extract(c.value,'$.status')!='complete')",[],|r|r.get(0))?;
    if bad{return err("probe_codex_shape_unsupported");}
    db.execute_batch("DROP TABLE IF EXISTS temp.selected; DROP TABLE IF EXISTS temp.active; DROP TABLE IF EXISTS temp.excluded;
      CREATE TEMP TABLE selected(seq INTEGER PRIMARY KEY,window TEXT UNIQUE,capture TEXT UNIQUE,end INTEGER,child TEXT,value TEXT);
      CREATE TEMP TABLE active(seq INTEGER PRIMARY KEY,event TEXT UNIQUE,turn_id TEXT);
      CREATE INDEX temp.active_turn ON active(turn_id,seq);
      CREATE TEMP TABLE excluded(seq INTEGER PRIMARY KEY,event TEXT UNIQUE,turn_id TEXT);
      CREATE INDEX temp.excluded_turn ON excluded(turn_id,seq);")?;
    let mut current=r["leaf"].as_str().ok_or("probe_leaf_required")?.to_owned();let mut incoming=Value::Null;let mut child=Value::Null;let mut depth=0;
    loop {
        let w:String=db.query_row("SELECT w.value FROM windows w JOIN chosen x ON x.id=w.capture WHERE w.id=? AND w.source=?",params![current,text(r,"source")],|r|r.get(0))?;
        let w=uasf_json::from_str(&w)?;let capid=text(&w,"capture_id");
        if depth==0 && w["session_id"]!=sid{return err("probe_leaf_session_mismatch");}
        let cap:String=db.query_row("SELECT value FROM captures WHERE id=?",[&capid],|r|r.get(0))?;let cap=uasf_json::from_str(&cap)?;
        let mut end=cap["length"].as_i64().ok_or("capture length")?;let mut cut=Value::Null;
        if !incoming.is_null(){
            end=incoming["end_byte_offset"].as_i64().ok_or("end")?;cut=incoming["end_ordinal_exclusive"].clone();
            let bound:Option<i64>=db.query_row("SELECT ordinal FROM objects WHERE kind='event' AND capture=? AND start=?",params![capid,end],|r|r.get(0)).optional()?;
            let last:i64=db.query_row("SELECT max(ordinal) FROM objects WHERE kind='event' AND capture=?",[&capid],|r|r.get(0))?;
            if bound.or(if cap["length"]==end{Some(last+1)}else{None})!=cut.as_i64(){return err("probe_boundary_unsupported");}
            if child["session_id"]!=w["session_id"] {
                if child["forked_from_id"]!=cap["session"] || child["forked_from_ordinal_exclusive"]!=cut{return err("probe_fork_unsupported");}
            }else if !child["forked_from_id"].is_null(){return err("probe_fork_unsupported");}
        }
        let chain=json!({"window":current,"capture_id":capid,"cut":cut,"end_byte_offset":end,"anchors":w["anchors"]});
        db.execute("INSERT INTO selected VALUES(?,?,?,?,?,?)",params![depth,current,capid,end,raw(&child),raw(&chain)])?;
        depth+=1;incoming=w["history_base"].clone();child=w;
        let Some(next)=incoming["thread_id"].as_str() else{break;};current=next.to_owned();
    }
    db.execute_batch("INSERT INTO active(event,turn_id) SELECT o.id,o.turn_id FROM objects o JOIN selected s ON s.capture=o.capture WHERE o.kind='event' AND o.ty!='session_meta' AND o.end<=s.end ORDER BY s.seq DESC,o.pos,o.id;
      INSERT INTO excluded(event,turn_id) SELECT o.id,o.turn_id FROM objects o JOIN selected s ON s.capture=o.capture WHERE o.kind='event' AND s.seq>0 AND o.start>=s.end ORDER BY s.seq,o.pos,o.id;")?;
    let duplicate:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM (SELECT DISTINCT a.turn_id,o.native FROM active a JOIN objects o ON o.id=a.turn_id WHERE a.turn_id!='') GROUP BY native HAVING count(*)>1)",[],|r|r.get(0))?;
    if duplicate{return err("probe_duplicate_turn_unsupported");}
    envelope_start(out,"complete",vec![])?;
    write!(out,"{{\"projection\":\"codex_selected_history\",\"leaf\":{},\"chain\":",raw(&r["leaf"]))?;
    array(db,"SELECT value FROM selected ORDER BY seq",out)?;
    out.write_all(b",\"turn_ids\":")?;array(db,"SELECT o.native FROM (SELECT turn_id,min(seq) AS first FROM active WHERE turn_id!='' GROUP BY turn_id) a JOIN objects o ON o.id=a.turn_id ORDER BY a.first",out)?;
    out.write_all(b",\"turns\":")?;turns(db,"active",out)?;
    out.write_all(b",\"events\":")?;array(db,"SELECT o.value FROM active a JOIN objects o ON o.id=a.event ORDER BY a.seq",out)?;
    out.write_all(b",\"excluded_turns\":")?;turns(db,"excluded",out)?;
    out.write_all(b",\"excluded_events\":")?;array(db,"SELECT o.value FROM excluded a JOIN objects o ON o.id=a.event ORDER BY a.seq",out)?;
    out.write_all(b",\"edges\":[")?;let mut first=true;
    let mut q=db.prepare("SELECT o.value FROM edge e JOIN selected s ON s.capture=e.capture JOIN objects o ON o.id=e.id ORDER BY e.id")?;let mut rows=q.query([])?;
    while let Some(row)=rows.next()?{
        let mut e=uasf_json::from_str(&row.get::<_,String>(0)?)?;let mut inherited=None;
        if ["history_base","fork"].contains(&text(&e,"kind").as_str()) {
            let w:String=db.query_row("SELECT value FROM windows WHERE capture=?",[text(&e,"capture_id")],|r|r.get(0))?;let w=uasf_json::from_str(&w)?;let base=&w["history_base"];
            if let Some(parent)=base["thread_id"].as_str(){
                let parent_cap:String=db.query_row("SELECT capture FROM windows WHERE id=?",[parent],|r|r.get(0))?;
                inherited=Some(format!("SELECT anchors FROM objects WHERE kind='event' AND capture={} AND end<={} ORDER BY pos,id",quote(&parent_cap),base["end_byte_offset"].as_i64().ok_or("base end")?));
            }
            e["status"]=json!("complete");e["reasons"]=json!([]);e["target_anchors"]=json!([]);
        }else if e["kind"]=="compaction" && !e["target"].is_null(){
            let target=raw(&e["target"]);let mut find=db.prepare("SELECT o.value FROM active a JOIN objects o ON o.id=a.event WHERE o.context=?")?;
            let matches:Vec<String>=find.query_map([target],|r|r.get(0))?.take(2).collect::<std::result::Result<_,_>>()?;
            if matches.len()==1 {let t=uasf_json::from_str(&matches[0])?;e["status"]=json!("complete");e["reasons"]=json!([]);e["target_event_id"]=t["id"].clone();e["target_anchors"]=t["anchors"].clone();e["assessment"]=json!("previous_compaction_reference");e["input_membership"]=json!("unknown");}
        }
        if !first{out.write_all(b",")?;}first=false;
        if let Some(sql)=inherited {object_with_array(db,&e,"target_anchors",&sql,true,out)?;}else{out.write_all(raw(&e).as_bytes())?;}
    }
    let mut q=db.prepare("SELECT o.value,s.child FROM excluded a JOIN objects o ON o.id=a.event JOIN selected s ON s.capture=o.capture ORDER BY a.seq")?;let mut rows=q.query([])?;
    while let Some(row)=rows.next()?{
        let e=uasf_json::from_str(&row.get::<_,String>(0)?)?;let child=uasf_json::from_str(&row.get::<_,String>(1)?)?;
        let aa:Vec<Value>=child["anchors"].as_array().ok_or("child anchors")?.iter().chain(e["anchors"].as_array().ok_or("event anchors")?.iter()).cloned().collect();
        let edge=json!({"id":key("edge",json!([child["id"],e["id"],"revert_excluded"])),"kind":"revert_excluded","event_id":child["id"],"target":e["id"],"anchors":aa,"status":"complete","reasons":[],"assessment":"excluded_from_selected_history","parser_version":child["parser_version"],"model_input":"unknown"});
        if !first{out.write_all(b",")?;}first=false;out.write_all(raw(&edge).as_bytes())?;
    }
    out.write_all(b"],\"comparison\":\"not_compared\",\"model_input\":\"unknown\"}}")?;Ok(())
}
fn object_with_array(db:&Connection,v:&Value,field:&str,sql:&str,flatten:bool,out:&mut impl Write)->Result<()> {
    out.write_all(b"{")?;
    for (i,(k,value)) in v.as_object().ok_or("object")?.iter().enumerate(){if i>0{out.write_all(b",")?;}write!(out,"{}:",raw(&json!(k)))?;if k==field{if flatten{anchors(db,sql,out)?;}else{array(db,sql,out)?;}}else{out.write_all(raw(value).as_bytes())?;}}
    out.write_all(b"}")?;Ok(())
}
fn turns(db:&Connection,table:&str,out:&mut impl Write)->Result<()> {
    out.write_all(b"[")?;let mut first=true;
    let sql=format!("SELECT o.value FROM (SELECT turn_id,min(seq) AS first FROM {table} WHERE turn_id!='' GROUP BY turn_id) a JOIN objects o ON o.id=a.turn_id ORDER BY a.first");
    let mut q=db.prepare(&sql)?;let mut rows=q.query([])?;
    while let Some(row)=rows.next()? {
        // One turn may span the complete input. Its stored membership is read by
        // SQL; discard those two giant arrays without decoding them in Rust.
        let original:String=row.get(0)?;
        let base:String=db.query_row("SELECT json_remove(?,'$.event_ids','$.anchors')",[&original],|r|r.get(0))?;
        let mut t=uasf_json::from_str(&base)?;let id=text(&t,"id");
        let partial:bool=db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM member m WHERE m.turn_id=? AND NOT EXISTS(SELECT 1 FROM {table} a WHERE a.event=m.event))"),[&id],|r|r.get(0))?;
        if partial {t["status"]=json!("incomplete");t["reasons"]=json!(["turn_cut_by_selected_boundary"]);}
        if !first{out.write_all(b",")?;}first=false;
        let serialized=raw(&t);out.write_all(serialized[..serialized.len()-1].as_bytes())?;
        out.write_all(b",\"event_ids\":")?;
        array(db,&format!("SELECT json_quote(a.event) FROM {table} a JOIN member m ON m.event=a.event AND m.turn_id={} ORDER BY a.seq",quote(&id)),out)?;
        out.write_all(b",\"anchors\":")?;
        anchors(db,&format!("SELECT o.anchors FROM {table} a JOIN member m ON m.event=a.event AND m.turn_id={} JOIN objects o ON o.id=a.event ORDER BY a.seq",quote(&id)),out)?;
        out.write_all(b"}")?;
    }
    out.write_all(b"]")?;Ok(())
}
