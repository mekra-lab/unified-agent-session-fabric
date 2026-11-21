//! Decode untrusted JSON without interpreting object keys as serializer markers.
//! Containers are assembled in one walk. String and numeric token validation
//! stays with serde_json; floats retain finite binary64 and integers stay exact.
use serde::de;
pub use serde_json::{json, Map, Number, Value, Error, Result, to_string, to_string_pretty, to_vec, to_vec_pretty, to_value, to_writer};
fn invalid(message: &str) -> Error { <Error as de::Error>::custom(message) }
pub fn from_slice(bytes: &[u8]) -> Result<Value> {
    from_str(std::str::from_utf8(bytes).map_err(|_| invalid("invalid UTF-8"))?)
}
pub fn from_str(text: &str) -> Result<Value> { decode(text, false, 0) }
/// Reserve a few derived root fields without changing decoding rules.
pub fn from_str_with_spare(text: &str, spare: usize) -> Result<Value> {
    if spare > 8 { return Err(invalid("root spare capacity limit")); }
    decode(text, false, spare)
}
fn decode(text: &str, unique: bool, root_spare: usize) -> Result<Value> {
    let mut p = Parser { text, at: 0, unique, root_spare };
    let value = p.value(0)?;
    p.space();
    if p.at != text.len() { return Err(invalid("trailing JSON input")); }
    Ok(value)
}
struct Parser<'a> { text: &'a str, at: usize, unique: bool, root_spare: usize }
impl Parser<'_> {
    fn space(&mut self) {
        while self.text.as_bytes().get(self.at).is_some_and(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t')) { self.at += 1; }
    }
    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        if self.text.as_bytes().get(self.at) == Some(&byte) { self.at += 1; true } else { false }
    }
    fn string(&mut self) -> Result<String> {
        self.space(); let start = self.at;
        if !self.eat(b'"') { return Err(invalid("expected JSON string")); }
        while let Some(&b) = self.text.as_bytes().get(self.at) {
            self.at += 1;
            match b {
                b'"' => return serde_json::from_str(&self.text[start..self.at]),
                b'\\' => {
                    // Scan only; serde_json validates escapes, controls and surrogate pairs.
                    if self.at == self.text.len() { break; }
                    self.at += 1;
                }
                _ => {}
            }
        }
        Err(invalid("unterminated JSON string"))
    }
    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > 128 { return Err(invalid("JSON nesting limit")); }
        self.space();
        match self.text.as_bytes().get(self.at).copied() {
            Some(b'{') => {
                self.at += 1; let mut entries = Vec::new();
                if !self.eat(b'}') {
                    loop {
                        let key = self.string()?;
                        if !self.eat(b':') { return Err(invalid("expected colon")); }
                        entries.push((key, self.value(depth + 1)?));
                        if self.eat(b'}') { break; }
                        if !self.eat(b',') { return Err(invalid("expected object separator")); }
                    }
                }
                // Query callers reserve access and edge-resolution fields only at
                // the root. Nested source objects retain exact initial capacity.
                let mut object = Map::with_capacity(entries.len() + if depth == 0 { self.root_spare } else { 0 });
                for (key, value) in entries {
                    if self.unique && object.contains_key(&key) { return Err(invalid("duplicate_json_key")); }
                    object.insert(key, value);
                }
                Ok(Value::Object(object))
            }
            Some(b'[') => {
                self.at += 1; let mut values = Vec::new();
                if !self.eat(b']') {
                    loop {
                        values.push(self.value(depth + 1)?);
                        if self.eat(b']') { break; }
                        if !self.eat(b',') { return Err(invalid("expected array separator")); }
                    }
                }
                values.shrink_to_fit(); Ok(Value::Array(values))
            }
            Some(b'"') => self.string().map(Value::String),
            Some(_) => {
                let start = self.at;
                while self.text.as_bytes().get(self.at).is_some_and(|b| !matches!(b, b',' | b']' | b'}' | b' ' | b'\n' | b'\r' | b'\t')) { self.at += 1; }
                let lexeme = &self.text[start..self.at];
                match lexeme {
                    "null" => Ok(Value::Null), "true" => Ok(Value::Bool(true)), "false" => Ok(Value::Bool(false)),
                    _ => {
                        let number: Number = serde_json::from_str(if lexeme == "-0" { "0" } else { lexeme })?;
                        let number = if lexeme.contains(['.', 'e', 'E']) {
                            let float: f64 = lexeme.parse().map_err(|_| invalid("invalid number"))?;
                            Number::from_f64(float).ok_or_else(|| invalid("non-finite number"))?
                        } else { number };
                        Ok(Value::Number(number))
                    }
                }
            }
            None => Err(invalid("missing JSON value")),
        }
    }
}

/// Stable IDs use sorted object keys and the established numeric representation.
pub fn canonical(value: &Value) -> String {
    match value {
        Value::Array(a) =>
            format!("[{}]",
                a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Value::Object(o) => {
            let mut entries: Vec<_> = o.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            format!("{{{}}}",
                entries.into_iter().map(|(k,v)|
                format!("{}:{}", serde_json::to_string(k).unwrap(),
                canonical(v))).collect::<Vec<_>>().join(","))
        }
        Value::Number(n) if n.is_f64() => {
            let text = format!("{:?}", n.as_f64().unwrap());
            if let Some((mantissa, exponent)) = text.split_once('e') {
                let exponent: i32 = exponent.parse().unwrap();
                format!("{mantissa}e{exponent:+03}")
            } else { text }
        }
        _ => value.to_string(),
    }
}

/// Typed conversions inspect the already decoded tree; they never invoke the
/// serializer's reserved-key Value visitor a second time.
pub trait FromValue: Sized {
    fn from_value(value: Value) -> Result<Self>;
}
pub fn from_value<T: FromValue>(value: Value) -> Result<T> { T::from_value(value) }
impl FromValue for Value { fn from_value(v: Value) -> Result<Self> { Ok(v) } }
impl FromValue for String {
    fn from_value(v: Value) -> Result<Self> {
        match v { Value::String(s) => Ok(s), _ => Err(invalid("expected string")) }
    }
}
impl FromValue for bool {
    fn from_value(v: Value) -> Result<Self> { v.as_bool().ok_or_else(|| invalid("expected bool")) }
}
impl FromValue for u64 {
    fn from_value(v: Value) -> Result<Self> { v.as_u64().ok_or_else(|| invalid("expected unsigned integer")) }
}
impl<T: FromValue> FromValue for Option<T> {
    fn from_value(v: Value) -> Result<Self> {
        if v.is_null() { Ok(None) } else { T::from_value(v).map(Some) }
    }
}
impl<T: FromValue> FromValue for Vec<T> {
    fn from_value(v: Value) -> Result<Self> {
        match v { Value::Array(a) => a.into_iter().map(T::from_value).collect(), _ => Err(invalid("expected array")) }
    }
}
impl<T: FromValue> FromValue for std::collections::BTreeMap<String, T> {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Object(o) => o.into_iter().map(|(k,v)| Ok((k,T::from_value(v)?))).collect(),
            _ => Err(invalid("expected object")),
        }
    }
}
/// Reject unknown fields before converting a fixed schema.
pub fn fields(value: &Value, allowed: &[&str]) -> Result<()> {
    if value.as_object().is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str()))) { Ok(()) }
    else { Err(invalid("unexpected fields")) }
}

/// Signed protocols reject duplicate keys instead of accepting a last value.
pub fn from_slice_unique(bytes: &[u8]) -> Result<Value> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("invalid UTF-8"))?;
    decode(text, true, 0)
}
