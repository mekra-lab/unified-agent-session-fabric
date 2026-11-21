use std::{io::Write,path::PathBuf,time::Instant};
use uasf_json::json;
fn main(){if let Err(e)=run(){eprintln!("probe error: {e}");std::process::exit(3);}}
fn run()->uasf_index_probe::Result<()> {
    let a:Vec<String>=std::env::args().collect();
    if a.get(1).map(String::as_str)!=Some("--synthetic-only"){return Err("explicit synthetic-only mode required".into());}
    let start=Instant::now();
    match a.get(2).map(String::as_str){
        Some("load") if a.len()==6=>uasf_index_probe::load(&PathBuf::from(&a[3]),&PathBuf::from(&a[4]),&PathBuf::from(&a[5]))?,
        Some("query") if a.len()==5=>{let request=uasf_json::from_slice(&std::fs::read(&a[4])?)?;uasf_index_probe::query(&PathBuf::from(&a[3]),&request,&mut std::io::stdout().lock())?;std::io::stdout().write_all(b"\n")?;},
        _=>return Err("usage: --synthetic-only load INDEX PERSISTENT PROBE | query PROBE REQUEST".into())
    }
    let status=std::fs::read_to_string("/proc/self/status")?;
    let rss=status.lines().find(|s|s.starts_with("VmHWM:")).and_then(|s|s.split_whitespace().nth(1)).unwrap_or("0");
    eprintln!("{}",json!({"seconds":start.elapsed().as_secs_f64(),"peak_rss_kib":rss.parse::<u64>()?}));Ok(())
}
