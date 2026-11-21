use std::{collections::BTreeMap, path::PathBuf, io::Read};
use uasf_json::{json, Value};
use uasf_gate::{
    files, transport, Gate, ProcessBridge, Error, Result, REQUEST_LIMIT,
};
fn run(admission: &mut Option<uasf_core::admission::Lease>) -> Result<Option<Value>> {
    let accepted=uasf_queue::now();
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or(Error)?;
    let allowed =
        match command.as_str() {
            "query" => vec!["socket"],
            "serve" =>
                vec!["bridge","registry","sources","staging","source","socket","socket-mode"],
            "check-local" =>
                vec!["bridge","registry","sources","staging","source"],
            _ => return Err(Error),
        };
    let mut options = BTreeMap::new();
    while let Some(flag) = args.next() {
        let name = flag.strip_prefix("--").ok_or(Error)?;
        if !allowed.contains(&name) || options.contains_key(name) {
            return Err(Error);
        }
        options.insert(name.to_owned(), args.next().ok_or(Error)?);
    }
    let get = |k: &str| options.get(k).cloned().ok_or(Error);
    if command == "query" {
        let mut bytes = Vec::new();
        std::io::stdin().take(REQUEST_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > REQUEST_LIMIT { return Err(Error); }
        return Ok(Some(transport::query(&PathBuf::from(get("socket")?),
                            &uasf_json::from_slice(&bytes)?)?));
    }
    uasf_core::admission::limit_gate_address_space().map_err(|_| Error)?;
    let sources_path = PathBuf::from(get("sources")?);
    files::private(&sources_path, false)?;
    let staging = PathBuf::from(get("staging")?);
    files::absolute(&staging)?;
    files::private(&staging, true)?;
    if staging.ancestors().any(|p|
                p.join(".git").is_file() || p.join(".git/HEAD").exists()) {
        return Err(Error);
    }
    let gate =
        Gate {
            bridge: ProcessBridge {
                binary: PathBuf::from(get("bridge")?),
                registry: PathBuf::from(get("registry")?),
            },
            registry: PathBuf::from(get("registry")?),
            staging,
            sources: files::json(&sources_path)?,
            source: get("source")?,
        };
    if let Some(binding)=uasf_queue::config::binding(&gate.staging).map_err(|_|Error)? {
        if binding.route.kind!="gate" || ["bridge","registry","sources","staging","source"].iter().any(|k|binding.route.arg(&format!("--{k}"))!=options.get(*k).map(String::as_str)) {return Err(Error);}
        if !uasf_queue::config::worker(&binding).map_err(|_|Error)? {
            if command=="serve" {
                gate.config()?;
                let mode=match options.get("socket-mode").map(String::as_str).unwrap_or("0600"){"0600"=>0o600,"0660"=>0o660,_=>return Err(Error)};
                transport::serve_scheduled(&binding,&PathBuf::from(get("socket")?),mode)?;return Ok(None);
            }
            let result=uasf_queue::sys::read_input(0,accepted+uasf_queue::DEADLINE_NS).and_then(|bytes|uasf_queue::client::forward(&binding,&bytes,accepted,1,true,None));
            match result {Ok(code)=>std::process::exit(code),Err(e)=>{if e.to_string()!="query_delivery_failed"{println!("{}",String::from_utf8(uasf_queue::refusal("gate",&uasf_queue::reason(&e))).unwrap());}std::process::exit(2);}}
        }
    }
    if command == "serve" {
        let mode =
            match options.get("socket-mode").map(String::as_str).unwrap_or("0600")
                {
                "0600" => 0o600,
                "0660" => 0o660,
                _ => return Err(Error),
            };
        transport::serve(&gate, &PathBuf::from(get("socket")?), mode)?;
        Ok(None)
    } else {
        let mut bytes = Vec::new();
        std::io::stdin().take(REQUEST_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > REQUEST_LIMIT { return Err(Error); }
        *admission = match uasf_core::admission::acquire_gate(&gate.staging) {
            Ok(lease) => Some(lease),
            Err(uasf_core::admission::Refusal::Capacity) => return Ok(Some(uasf_gate::rejected("unsupported", "query_capacity_exceeded"))),
            Err(_) => return Ok(Some(uasf_gate::rejected("unsupported", "query_admission_unavailable"))),
        };
        Ok(Some(gate.handle_admitted(&uasf_json::from_slice(&bytes)?, admission.as_ref().unwrap())))
    }
}
fn main() {
    let mut admission = None;
    let result =
        match run(&mut admission) {
            Ok(None) => return,
            Ok(Some(v)) => v,
            Err(_) =>
                json!({"status":"unsupported","reason":"request or gate unavailable"}),
        };
    println!("{result}");
    if result["status"] != "verified" { std::process::exit(2); }
}
