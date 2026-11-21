//! A private, anonymous disk spool keeps record bodies and repeated anchors out
//! of the projection plan. Only schema-owned fields can contain spool references.
use std::{cell::RefCell, fs::File, io::{BufReader, BufWriter, Read, Seek, Write}, path::Path};
use crate::{model::{Error, Result}, store::Core};
use uasf_json::{json, Value};
const LIMIT: u64 = crate::limits::OUTPUT as u64;
struct Capped<'a, W> { inner: &'a mut W, remaining: u64, exceeded: &'a std::cell::Cell<bool> }
impl<W: Write> Write for Capped<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining { self.exceeded.set(true); return Err(std::io::Error::other("query_output_limit")); }
        let n = self.inner.write(bytes)?; self.remaining -= n as u64; Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
}
// The same projection renderer drives both counting and delivery. A count
// uses validated spool slice lengths, never reads or decodes a saved body.
trait ProjectionWriter: Write {
    fn reference(&mut self, spool: &Spool, start: u64, len: u64) -> Result<()>;
}
#[derive(Default)]
struct Count { bytes: u64, exceeded: bool }
impl Count {
    fn add(&mut self, bytes: u64) -> std::io::Result<()> {
        let next = self.bytes.checked_add(bytes).filter(|n| *n <= LIMIT);
        if self.exceeded || next.is_none() {
            self.exceeded = true;
            return Err(std::io::Error::other("query_output_limit"));
        }
        self.bytes = next.unwrap();
        Ok(())
    }
    fn finish(self, result: Result<()>) -> Result<u64> {
        // Serialization may wrap the writer error. Preserve the public refusal
        // reason for both saved references and ordinary JSON metadata/Values.
        if self.exceeded { return Err(Error::Invalid("query_output_limit")); }
        result?; Ok(self.bytes)
    }
}
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.add(bytes.len() as u64)?; Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl ProjectionWriter for Count {
    fn reference(&mut self, _: &Spool, _: u64, len: u64) -> Result<()> {
        self.add(len)?; Ok(())
    }
}
struct Delivery<'a, W>(&'a mut W);
impl<W: Write> Write for Delivery<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> { self.0.write(bytes) }
    fn flush(&mut self) -> std::io::Result<()> { self.0.flush() }
}
impl<W: Write> ProjectionWriter for Delivery<'_, W> {
    fn reference(&mut self, spool: &Spool, start: u64, len: u64) -> Result<()> {
        spool.copy_bytes(start, len, self.0)
    }
}
pub(crate) struct Spool { file: RefCell<BufWriter<File>>, offset: std::cell::Cell<u64>, reader: RefCell<Option<BufReader<File>>>, read_offset: std::cell::Cell<u64> }
fn anonymous(directory: &Path) -> Result<File> {
    use std::os::{unix::ffi::OsStrExt, fd::FromRawFd};
    let path = std::ffi::CString::new(directory.as_os_str().as_bytes()).map_err(|_| Error::Invalid("spool_directory"))?;
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_TMPFILE | libc::O_RDWR | libc::O_CLOEXEC, 0o600) };
    if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
    Ok(unsafe { File::from_raw_fd(fd) })
}
impl Spool {
    pub fn new(directory: &Path) -> Result<Self> {
        Ok(Self { file: RefCell::new(BufWriter::with_capacity(65536, anonymous(directory)?)), offset: 0.into(), reader: RefCell::new(None), read_offset: 0.into() })
    }
    fn save(&self, value: &Value) -> Result<Value> {
        // At most one field is serialized in memory, never the response.
        let bytes = uasf_json::to_vec(value)?;
        let start = self.offset.get(); let end = start + bytes.len() as u64;
        if end > LIMIT { return Err(Error::Invalid("query_output_limit")); }
        self.file.borrow_mut().write_all(&bytes)?; self.offset.set(end);
        Ok(json!([start, bytes.len()]))
    }
    pub fn compact(&self, mut value: Value) -> Result<Value> {
        if let Some(body) = value.get_mut("body") { *body = self.save(body)?; }
        if let Some(anchors) = value.get_mut("anchors").and_then(Value::as_array_mut) {
            for anchor in anchors {
                let slice = self.save(anchor)?;
                // These fields alone are used by the graph projection. References
                // never enter source bodies or user-supplied target objects.
                *anchor = json!({"start":anchor["start"],"end":anchor["end"],"slice":slice});
            }
        }
        Ok(value)
    }
    fn copy(&self, reference: &Value, out: &mut impl ProjectionWriter) -> Result<()> {
        let start = reference[0].as_u64().ok_or(Error::Invalid("spool_reference"))?;
        let len = reference[1].as_u64().ok_or(Error::Invalid("spool_reference"))?;
        if start.checked_add(len).is_none_or(|end| end > self.offset.get()) { return Err(Error::Invalid("spool_reference")); }
        out.reference(self, start, len)
    }
    fn copy_bytes(&self, start: u64, len: u64, out: &mut impl Write) -> Result<()> {
        let mut reader = self.reader.borrow_mut();
        if reader.is_none() {
            let mut file = self.file.borrow_mut(); file.flush()?; file.get_mut().rewind()?;
            *reader = Some(BufReader::with_capacity(65536, file.get_ref().try_clone()?));
        }
        let reader = reader.as_mut().unwrap();
        reader.seek_relative(start as i64 - self.read_offset.get() as i64)?;
        let copied = std::io::copy(&mut reader.take(len), out)?;
        self.read_offset.set(start + copied);
        if copied != len { return Err(Error::Invalid("spool_truncated")); }
        Ok(())
    }
    fn anchors(&self, value: &Value, out: &mut impl ProjectionWriter) -> Result<()> {
        out.write_all(b"[")?;
        for (i, anchor) in value.as_array().ok_or(Error::Invalid("anchors"))?.iter().enumerate() {
            if i != 0 { out.write_all(b",")?; }
            self.copy(&anchor["slice"], out)?;
        }
        out.write_all(b"]")?; Ok(())
    }
    fn row(&self, value: &Value, out: &mut impl ProjectionWriter) -> Result<()> {
        let Some(object) = value.as_object() else { return raw(value, out); };
        out.write_all(b"{")?;
        for (i, (key, value)) in object.iter().enumerate() {
            if i != 0 { out.write_all(b",")?; } uasf_json::to_writer(&mut *out, key)?; out.write_all(b":")?;
            match key.as_str() {
                "body" => self.copy(value, out)?,
                "anchors" | "target_anchors" => self.anchors(value, out)?,
                _ => raw(value, out)?,
            }
        }
        out.write_all(b"}")?; Ok(())
    }
    fn measure(&self, value: &Value) -> Result<u64> {
        let mut count = Count::default(); let result = self.render(value, &mut count); count.finish(result)
    }
    pub fn write(&self, value: &Value, out: &mut impl Write) -> Result<()> {
        self.render(value, &mut Delivery(out))
    }
    fn render(&self, value: &Value, out: &mut impl ProjectionWriter) -> Result<()> {
        out.write_all(b"{")?;
        for (i, (key, value)) in value.as_object().ok_or(Error::Invalid("envelope"))?.iter().enumerate() {
            if i != 0 { out.write_all(b",")?; } uasf_json::to_writer(&mut *out, key)?; out.write_all(b":")?;
            if key != "data" { raw(value, out)?; continue; }
            out.write_all(b"{")?;
            for (j, (key, value)) in value.as_object().ok_or(Error::Invalid("query_data"))?.iter().enumerate() {
                if j != 0 { out.write_all(b",")?; } uasf_json::to_writer(&mut *out, key)?; out.write_all(b":")?;
                if ["events","excluded_events","turns","excluded_turns","edges","chain","unreconstructed_event_refs"].contains(&key.as_str()) {
                    out.write_all(b"[")?;
                    for (k, row) in value.as_array().ok_or(Error::Invalid("query_array"))?.iter().enumerate() {
                        if k != 0 { out.write_all(b",")?; } self.row(row, out)?;
                    }
                    out.write_all(b"]")?;
                } else { raw(value, out)?; }
            }
            out.write_all(b"}")?;
        }
        out.write_all(b"}")?; Ok(())
    }
}
fn raw(value: &Value, out: &mut impl Write) -> Result<()> { uasf_json::to_writer(out, value)?; Ok(()) }
impl Core {
    fn write_projection(&self, spool: &Spool, plan: &Value, out: &mut impl Write) -> Result<()> {
        self.scope_output_size(spool.measure(plan)?)?;
        spool.write(plan, out)
    }
    /// Serialize the entire query before touching the destination. Database and
    /// serialization failures leave it untouched. A destination I/O failure can
    /// interrupt delivery; callers must not append a second JSON response.
    /// Anonymous temporary files are removed even on process termination.
    pub fn query_to_writer(&self, request: &Value, out: &mut impl Write) -> Result<String> {
        let _deletion = match self.deletion_lease() { Ok(v)=>v, Err(_)=> {
            let bytes=uasf_json::to_vec(&crate::deletion_binding::pending())?;
            out.write_all(&bytes).map_err(Error::Delivery)?;return Ok("incomplete".into());
        }};
        { let mut metrics = self.scope_metrics.borrow_mut(); metrics.output_estimate_bytes = None; metrics.serialized_output_bytes = None; }
        let spool = Spool::new(&self.spool_directory)?;
        let mut complete = BufWriter::with_capacity(65536, anonymous(&self.spool_directory)?);
        self.db.execute_batch("BEGIN")?;
        let exceeded = std::cell::Cell::new(false);
        let result = (|| {
            // Pin both attached databases before reading policy or index rows.
            self.db.query_row("SELECT (SELECT value FROM metadata WHERE id='parser_version'), (SELECT epoch FROM durable.query_generation WHERE id=1)", [], |_| Ok(()))?;
            let mut capped = Capped { inner: &mut complete, remaining: LIMIT, exceeded: &exceeded };
            let status;
            if ["history","events","turn","lineage","status"].contains(&request["op"].as_str().unwrap_or("")) {
                let plan = crate::query::query_with_spool(self, request, Some(&spool))?;
                status = plan["status"].as_str().unwrap_or("unsupported").to_owned();
                self.write_projection(&spool, &plan, &mut capped)?;
            } else { let value = crate::query::query(self, request)?; status = value["status"].as_str().unwrap_or("unsupported").to_owned(); self.scope_output_size(value_size(&value)?)?; raw(&value, &mut capped)?; }
            complete.flush()?;
            let actual = complete.get_mut().stream_position()?;
            self.scope_metrics.borrow_mut().serialized_output_bytes = Some(actual);
            if self.scope_metrics.borrow().output_estimate_bytes != Some(actual) { return Err(Error::Invalid("output_size_mismatch")); }
            if actual > LIMIT { return Err(Error::Invalid("query_output_limit")); }
            complete.get_mut().rewind()?;
            Ok(status)
        })();
        self.scope_reset();
        let rollback = self.db.execute_batch("ROLLBACK");
        if exceeded.get() { rollback?; return Err(Error::Invalid("query_output_limit")); }
        let status = result?; rollback?;
        std::io::copy(complete.get_mut(), out).map_err(Error::Delivery)?;
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_count_preserves_escaping_and_repeated_anchor_multiplicity() {
        let spool = Spool::new(&std::env::temp_dir()).unwrap();
        let body = uasf_json::from_str(r#"{"text":"é\n\"","n":-0.0}"#).unwrap();
        let original = json!({"body":body,"anchors":[{"start":0,"end":9,"sha256":"synthetic"}],"id":"row"});
        let row = spool.compact(original.clone()).unwrap();
        let edges: Vec<_> = (0..300).map(|_| json!({"anchors":row["anchors"],"target_anchors":row["anchors"],"kind":"preservedSegment"})).collect();
        let expected_edges: Vec<_> = (0..300).map(|_| json!({"anchors":original["anchors"],"target_anchors":original["anchors"],"kind":"preservedSegment"})).collect();
        let plan = json!({"status":"unknown","data":{"events":[row],"edges":edges,"observed_chain":["row"]}});
        let expected = uasf_json::to_vec(&json!({"status":"unknown","data":{"events":[original],"edges":expected_edges,"observed_chain":["row"]}})).unwrap();
        let count = spool.measure(&plan).unwrap();
        assert!(spool.reader.borrow().is_none());
        assert_eq!(spool.read_offset.get(), 0);
        assert_eq!(count, expected.len() as u64);
        let mut output = Vec::new(); spool.write(&plan, &mut output).unwrap();
        assert_eq!(output, expected);
    }
    #[test]
    fn projection_limit_counts_repeated_slices_before_reading_them() {
        let spool = Spool::new(&std::env::temp_dir()).unwrap();
        // Virtual saved slices let the count test cover the exact cap without
        // allocating a cap-sized field. Any attempt to read would fail.
        spool.offset.set(LIMIT);
        let mut plan = json!({"data":{"events":[{"body":[0,0]}]}});
        let overhead = spool.measure(&plan).unwrap();
        plan["data"]["events"][0]["body"][1] = json!(LIMIT-overhead);
        assert_eq!(spool.measure(&plan).unwrap(), LIMIT);
        plan["data"]["events"][0]["body"][1] = json!(LIMIT-overhead+1);
        assert!(matches!(spool.measure(&plan), Err(Error::Invalid("query_output_limit"))));
        // Fill the count exactly at the first body. Its closing delimiter must
        // stop the walk; the malformed second row must never be examined.
        let prefix = br#"{"data":{"events":[{"body":"#.len() as u64;
        plan["data"]["events"][0]["body"][1] = json!(LIMIT-prefix);
        plan["data"]["events"].as_array_mut().unwrap().push(json!({"body":null}));
        assert!(matches!(spool.measure(&plan), Err(Error::Invalid("query_output_limit"))));
        assert!(spool.reader.borrow().is_none());
        let root = std::env::temp_dir().join(format!("output-boundary-{}",std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let core = Core::open(root.join("index.sqlite"),root.join("persistent.sqlite")).unwrap();
        let mut destination = b"untouched".to_vec();
        assert!(matches!(core.write_projection(&spool, &plan, &mut destination), Err(Error::Invalid("query_output_limit"))));
        assert_eq!(destination, b"untouched");
        assert_eq!(core.scope_metrics().output_estimate_bytes, None);
        assert_eq!(core.scope_metrics().serialized_output_bytes, None);
        // Crossing the cap within a saved slice follows the same refusal path.
        plan["data"]["events"][0]["body"][1] = json!(LIMIT-prefix+1);
        assert!(matches!(spool.measure(&plan), Err(Error::Invalid("query_output_limit"))));
        assert!(spool.reader.borrow().is_none());
        core.scope_output_size(LIMIT).unwrap();
        assert!(matches!(core.scope_output_size(LIMIT+1),Err(Error::Invalid("query_output_limit"))));
        assert_eq!(core.scope_metrics().output_estimate_bytes,Some(LIMIT+1));
        drop(core); std::fs::remove_dir_all(root).unwrap();
        plan["data"]["events"][0]["body"] = json!([LIMIT,1]);
        assert!(matches!(spool.measure(&plan), Err(Error::Invalid("spool_reference"))));
    }
    #[test]
    fn count_stops_at_first_byte_over_limit_and_handles_overflow() {
        let mut count = Count::default();
        count.add(LIMIT).unwrap();
        assert!(count.add(1).is_err());
        assert_eq!(count.bytes, LIMIT);
        assert!(count.add(0).is_err());
        assert!(matches!(count.finish(Ok(())), Err(Error::Invalid("query_output_limit"))));
        let mut count = Count::default(); count.add(1).unwrap();
        assert!(count.add(u64::MAX).is_err());
        assert_eq!(count.bytes, 1);
        assert!(matches!(count.finish(Ok(())), Err(Error::Invalid("query_output_limit"))));
    }
    #[test]
    fn value_count_preserves_limit_reason_through_json_error_wrapping() {
        let mut count = Count { bytes: LIMIT-4, exceeded: false };
        let result = raw(&Value::Null, &mut count);
        assert_eq!(count.finish(result).unwrap(), LIMIT);
        let mut count = Count { bytes: LIMIT-3, exceeded: false };
        let result = raw(&Value::Null, &mut count);
        assert!(result.is_err());
        assert_eq!(count.bytes, LIMIT-3);
        assert!(matches!(count.finish(result), Err(Error::Invalid("query_output_limit"))));
        // Other renderer errors must not be relabeled as output refusals.
        assert!(matches!(Count::default().finish(Err(Error::Invalid("spool_reference"))), Err(Error::Invalid("spool_reference"))));
    }
    #[test]
    fn response_spool_limit_is_recorded_before_an_oversized_write() {
        let mut bytes = Vec::new(); let exceeded = std::cell::Cell::new(false);
        let mut limited = Capped { inner: &mut bytes, remaining: 4, exceeded: &exceeded };
        limited.write_all(b"null").unwrap();
        assert!(limited.write_all(b" ").is_err());
        assert!(exceeded.get()); assert_eq!(bytes, b"null");
    }
}

// The compatibility Value API uses the same exact JSON scalar/container count.
pub(crate) fn value_size(value: &Value) -> Result<u64> {
    let mut count = Count::default(); let result = raw(value, &mut count); count.finish(result)
}
