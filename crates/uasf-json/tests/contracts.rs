use uasf_json::{canonical, from_str, from_value, json, Value};
#[test]
fn shared_corpus_preserves_objects_or_rejects_without_partial_values() {
    let cases = from_str(include_str!("inspection.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let v = from_str(c["input"].as_str().unwrap());
        assert_eq!(v.is_ok(), c["decode_ok"].as_bool().unwrap(), "{}", c["name"]);
        for spare in [0, 2, 5, 8] {
            let reserved = uasf_json::from_str_with_spare(c["input"].as_str().unwrap(), spare);
            assert_eq!(reserved.is_ok(), v.is_ok());
            if let (Ok(a), Ok(b)) = (&reserved, &v) { assert_eq!(uasf_json::to_vec(a).unwrap(), uasf_json::to_vec(b).unwrap()); }
        }
        if let Ok(v) = v {
            assert!(v.is_object());
            assert_eq!(from_str(&v.to_string()).unwrap(), v);
            assert_eq!(from_value::<Value>(v.clone()).unwrap(), v);
            assert!(v.to_string().contains("12345"));
        }
    }
}
#[test]
fn numeric_contract_depth_and_marker_keys_are_explicit() {
    for raw in ["1e0", "1.00", "10e-1"] { assert_eq!(canonical(&from_str(raw).unwrap()), "1.0"); }
    assert_ne!(canonical(&from_str("1").unwrap()), canonical(&from_str("1.0").unwrap()));
    assert_ne!(canonical(&from_str("-0.0").unwrap()), canonical(&from_str("0.0").unwrap()));
    assert_eq!(from_str("-0").unwrap(), json!(0));
    assert_eq!(from_str("1267650600228229401496703205376").unwrap().to_string(), "1267650600228229401496703205376");
    for d in [128,129] {
        assert_eq!(from_str(&format!("{}0{}", "[".repeat(d), "]".repeat(d))).is_ok(), d == 128);
    }
    for raw in ["{} trailing", "[01]", "NaN", "1e9999", r#""\uD800""#] { assert!(from_str(raw).is_err()); }
    let v = from_str(r#"{"$serde_json::private::RawValue":"false","$serde_json::private::Number":"not numeric","large":1267650600228229401496703205376}"#).unwrap();
    assert!(v.is_object());
    assert_eq!(from_value::<Value>(v.clone()).unwrap(), v);
    assert_eq!(from_str(r#"{"x":1,"x":2}"#).unwrap(),json!({"x":2}));
}
#[test]
fn signed_protocol_unique_keys_does_not_change_default_contract() {
    let raw=br#"{"a":1,"\u0061":2}"#;
    assert!(uasf_json::from_slice_unique(raw).is_err());
    assert_eq!(uasf_json::from_slice(raw).unwrap(),json!({"a":2}));
    assert!(uasf_json::from_slice_unique(br#"{"$serde_json::private::Number":"123"}"#).unwrap().is_object());
}

#[test]
fn single_walk_rejects_invalid_grammar_without_partial_values() {
    for text in ["", "[", "{", "[1,]", "{\"a\":1,}", "[1 2]", "{\"a\" 1}",
        "true false", "01", "-01", "+1", ".1", "1.", "1e", "1e+", "NaN", "Infinity",
        "\"\\x00\"", "\"\\uD800\"", "\"line\nfeed\"", "[null]junk", "\u{b}null",
        "{\"a\":1e9999,\"a\":0}", "{\"a\":0,\"a\":1e9999}"] {
        assert!(uasf_json::from_str(text).is_err(), "accepted {text:?}");
    }
    for text in [r#" {"a":[true,false,null,-0,1e0],"b":"é\uD83D\uDE00"} "#,
        r#"{"a":1,"a":2,"$serde_json::private::Number":"123"}"#,
        r#"["\\", "\"", "\b\f\n\r\t", 123456789012345678901234567890]"#] {
        let value=uasf_json::from_str(text).unwrap();
        let encoded=uasf_json::to_vec(&value).unwrap();
        assert_eq!(value,uasf_json::from_slice(&encoded).unwrap());
        for end in 0..text.len() {
            if let Some(prefix)=text.get(..end) {
                // A prefix may itself be a complete number/string, but both decoders
                // must agree on JSON grammar (the corpus excludes marker roots).
                let valid=serde_json::from_str::<&serde_json::value::RawValue>(prefix).is_ok();
                assert_eq!(uasf_json::from_str(prefix).is_ok(),valid,"{prefix:?}");
            }
        }
    }
}
