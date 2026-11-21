use std::{collections::BTreeMap, io::Read, path::PathBuf};
use uasf_json::{json, Value};
use uasf_core::{
    model::{Error, Result},
    Binding, Capture, Core, LocalProvider, Source,
};
fn run() -> Result<Option<Value>> {
    let accepted = uasf_queue::now();
    let mut args = std::env::args().skip(1);
    let mut index = None;
    let mut persistent = None;
    let mut command = None;
    let mut input = "-".to_owned();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--index" => index = args.next(),
            "--persistent" => persistent = args.next(),
            "--json" =>
                input = args.next().ok_or(Error::Invalid("json_argument"))?,
            "query" | "rebuild" | "name" | "evidence" | "delete" if
                command.is_none() => command = Some(arg),
            _ => return Err(Error::Invalid("arguments")),
        }
    }
    if command.as_deref() == Some("query") {
        let index_path = PathBuf::from(index.as_ref().ok_or(Error::Invalid("index_required"))?);
        let persistent_path = PathBuf::from(persistent.as_ref().ok_or(Error::Invalid("persistent_required"))?);
        match uasf_queue::client::core(&index_path, &persistent_path, &input, accepted) {
            Ok(Some(code)) => std::process::exit(code),
            Ok(None) => {},
            Err(e) => {
                let reason=uasf_queue::reason(&e);
                if reason != "query_delivery_failed" {println!("{}", String::from_utf8(uasf_queue::refusal("core", &reason)).unwrap());}
                std::process::exit(2);
            }
        }
    }
    if input == "-" {
        input.clear();
        if command.as_deref() == Some("query") {
            std::io::stdin().take(1024 * 1024 + 1).read_to_string(&mut input)?;
            if input.len() > 1024 * 1024 { return Err(Error::Invalid("query_request_limit")); }
        } else { std::io::stdin().read_to_string(&mut input)?; }
    }
    let _lease = if command.as_deref() == Some("query") {
        let persistent_path = PathBuf::from(persistent.as_ref().ok_or(Error::Invalid("persistent_required"))?);
        let root = persistent_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
        std::fs::create_dir_all(root)?;
        match uasf_core::admission::acquire(root) {
            Ok(lease) => { uasf_core::admission::limit_address_space()?; Some(lease) },
            Err(reason) => return Err(Error::Invalid(match reason {
                uasf_core::admission::Refusal::Capacity => "query_capacity_exceeded",
                _ => "query_admission_unavailable",
            })),
        }
    } else { None };
    let value: Value = uasf_json::from_str(&input)?;
    let core =
        Core::open(index.ok_or(Error::Invalid("index_required"))?,
                persistent.ok_or(Error::Invalid("persistent_required"))?)?;
    match command.as_deref() {
        Some("query") => {
            core.query_to_writer(&value, &mut std::io::stdout().lock())?;
            use std::io::Write; std::io::stdout().write_all(b"\n").map_err(Error::Delivery)?;
            Ok(None)
        },
        Some("name") => {
            if value.as_object().is_none_or(|o| o.len() != 3) {
                return Err(Error::Invalid("name_fields"));
            }
            core.set_display_name(value["source"].as_str().ok_or(Error::Invalid("source"))?,
                    value["session"].as_str().ok_or(Error::Invalid("session"))?,
                    value["name"].as_str().ok_or(Error::Invalid("name"))?)?;
            Ok(Some(json!({"status":"complete"})))
        }
        Some("evidence") =>
            Ok(Some(json!({"status":"complete","id":core.add_evidence(&value)?}))),
        Some("rebuild") => {
            for source in
                value["sources"].as_array().ok_or(Error::Invalid("sources"))?
                {
                let source: Source = uasf_json::from_value(source.clone())?;
                core.register_source(&source)?;
            }
            let mut bindings = BTreeMap::new();
            for entry in
                value["captures"].as_array().ok_or(Error::Invalid("captures"))?
                {
                let c: Capture =
                    uasf_json::from_value(entry["capture"].clone())?;
                core.register_capture(&c)?;
                bindings.insert(c.id(),
                    Binding::File(PathBuf::from(entry["fixture_path"].as_str().ok_or(Error::Invalid("fixture_path"))?)));
            }
            core.rebuild(&LocalProvider::new(bindings)).map(Some)
        }
        Some("delete") => core.delete(&value).map(Some),
        _ => Err(Error::Invalid("command")),
    }
}
fn main() {
    match run() {
        Ok(Some(v)) => println!("{v}"),
        Ok(None) => {},
        Err(Error::Delivery(_)) => std::process::exit(2),
        Err(Error::Invalid(reason @ ("query_capacity_exceeded" | "query_admission_unavailable" | "query_spool_limit" | "query_output_limit" | "query_request_limit"))) => {
            println!("{}",json!({"status":"unsupported","reasons":[reason]}));
            std::process::exit(2);
        }
        Err(_) => {
            println!("{}",json!({"status":"unsupported","reasons":["invalid_input_or_unavailable_store"]}));
            std::process::exit(2);
        }
    }
}
