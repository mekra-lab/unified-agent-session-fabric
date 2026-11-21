//! Streaming framing for trusted derived JSON. Every atomic value still passes
//! through the shared decoder; this scanner never interprets numeric tokens.
use std::io::{BufRead, BufReader, Read};
use crate::Result;
pub struct Stream<R: Read> { input: BufReader<R> }
impl<R: Read> Stream<R> {
    pub fn new(input: R) -> Self { Self { input: BufReader::new(input) } }
    fn byte(&mut self) -> Result<u8> { let mut b=[0]; self.input.read_exact(&mut b)?; Ok(b[0]) }
    pub fn peek(&mut self) -> Result<u8> {
        loop { let b=*self.input.fill_buf()?.first().ok_or("unexpected EOF")?;
            if b.is_ascii_whitespace() { self.input.consume(1); } else { return Ok(b); } }
    }
    pub fn eat(&mut self, b:u8) -> Result<bool> { if self.peek()?==b {self.byte()?;Ok(true)}else{Ok(false)} }
    pub fn expect(&mut self,b:u8)->Result<()> { if self.eat(b)?{Ok(())}else{Err("unexpected delimiter".into())} }
    pub fn raw(&mut self)->Result<String> {
        let first=self.peek()?;let mut bytes=vec![self.byte()?];
        if matches!(first,b'{'|b'['|b'"') {
            let mut depth=usize::from(first!=b'"');let mut string=first==b'"';let mut escaped=false;
            loop {let b=self.byte()?;bytes.push(b);
                if string {if escaped {escaped=false;}else if b==b'\\'{escaped=true;}else if b==b'"'{string=false;}}
                else {match b {b'"'=>string=true,b'{'|b'['=>depth+=1,b'}'|b']'=>depth=depth.checked_sub(1).ok_or("depth")?,_=>{}}}
                if !string && depth==0 {break;}
                if bytes.len()>64*1024*1024 {return Err("probe_atomic_input_limit".into());}
            }
        } else {
            loop {let buf=self.input.fill_buf()?;let Some(&b)=buf.first() else{break;};
                if matches!(b,b','|b']'|b'}')||b.is_ascii_whitespace(){break;}bytes.push(self.byte()?);}
        }
        let s=String::from_utf8(bytes)?;uasf_json::from_str(&s)?;Ok(s)
    }
    pub fn key(&mut self)->Result<String>{let s=uasf_json::from_str(&self.raw()?)?.as_str().ok_or("key")?.to_owned();self.expect(b':')?;Ok(s)}
}
