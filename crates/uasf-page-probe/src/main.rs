use std::{path::Path,io::Write,time::{Instant,SystemTime,UNIX_EPOCH}};
use uasf_json::json;
fn main(){if let Err(e)=run(){eprintln!("page probe error: {e}");std::process::exit(3);}}
fn run()->uasf_page_probe::Result<()> {
    let a:Vec<String>=std::env::args().collect();if a.get(1).map(String::as_str)!=Some("--synthetic-only"){return Err("explicit synthetic-only mode required".into());}
    let start=Instant::now();let detail=match a.get(2).map(String::as_str){
        Some("prepare") if a.len()==7=>{let r=uasf_json::from_slice_unique(&std::fs::read(&a[5])?)?;let v=uasf_page_probe::prepare(Path::new(&a[3]),Path::new(&a[4]),&r,Path::new(&a[6]))?;println!("{v}");json!({})},
        Some("page") if a.len()==5||a.len()==6=>{let bytes=std::fs::read(&a[4])?;if bytes.len()>1048576{return Err("page_request_invalid".into());}let r=uasf_json::from_slice_unique(&bytes)?;let now=if a.len()==6{a[5].parse()?}else{SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64};uasf_page_probe::page(Path::new(&a[3]),&r,now,&mut std::io::stdout().lock())?},
        _=>return Err("usage: --synthetic-only prepare PROBE PERSISTENT REQUEST DESTINATION | page PREPARED REQUEST [TEST_CLOCK]".into())
    };
    let status=std::fs::read_to_string("/proc/self/status")?;let rss=status.lines().find(|x|x.starts_with("VmHWM:")).and_then(|x|x.split_whitespace().nth(1)).unwrap_or("0").parse::<u64>()?;
    eprintln!("{}",json!({"seconds":start.elapsed().as_secs_f64(),"peak_rss_kib":rss,"detail":detail}));std::io::stdout().flush()?;Ok(())
}
