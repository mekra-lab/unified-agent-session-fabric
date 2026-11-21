// Native fixed-commit Polylogue store adapter, no daemon and no model.
use polylogue_model::Timestamp;
use polylogue_store::{Archive, ReadPool, Writer, UserRequest, UserWrite};
use serde_json::{Value, json};
use std::{env, fs, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let input: Value = serde_json::from_slice(&fs::read(&args[1])?)?;
    let (archive, _lock) = Archive::open_or_create(Path::new(&args[2]))?;
    let writer = Writer::start(&archive)?;
    let mut ids = Vec::new();
    for row in input.as_array().ok_or("array required")? {
        let request = UserRequest {
            id: row["request_id"].as_str().ok_or("request ID")?.to_owned(),
            machine: "hr1-synthetic-adapter".to_owned(),
            at: Timestamp::from_unix_ms(row["at_ms"].as_i64().ok_or("time")?).ok_or("time range")?,
            write: serde_json::from_value::<UserWrite>(row["write"].clone())?,
        };
        let is_delivery = row["write"]["write"] == "deliver";
        let outcome = writer.user(request)?;
        if is_delivery { ids.push(outcome.id); }
    }
    writer.flush()?;
    let pool = ReadPool::open(&archive)?;
    let reader = pool.reader()?;
    let mut exported = Vec::new();
    for id in ids {
        let (row, text) = reader.delivery(&id)?.ok_or("native readback missing")?;
        exported.push(json!({"delivery":row,"text":text}));
    }
    println!("{}", serde_json::to_string(&exported)?);
    Ok(())
}
