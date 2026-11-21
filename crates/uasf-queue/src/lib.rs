//! Explicitly bound, finite local query admission. No persistent request queue.
pub mod client;
pub mod config;
pub mod guardian;
pub mod model;
pub mod server;
pub mod sys;
pub const DEADLINE_NS: u64 = 8_000_000_000;
pub const REQUEST_LIMIT: usize = 1024 * 1024;
pub const RESPONSE_LIMIT: u64 = 256 * 1024 * 1024;
pub const RSS_LIMIT: u64 = 1024 * 1024 * 1024;
pub fn now() -> u64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t); }
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}
pub fn error(reason: &str) -> std::io::Error { std::io::Error::other(reason.to_owned()) }
pub fn refusal(kind: &str, reason: &str) -> Vec<u8> {
    let v = if kind == "gate" { uasf_json::json!({"status":"unsupported","reason":reason}) }
        else { uasf_json::json!({"status":"unsupported","reasons":[reason]}) };
    uasf_json::to_vec(&v).unwrap()
}

/// Keep transport/configuration failures in the documented reason namespace.
pub fn reason(error:&std::io::Error)->String {let s=error.to_string();if s.starts_with("query_") && s.bytes().all(|c|c.is_ascii_lowercase()||c==b'_'){s}else{"query_service_unavailable".into()}}

/// Private IPC header: body length, exit status, and explicit lease acknowledgement.
/// Refusals without a guardian are terminal and must not require a write after EOF.
pub const RESPONSE_HEADER_SIZE: usize = 13;
pub fn response_header(length:u64,exit:i32,ack:bool)->[u8;RESPONSE_HEADER_SIZE] {
    let mut h=[0;RESPONSE_HEADER_SIZE];h[..8].copy_from_slice(&length.to_be_bytes());
    h[8..12].copy_from_slice(&exit.to_be_bytes());h[12]=u8::from(ack);h
}
pub fn parse_response_header(h:[u8;RESPONSE_HEADER_SIZE])->std::io::Result<(u64,i32,bool)> {
    let length=u64::from_be_bytes(h[..8].try_into().unwrap());
    let exit=i32::from_be_bytes(h[8..12].try_into().unwrap());
    if length==0 || ![0,2].contains(&exit) || h[12]>1 || (h[12]==0 && exit!=2) {
        return Err(error("query_invalid_response"));
    }
    if length>RESPONSE_LIMIT{return Err(error("query_output_limit"));}
    Ok((length,exit,h[12]==1))
}
#[cfg(test)]
mod protocol_tests {
    use super::*;
    #[test] fn worker_error_and_terminal_refusal_have_distinct_ack_obligations() {
        assert_eq!(parse_response_header(response_header(20,2,false)).unwrap(),(20,2,false));
        assert_eq!(parse_response_header(response_header(20,2,true)).unwrap(),(20,2,true));
        assert_eq!(parse_response_header(response_header(20,0,true)).unwrap(),(20,0,true));
    }
    #[test] fn invalid_headers_fail_before_body_delivery() {
        for h in [response_header(0,2,false),response_header(1,125,true),response_header(1,0,false),response_header(RESPONSE_LIMIT+1,2,false)] {assert!(parse_response_header(h).is_err());}
        let mut h=response_header(1,2,false);h[12]=2;assert!(parse_response_header(h).is_err());
    }
}
