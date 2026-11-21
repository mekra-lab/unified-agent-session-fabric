//! Explicit synthetic-only paging feasibility experiment. No production reader,
//! scheduler, authorization service, or deletion-completion claim.
mod stream;
mod prepare;
pub use prepare::prepare;
use std::{fs::{self,File},io::{Read,Write},path::Path,time::Instant};
use base64::{Engine,engine::general_purpose::URL_SAFE_NO_PAD as B64};
use hmac::{Hmac,Mac};
use sha2::{Sha256,Digest};
use rusqlite::{Connection,params,StatementStatus};
use uasf_json::{Value,json};
pub type Result<T> = std::result::Result<T,Box<dyn std::error::Error>>;
pub fn raw(v:&Value)->String{uasf_json::to_string(v).unwrap()}
pub fn digest(bytes:&[u8])->String{format!("{:x}",Sha256::digest(bytes))}
#[derive(Default,Debug)]
pub struct Stats{pub vm:u64,pub scans:u64,pub visits:u64,pub decodes:u64,pub decode_bytes:u64,pub summary_reads:u64}
impl Stats {
    fn statement(&mut self,s:&rusqlite::Statement<'_>){self.vm+=s.get_status(StatementStatus::VmStep) as u64;self.scans+=s.get_status(StatementStatus::FullscanStep) as u64;}
    fn decode(&mut self,s:&str)->Result<Value>{self.decodes+=1;self.decode_bytes+=s.len() as u64;Ok(uasf_json::from_str(s)?)}
    pub fn value(&self)->Value{json!({"vm_steps":self.vm,"fullscan_steps":self.scans,"fetched_rows":self.visits,"decode_count":self.decodes,"decode_bytes":self.decode_bytes,"summary_reads":self.summary_reads,"source_reparse":0,"graph_solves":0})}
}
fn random_id()->Result<String>{let mut b=[0;16];File::open("/dev/urandom")?.read_exact(&mut b)?;Ok(B64.encode(b))}
fn sign(key:&[u8],v:&Value)->Result<String>{let bytes=raw(v);let mut h=Hmac::<Sha256>::new_from_slice(key)?;h.update(b"uasf-page/1\0");h.update(bytes.as_bytes());Ok(format!("v1.synthetic.{}.{}",B64.encode(bytes),B64.encode(h.finalize().into_bytes())))}
fn verify(key:&[u8],encoded_cursor:&str)->Result<Value>{
    if encoded_cursor.len()>8192{return Err("page_cursor_invalid".into());}
    let p:Vec<_>=encoded_cursor.split('.').collect();if p.len()!=4||p[0]!="v1"||p[1]!="synthetic"{return Err("page_cursor_invalid".into());}
    let bytes=B64.decode(p[2]).map_err(|_|"page_cursor_invalid")?;let tag=B64.decode(p[3]).map_err(|_|"page_cursor_invalid")?;
    if B64.encode(&bytes)!=p[2]||B64.encode(&tag)!=p[3]{return Err("page_cursor_invalid".into());}
    let mut h=Hmac::<Sha256>::new_from_slice(key)?;h.update(b"uasf-page/1\0");h.update(&bytes);h.verify_slice(&tag).map_err(|_|"page_cursor_invalid")?;
    Ok(uasf_json::from_slice_unique(&bytes).map_err(|_|"page_cursor_invalid")?)
}
fn number(v:&Value)->Result<i64>{v.as_i64().ok_or_else(||"page_request_invalid".into())}
fn read_meta(db:&Connection,stats:&mut Stats)->Result<Value>{let mut q=db.prepare("SELECT value FROM metadata WHERE id=1")?;let s:String=q.query_row([],|r|r.get(0))?;stats.statement(&q);stats.visits+=1;stats.decode(&s)}
fn max_range(db:&Connection,mut start:i64,end:i64,s:&mut Stats)->Result<i64>{
    let mut q=db.prepare("SELECT maximum FROM maxima WHERE level=? AND chunk=?")?;let mut maximum=0;
    while start<end{let mut level=0;while start%(1i64<<(level+1))==0 && start+(1i64<<(level+1))<=end{level+=1;}
        maximum=maximum.max(q.query_row(params![level,start>>level],|r|r.get(0))?);start+=1i64<<level;s.visits+=1;s.summary_reads+=1;}
    s.statement(&q);Ok(maximum)
}
fn lookup(db:&Connection,c:&str,pos:i64,end:bool,s:&mut Stats)->Result<i64>{
    let mut q=db.prepare(if end{"SELECT end FROM elements WHERE collection=? AND pos=?"}else{"SELECT start FROM elements WHERE collection=? AND pos=?"})?;
    let n=q.query_row(params![c,pos],|r|r.get(0))?;s.visits+=1;s.summary_reads+=1;s.statement(&q);Ok(n)
}
fn error(op:&Value,reason:&str)->Value{json!({"schema":"uasf-core.page/1-draft","op":op,"status":"unsupported","reasons":[reason],"data":{},"page":{"complete":false,"range_complete":false,"full_projection_enumerated":false,"history_complete":false,"next_cursor":null,"returned_records":0}})}
/// Generates a complete bounded buffer before writing. `now` is explicit only
/// for deterministic synthetic lifetime tests; it is not a production clock.
pub fn page(path:&Path,request:&Value,now:i64,out:&mut impl Write)->Result<Value>{
    let start=Instant::now();let mut stats=Stats::default();
    let response=match build(path,request,now,&mut stats){Ok(v)=>v,Err(e) if e.to_string().starts_with("page_")=>error(&request["op"],&e.to_string()),Err(e)=>return Err(e)};
    let mut bytes=uasf_json::to_vec(&response)?;bytes.push(b'\n');
    if start.elapsed().as_secs()>=8 {return Err("page_deadline_exceeded before delivery".into());}
    out.write_all(&bytes)?;Ok(json!({"seconds":start.elapsed().as_secs_f64(),"stats":stats.value(),"bytes":bytes.len()}))
}
fn build(path:&Path,request:&Value,now:i64,stats:&mut Stats)->Result<Value>{
    let mut normalized=request.clone();let p=&request["page"];
    let records=p.get("max_records").map(number).transpose()?.unwrap_or(256);let bytes=p.get("max_bytes").map(number).transpose()?.unwrap_or(1048576);
    if !(1..=1024).contains(&records)||!(4096..=4194304).contains(&bytes){return Err("page_limit_invalid".into());}
    let range=request.get("range").cloned().unwrap_or(json!({"kind":"all"}));
    normalized["range"]=range.clone();normalized["page"]=json!({"contract":"uasf-core.page/1-draft","manifest":p["manifest"],"max_records":records,"max_bytes":bytes});
    let binding=digest(raw(&canonical(&normalized)).as_bytes());
    let key=fs::read(path.with_extension("key"))?;if key.len()!=32{return Err("page_key_unavailable".into());}
    let token=if p["cursor"].is_null(){None}else{Some(verify(&key,p["cursor"].as_str().ok_or("page_cursor_invalid")?)?)};
    let db=Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;db.busy_timeout(std::time::Duration::from_millis(500))?;
    db.execute_batch("PRAGMA cache_size=-4096; PRAGMA mmap_size=0; BEGIN;")?;
    let meta=read_meta(&db,stats)?;
    if p["manifest"]!=meta["manifest"]{return Err("page_cursor_snapshot_unavailable".into());}
    let mut original=request.clone();let map=original.as_object_mut().ok_or("page_request_invalid")?;map.remove("page");map.remove("range");
    if original!=meta["request"]{return Err("page_cursor_binding_mismatch".into());}
    db.execute("ATTACH DATABASE ? AS durable",[meta["persistent"].as_str().ok_or("persistent")?])?;
    let mut epochq=db.prepare("SELECT epoch FROM durable.query_generation WHERE id=1")?;let epoch:i64=epochq.query_row([],|r|r.get(0))?;stats.statement(&epochq);stats.visits+=1;
    if epoch!=meta["generation"]{return Err("page_cursor_data_changed".into());}
    let mut q=db.prepare("SELECT name,length FROM collections ORDER BY name")?;let cols:Vec<(String,i64)>=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<std::result::Result<_,_>>()?;stats.statement(&q);stats.visits+=cols.len() as u64;
    let mut arrays=json!({});let mut omitted=vec![];let mut lo=0;let mut hi=number(&meta["total"])?;
    if range["kind"]=="all"{for(c,n)in &cols{arrays[c]=json!({"length":n,"selected_count":n});}}
    else if range["kind"]=="positions"{
        let c=range["collection"].as_str().ok_or("page_range_invalid")?;let n=cols.iter().find(|(k,_)|k==c).ok_or("page_range_invalid")?.1;
        let a=number(&range["start"])?;let b=number(&range["end"])?;if a<0||a>b||b>n{return Err("page_range_invalid".into());}
        if a==b{lo=0;hi=0;}else{lo=lookup(&db,c,a,false,stats)?;hi=lookup(&db,c,b-1,true,stats)?;}
        arrays[c]=json!({"length":n,"selected_count":b-a});for(k,_)in &cols{if k!=c{omitted.push(k);}}
    }else{return Err("page_range_not_prepared".into());}
    let mut cursor=if let Some(t)=token{
        if t["binding"]!=binding||t["manifest"]!=meta["manifest"]||t["epochs"]!=meta["epochs"]{return Err("page_cursor_binding_mismatch".into());}
        if now<number(&t["issued"])?{return Err("page_cursor_clock_untrusted".into());}if now>=number(&t["expires"])?{return Err("page_cursor_expired".into());}t
    }else{json!({"kid":"synthetic","domain":meta["domain"],"manifest":meta["manifest"],"projection":meta["projection_id"],"binding":binding,"epochs":meta["epochs"],"rank":lo,"traversal_id":random_id()?,"issued":now,"expires":now+900})};
    let position=number(&cursor["rank"])?;if position<lo||position>hi{return Err("page_cursor_invalid".into());}
    let mut response=json!({"schema":"uasf-core.page/1-draft","op":request["op"],"status":meta["status"],"reasons":meta["reasons"],"data":{"header":{"fields":meta["fields"],"arrays":arrays,"omitted_collections":omitted},"items":[]},"page":{"complete":true,"range_complete":false,"full_projection_enumerated":false,"history_complete":false,"next_cursor":null,"returned_records":0,"max_records":records,"max_bytes":bytes,"manifest":meta["manifest"],"projection_id":meta["projection_id"],"domain":meta["domain"],"normalized_request_digest":binding,"traversal_id":cursor["traversal_id"],"boundary_digest":digest(raw(&request["boundary"]).as_bytes()),"parser_version":"m1.2","layout_version":"projection-parts/1","leaf":request["leaf"],"range":range,"data_epoch":meta["epochs"]["data_epoch"],"policy_epoch":meta["epochs"]["policy_epoch"],"deletion_epoch":meta["epochs"]["deletion_epoch"],"parser_epoch":meta["epochs"]["parser_epoch"]}});
    // Preflight uses a range-max segment tree, never a scan of selected rows.
    cursor["rank"]=json!(hi);response["page"]["next_cursor"]=json!(sign(&key,&cursor)?);
    let overhead=raw(&response).len() as i64+1+4;let maximum=max_range(&db,lo,hi,stats)?;
    if overhead>bytes{return Err("page_metadata_too_large".into());}if overhead+maximum>bytes{return Err("page_item_too_large".into());}
    let mut q=db.prepare("SELECT p.rank,p.collection,p.pos,p.child,p.sub,p.kind,coalesce(o.value,a.value) FROM parts p JOIN atoms a ON a.hash=p.atom LEFT JOIN objects o ON o.id=a.object_id WHERE p.rank>=? AND p.rank<? ORDER BY p.rank LIMIT ?")?;
    let mut rows=q.query(params![position,hi,records+1])?;let mut items=Vec::new();let mut next=position;let mut item_bytes=0usize;
    while let Some(row)=rows.next()?{
        let rank:i64=row.get(0)?;let c:String=row.get(1)?;let pos:i64=row.get(2)?;let child:String=row.get(3)?;let sub:i64=row.get(4)?;let kind:String=row.get(5)?;let text:String=row.get(6)?;
        stats.visits+=3;let value=stats.decode(&text)?;let path=if child.is_empty(){json!([c,pos])}else{json!([c,pos,child,sub])};
        let v=if kind=="object"{json!({"kind":kind,"path":path,"fields":value["fields"],"arrays":value["arrays"]})}else{json!({"kind":kind,"path":path,"value":value})};
        if items.len()==records as usize{break;}
        let n=raw(&v).len()+usize::from(!items.is_empty());
        response["page"]["returned_records"]=json!(items.len()+1);
        cursor["rank"]=json!(rank+1);
        response["page"]["next_cursor"]=if rank+1==hi {Value::Null}else{json!(sign(&key,&cursor)?)};
        if raw(&response).len()+1+item_bytes+n>bytes as usize{break;}
        items.push(v);item_bytes+=n;next=rank+1;
    }drop(rows);stats.statement(&q);
    if stats.vm>if records<=256{250000}else{1000000}||stats.scans>64||stats.visits>if records<=256{4096}else{16384}||stats.decode_bytes>if records<=256{16*1048576}else{64*1048576}{return Err("page_work_limit".into());}
    response["data"]["items"]=json!(items);response["page"]["returned_records"]=json!(items.len());let terminal=next==hi;
    cursor["rank"]=json!(next);response["page"]["next_cursor"]=if terminal{Value::Null}else{json!(sign(&key,&cursor)?)};
    response["page"]["range_complete"]=json!(terminal);response["page"]["full_projection_enumerated"]=json!(terminal&&range["kind"]=="all");response["page"]["history_complete"]=json!(terminal&&range["kind"]=="all"&&request["op"]=="history"&&meta["status"]=="complete");
    if raw(&response).len()+1>bytes as usize{return Err("page_work_limit".into());}
    Ok(response)
}

fn canonical(v:&Value)->Value {
    match v {Value::Object(o)=>{let mut keys:Vec<_>=o.keys().collect();keys.sort();let mut out=json!({});for k in keys{out[k]=canonical(&o[k]);}out},Value::Array(a)=>Value::Array(a.iter().map(canonical).collect()),_=>v.clone()}
}
