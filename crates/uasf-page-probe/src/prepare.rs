use std::{fs::{self,File},io::{Write,Read},path::Path};
use rusqlite::{Connection,params,OptionalExtension};
use uasf_json::{Value,json};
use crate::{Result,raw,digest,stream::Stream};

fn atom(db:&Connection,s:&str)->Result<String>{
    let hash=digest(s.as_bytes());
    let object:Option<String>=db.query_row("SELECT id FROM hashes WHERE hash=?",[&hash],|r|r.get(0)).optional()?;
    db.execute("INSERT OR IGNORE INTO atoms VALUES(?,?,?)",params![hash,object,if object.is_none(){Some(s)}else{None}])?;Ok(hash)
}
fn item(db:&Connection,c:&str,pos:i64,child:&str,sub:i64,kind:&str,s:&str)->Result<()> {
    let a=atom(db,s)?;
    let path=if child.is_empty(){json!([c,pos])}else{json!([c,pos,child,sub])};
    let v=uasf_json::from_str(s)?;
    let wire=if kind=="object"{json!({"kind":kind,"path":path,"fields":v["fields"],"arrays":v["arrays"]})}else{json!({"kind":kind,"path":path,"value":v})};
    db.execute("INSERT INTO unordered VALUES(?,?,?,?,?,?,?)",params![c,pos,child,sub,kind,a,raw(&wire).len() as i64])?;Ok(())
}
fn collection<R:Read>(db:&Connection,c:&str,input:&mut Stream<R>)->Result<i64>{
    input.expect(b'[')?;let mut pos=0;
    while !input.eat(b']')? {
        if pos>0{input.expect(b',')?;}
        let split:&[&str]=match c{"turns"|"excluded_turns"=>&["anchors","event_ids"],"edges"=>&["anchors","target_anchors"],"chain"=>&["anchors"],_=>&[]};
        if split.is_empty(){item(db,c,pos,"",0,"value",&input.raw()?)?;}
        else{
            input.expect(b'{')?;let mut fields=json!({});let mut arrays=json!({});let mut first=true;
            while !input.eat(b'}')? {
                if !first {input.expect(b',')?;} first=false;let field=input.key()?;
                if split.contains(&field.as_str()) && input.peek()?==b'[' {
                    input.expect(b'[')?;let mut n=0;
                    while !input.eat(b']')?{if n>0{input.expect(b',')?;}item(db,c,pos,&field,n,"value",&input.raw()?)?;n+=1;}
                    arrays[&field]=json!(n);
                }else{fields[&field]=uasf_json::from_str(&input.raw()?)?;}
            }
            item(db,c,pos,"",0,"object",&raw(&json!({"fields":fields,"arrays":arrays})))?;
        }
        pos+=1;
    }Ok(pos)
}
fn ingest(db:&Connection,path:&Path)->Result<Value>{
    let mut s=Stream::new(File::open(path)?);s.expect(b'{')?;let mut meta=json!({});let mut fields=json!({});let mut first=true;
    while !s.eat(b'}')?{
        if !first{s.expect(b',')?;}first=false;let k=s.key()?;
        if k!="data"{meta[&k]=uasf_json::from_str(&s.raw()?)?;continue;}
        s.expect(b'{')?;let mut first=true;
        while !s.eat(b'}')?{
            if !first{s.expect(b',')?;}first=false;let c=s.key()?;
            if s.peek()?==b'['{let n=collection(db,&c,&mut s)?;db.execute("INSERT INTO collections VALUES(?,?)",params![c,n])?;}
            else{fields[&c]=uasf_json::from_str(&s.raw()?)?;}
        }
    }
    meta["fields"]=fields;Ok(meta)
}
fn events(probe:&Path,r:&Value,out:&mut File)->Result<()> {
    let db=Connection::open(probe)?;let sid=uasf_core::model::session_key(r["source"].as_str().ok_or("source")?,r["session"].as_str().ok_or("session")?);
    let mut status="complete".to_owned();let mut reasons=vec![];
    for c in r["boundary"].as_array().ok_or("boundary")? {
        let v:String=db.query_row("SELECT value FROM captures WHERE id=? AND source=?",params![c.as_str(),r["source"].as_str()],|row|row.get(0))?;
        let v=uasf_json::from_str(&v)?;if v["session_id"]==sid{status=uasf_core::model::worst([status.as_str(),v["status"].as_str().unwrap()]).into();reasons.extend(uasf_core::model::strings(&v["reasons"]));}
    }
    write!(out,"{{\"schema\":\"uasf-core.query/1\",\"op\":\"events\",\"status\":{},\"reasons\":{},\"data\":{{\"events\":[",raw(&json!(status)),raw(&json!(uasf_core::model::sorted_unique(reasons))))?;
    let mut first=true;
    for c in r["boundary"].as_array().unwrap(){let mut q=db.prepare("SELECT o.value FROM objects o JOIN captures c ON c.id=o.capture WHERE o.kind='event' AND o.capture=? AND c.session=? ORDER BY o.pos,o.id")?;let mut rows=q.query(params![c.as_str(),sid])?;
        while let Some(row)=rows.next()?{if !first{out.write_all(b",")?;}first=false;out.write_all(row.get_ref(0)?.as_str()?.as_bytes())?;}
    }out.write_all(b"]}}")?;Ok(())
}
/// Offline only. A temporary complete serialization is used to reuse the disk
/// resolver. It is discarded before publication. Stored parts reference source
/// objects, or normalized scalar/relationship metadata, never page bodies.
pub fn prepare(probe:&Path,persistent:&Path,request:&Value,destination:&Path)->Result<Value>{
    if destination.exists(){return Err("page_destination_exists".into());}
    let tmp=destination.with_extension("building");let spool=destination.with_extension("projection");
    struct Clean(std::path::PathBuf,std::path::PathBuf);impl Drop for Clean{fn drop(&mut self){let _=fs::remove_file(&self.0);let _=fs::remove_file(&self.1);}}
    let _clean=Clean(tmp.clone(),spool.clone());
    if tmp.exists()||spool.exists(){return Err("page_stale_prepare_files".into());}
    let mut out=fs::OpenOptions::new().write(true).create_new(true).open(&spool)?;
    let mut history=request.clone();history["op"]=json!("history");
    match request["op"].as_str(){Some("events")=>events(probe,request,&mut out)?,Some("history"|"lineage")=>uasf_index_probe::query(probe,&history,&mut out)?,_=>return Err("page_request_invalid".into())}
    out.sync_all()?;drop(out);let spool_bytes=spool.metadata()?.len();
    fs::copy(probe,&tmp)?;let db=Connection::open(&tmp)?;
    db.execute_batch("PRAGMA cache_size=-16384; PRAGMA temp_store=FILE; PRAGMA mmap_size=0; BEGIN;
      CREATE TABLE hashes(hash TEXT PRIMARY KEY,id TEXT);
      CREATE TABLE atoms(hash TEXT PRIMARY KEY,object_id TEXT,value TEXT);
      CREATE TABLE unordered(collection TEXT,pos INTEGER,child TEXT,sub INTEGER,kind TEXT,atom TEXT,bytes INTEGER);
      CREATE TABLE collections(name TEXT PRIMARY KEY,length INTEGER);
      CREATE TABLE metadata(id INTEGER PRIMARY KEY,value TEXT);")?;
    let mut q=db.prepare("SELECT id,value FROM objects WHERE kind='event'")?;let mut rows=q.query([])?;
    while let Some(row)=rows.next()?{let id:String=row.get(0)?;let v:String=row.get(1)?;db.execute("INSERT OR IGNORE INTO hashes VALUES(?,?)",params![digest(v.as_bytes()),id])?;}
    drop(rows);drop(q);
    let mut meta=ingest(&db,&spool)?;
    if request["op"]=="lineage" {
        if !request["target"].is_null(){return Err("probe_lineage_target_not_prepared".into());}
        if let Some(kind)=request.get("kind") {
            db.execute("DELETE FROM unordered WHERE collection='edges' AND pos IN (SELECT u.pos FROM unordered u JOIN atoms a ON a.hash=u.atom WHERE u.collection='edges' AND u.child='' AND json_extract(a.value,'$.fields.kind')!=?)",[kind.as_str()])?;
            // Positions after filtering are dense, just as the single response.
            db.execute_batch("CREATE TEMP TABLE renumber AS SELECT pos,row_number() OVER(ORDER BY pos)-1 AS n FROM unordered WHERE collection='edges' AND child=''; UPDATE unordered SET pos=(SELECT n FROM renumber WHERE renumber.pos=unordered.pos) WHERE collection='edges';")?;
        }
        db.execute("DELETE FROM unordered WHERE collection!='edges'",[])?;
        db.execute("DELETE FROM collections WHERE name!='edges'",[])?;
        db.execute("UPDATE collections SET length=(SELECT count(*) FROM unordered WHERE child='')",[])?;
        meta["fields"]=json!({"input_membership":"unknown","model_input":"unknown"});meta["op"]=json!("lineage");
    }
    db.execute_batch("CREATE TABLE parts(rank INTEGER PRIMARY KEY,collection TEXT,pos INTEGER,child TEXT,sub INTEGER,kind TEXT,atom TEXT,bytes INTEGER);
      INSERT INTO parts SELECT row_number() OVER(ORDER BY collection,pos,child,sub)-1,* FROM unordered ORDER BY collection,pos,child,sub;
      CREATE INDEX part_position ON parts(collection,pos,rank);
      CREATE TABLE elements(collection TEXT,pos INTEGER,start INTEGER,end INTEGER,maximum INTEGER,PRIMARY KEY(collection,pos));
      INSERT INTO elements SELECT collection,pos,min(rank),max(rank)+1,max(bytes) FROM parts GROUP BY collection,pos;
      CREATE TABLE maxima(level INTEGER,chunk INTEGER,maximum INTEGER,PRIMARY KEY(level,chunk));
      INSERT INTO maxima SELECT 0,rank,bytes FROM parts;")?;
    let total:i64=db.query_row("SELECT count(*) FROM parts",[],|r|r.get(0))?;let mut size=total;let mut level=0;
    while size>1{db.execute("INSERT INTO maxima SELECT ?,chunk/2,max(maximum) FROM maxima WHERE level=? GROUP BY chunk/2",params![level+1,level])?;size=(size+1)/2;level+=1;}
    let durable=Connection::open_with_flags(persistent,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let epoch:i64=durable.query_row("SELECT epoch FROM query_generation WHERE id=1",[],|r|r.get(0))?;
    meta["request"]=request.clone();meta["domain"]=json!("synthetic-only");meta["manifest"]=json!(format!("m:{}",file_digest(probe)?));meta["projection_id"]=json!(digest(raw(request).as_bytes()));
    meta["persistent"]=json!(persistent.canonicalize()?.to_string_lossy());meta["generation"]=json!(epoch);meta["total"]=json!(total);
    meta["epochs"]=json!({"data_epoch":epoch,"policy_epoch":0,"deletion_epoch":0,"parser_epoch":1});
    db.execute("INSERT INTO metadata VALUES(1,?)",[raw(&meta)])?;
    db.execute_batch("DROP TABLE unordered; DROP TABLE hashes; COMMIT;")?;drop(db);
    let mut key=[0u8;32];File::open("/dev/urandom")?.read_exact(&mut key)?;
    let keypath=destination.with_extension("key");use std::os::unix::fs::OpenOptionsExt;
    let mut k=fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(keypath)?;k.write_all(&key)?;k.sync_all()?;
    File::open(&tmp)?.sync_all()?;fs::rename(&tmp,destination)?;File::open(destination.parent().ok_or("parent")?)?.sync_all()?;
    fs::remove_file(&spool)?;
    Ok(json!({"manifest":meta["manifest"],"records":total,"temporary_projection_bytes":spool_bytes,"prepared_bytes":destination.metadata()?.len()}))
}

fn file_digest(path:&Path)->Result<String>{
    use sha2::{Digest,Sha256};let mut h=Sha256::new();let mut f=File::open(path)?;let mut buf=[0;65536];
    loop{let n=f.read(&mut buf)?;if n==0{break;}h.update(&buf[..n]);}Ok(format!("{:x}",h.finalize()))
}
