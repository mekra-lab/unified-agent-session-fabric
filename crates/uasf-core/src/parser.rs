use std::collections::BTreeMap;use uasf_json::{json, Value};
use crate::{model::*, provider::ByteResult};
pub const VERSION: &str = "m1.2";
// Compatible inputs retain their existing wire version. Only captures containing
// this extension (including rejected shapes) and their sessions use this revision.
pub const PERMISSION_MODE_VERSION: &str = "m1.3";
pub type Bundle = BTreeMap<String, Vec<Value>>;
const CODEX_TYPES: &[&str] =
    &["session_meta", "event_msg", "response_item", "world_state",
                "turn_context", "token_usage_record", "compacted"];
const CLAUDE_TYPES: &[&str] =
    &["queue-operation", "user", "assistant", "system", "attachment",
                "file-history-snapshot", "atis-latch", "api-request-shape",
                "api-request-blob", "api-request", "last-prompt",
                "cost-state", "mode", "custom-title", "ai-title", "progress",
                "summary", "permission-mode"];
fn edge(base: &Value, events: &mut [Value], edges: &mut Vec<Value>, i: usize,
    kind: &str, target: Value, slot: &str, reason: &str) {
    let e = &mut events[i];
    let id = key("edge", json!([e["id"], kind, slot, target]));
    edges.push(merge(base.clone(),
            json!({"id":id,"event_id":e["id"],"kind":kind,"target":target,"anchors":e["anchors"],"status":"unknown","reasons":[reason],"slot":slot,"assessment":"observed_reference","model_input":"unknown"})));
    e["links"].as_array_mut().unwrap().push(json!(id));
}
pub fn parse(c: &Capture, engine: &str, result: &ByteResult, version: &str)
    -> Bundle {
    parse_into(c, engine, result, version, None).expect("no fallible sink")
}

/// A sink releases independent Claude records inside the caller's transaction.
pub(crate) fn parse_into(c: &Capture, engine: &str, result: &ByteResult, version: &str,
    mut sink: Option<&mut dyn FnMut(&str, &Value) -> Result<()>>) -> Result<Bundle> {
    let mut base =
        json!({"source":c.source,"session_id":c.session_id(),"capture_id":c.id(),"parser_version":format!("{engine}/{version}"),"verification":result.verification,
                "input_label":if result.verification == "verified"
                {"\u{ac80}\u{c99d}\u{b41c} \u{c785}\u{b825}"
                    // Compatibility wire values; these labels are not verification evidence.
                } else {"\u{bbf8}\u{ac80}\u{c99d} \u{c785}\u{b825}"}});
    let mut observation =
        merge(merge(base.clone(), uasf_json::to_value(c).unwrap()),
            json!({"id":c.id(),"anchors":[c.anchor()],"status":"complete","reasons":[]}));
    let mut bundle: Bundle =
        ["capture", "event", "window", "turn",
                            "edge"].into_iter().map(|k| (k.into(), vec![])).collect();
    if result.verification != "verified" {
        issue(&mut observation,
            if ["deleted",
                            "unsupported"].contains(&result.verification.as_str()) {
                &result.verification
            } else { "unknown" },
            result.reason.as_deref().unwrap_or("unverified_input"));
    }
    let Some(data) =
        &result.data else {
            bundle.get_mut("capture").unwrap().push(observation);
            return Ok(bundle);
        };
    if data.len() as u64 != c.length || sha(data) != c.sha256 ||
            result.sha256.as_deref() != Some(&sha(data)) {
        issue(&mut observation, "unsupported", "provider_capture_mismatch");
        bundle.get_mut("capture").unwrap().push(observation);
        return Ok(bundle);
    }
    if !["codex", "claude", "api"].contains(&engine) {
        issue(&mut observation, "unsupported", "unsupported_engine");
    }
    let mut events = vec![];
    let mut offset = 0;
    for line in data.split_inclusive(|b| *b == b'\n') {
        let anchor =
            c.range_anchor(offset, offset + line.len() as u64, &sha(line));
        let mut e =
            merge(base.clone(),
                json!({"id":anchor_id(&anchor),"anchors":[anchor],"position":offset,"status":"complete","reasons":[],"body":null,"type":"invalid","native_id":null,"ordinal":null,"turn_id":null,"links":[]}));
        offset += line.len() as u64;
        if !line.ends_with(b"\n") {
            issue(&mut e, "incomplete", "unfinished_jsonl");
        } else {
            match uasf_json::from_slice(line) {
                Ok(r) if r.is_object() && r["type"].is_string() => {
                    e["type"] = r["type"].clone();
                    e["native_id"] = r["uuid"].clone();
                    e["ordinal"] = r["ordinal"].clone();
                    e["body"] = r;
                }
                _ => issue(&mut e, "unsupported", "invalid_json_record"),
            }
        }
        for reason in strings(&e["reasons"]) {
            issue(&mut observation, e["status"].as_str().unwrap(), &reason);
        }
        events.push(e);
    }
    if events.is_empty() {
        issue(&mut observation, "incomplete", "empty_capture");
        bundle.get_mut("capture").unwrap().push(observation);
        return Ok(bundle);
    }
    if engine == "claude" && version == VERSION && events.iter().any(|e| e["type"] == "permission-mode") {
        let revision = json!(format!("claude/{PERMISSION_MODE_VERSION}"));
        base["parser_version"] = revision.clone();
        observation["parser_version"] = revision.clone();
        for e in &mut events { e["parser_version"] = revision.clone(); }
    }
    let mut windows = vec![];
    let mut turns = vec![];
    let mut edges = vec![];
    if engine == "codex" {
        let mut previous = Value::Null;
        for e in &events {
            let r = &e["body"];
            if r.is_null() { continue; }
            let n = &r["ordinal"];
            if n.as_u64().is_none() {
                issue(&mut observation, "unsupported", "invalid_ordinal");
            } else if !previous.is_null() &&
                    previous.as_u64().and_then(|p| p.checked_add(1)) !=
                        n.as_u64() {
                issue(&mut observation, "unsupported",
                    "noncontiguous_or_duplicate_ordinal");
            }
            previous = n.clone();
            if !CODEX_TYPES.contains(&r["type"].as_str().unwrap_or("")) {
                issue(&mut observation, "unsupported",
                    "unsupported_codex_record");
            }
            if !r["payload"].is_object() {
                issue(&mut observation, "unsupported", "payload_shape");
            }
        }
        let first = events[0]["body"].clone();
        let meta = &first["payload"];
        if first["type"] != "session_meta" {
            issue(&mut observation, "unsupported", "missing_session_meta");
        }
        if events[1..].iter().any(|e| e["type"] == "session_meta") {
            issue(&mut observation, "unsupported", "multiple_session_meta");
        }
        if meta["history_mode"] != "paginated" {
            issue(&mut observation, "unsupported", "history_mode");
        }
        if meta["session_id"] != c.session {
            issue(&mut observation, "unsupported",
                "session_binding_mismatch");
        }
        if c.window.is_none() {
            issue(&mut observation, "unsupported", "window_id_required");
        }
        let fork = !meta["forked_from_id"].is_null();
        if fork {
            if !nonempty(&meta["forked_from_id"]) {
                issue(&mut observation, "unsupported",
                    "fork_declaration_shape");
            } else {
                edge(&base, &mut events, &mut edges, 0, "fork",
                    meta["forked_from_id"].clone(), "",
                    "fork_inheritance_unverified");
            }
            if meta["forked_from_ordinal_exclusive"].as_u64().is_none() {
                issue(&mut observation, "incomplete",
                    "fork_boundary_missing");
            }
            if meta["history_base"].is_null() {
                issue(&mut observation, "incomplete",
                    "fork_history_base_missing");
            }
        } else if !meta["forked_from_ordinal_exclusive"].is_null() {
            issue(&mut observation, "incomplete", "fork_parent_missing");
        }
        let mut history_base = meta["history_base"].clone();
        if !history_base.is_null() {
            if !history_base.is_object() ||
                        !nonempty(&history_base["thread_id"]) ||
                    ["end_ordinal_exclusive",
                                    "end_byte_offset"].iter().any(|k|
                            history_base[k].as_u64().is_none()) {
                issue(&mut observation, "unsupported", "history_base_shape");
                history_base = Value::Null;
            } else {
                if first["ordinal"] != history_base["end_ordinal_exclusive"] {
                    issue(&mut observation, "unsupported",
                        "child_start_boundary_mismatch");
                }
                edge(&base, &mut events, &mut edges, 0, "history_base",
                    history_base["thread_id"].clone(), "",
                    "unresolved_reference");
                if fork &&
                        meta["forked_from_ordinal_exclusive"] !=
                            history_base["end_ordinal_exclusive"] {
                    issue(&mut observation, "unsupported",
                        "fork_boundary_conflict");
                }
            }
        } else if first["ordinal"] != 0 {
            issue(&mut observation, "incomplete", "root_prefix_missing");
        }
        windows.push(merge(base.clone(),
                json!({"id":key("window",json!([c.id()])),"window_id":c.window,"anchors":[c.anchor()],"history_base":history_base,"forked_from_id":meta["forked_from_id"],"forked_from_ordinal_exclusive":meta["forked_from_ordinal_exclusive"],"status":observation["status"],"reasons":observation["reasons"]})));
        let mut current: Option<usize> = None;
        for i in 0..events.len() {
            let p = events[i]["body"]["payload"].clone();
            if events[i]["type"] == "event_msg" && p["type"] == "task_started"
                {
                if !nonempty(&p["turn_id"]) {
                    issue(&mut observation, "unsupported", "missing_turn_id");
                    continue;
                }
                turns.push(merge(base.clone(),
                        json!({"id":key("turn",json!([events[i]["id"]])),"native_id":p["turn_id"],"event_ids":[],"anchors":[],"status":"incomplete","reasons":["completion_not_observed"]})));
                current = Some(turns.len() - 1);
            }
            if let Some(ti) = current {
                let t = &mut turns[ti];
                t["event_ids"].as_array_mut().unwrap().push(events[i]["id"].clone());
                t["anchors"].as_array_mut().unwrap().extend(arr(&events[i]["anchors"]).to_vec());
                events[i]["turn_id"] = t["id"].clone();
                if events[i]["type"] == "event_msg" &&
                        ["task_complete",
                                    "turn_aborted"].contains(&p["type"].as_str().unwrap_or(""))
                    {
                    if p["turn_id"].is_null() || p["turn_id"] == t["native_id"]
                        {
                        t["status"] = json!("complete");
                        t["reasons"] = json!([]);
                        current = None;
                    } else {
                        issue(&mut observation, "unsupported",
                            "turn_completion_binding_mismatch");
                    }
                }
            }
            if events[i]["type"] == "compacted" {
                events[i]["compaction"] =
                    json!({"inspectability":"mixed","encrypted_result":"opaque","retained_context":"readable_in_original","new_utterance":false,"input_membership":"unknown","context_window_id":p["window_id"]});
                edge(&base, &mut events, &mut edges, i, "compaction",
                    p["previous_window_id"].clone(), "previous_context_window",
                    "exact_input_membership_unverified");
            }
        }
    } else if engine == "claude" {
        for i in 0..events.len() {
            let r = events[i]["body"].clone();
            let kind = events[i]["type"].as_str().unwrap_or("").to_owned();
            if kind == "permission-mode" {
                let valid = r.as_object().is_some_and(|o| o.len() == 3 &&
                    ["type", "permissionMode", "sessionId"].iter().all(|k| o.contains_key(*k))) &&
                    r["permissionMode"].is_string() && r["sessionId"] == c.session;
                if !valid {
                    issue(&mut observation, "unsupported", "unsupported_claude_record");
                }
                // Uninterpreted session metadata, never a turn, reference or root.
                // Even a rejected shape must not create graph/deletion dependencies.
                if let Some(emit) = sink.as_mut() {
                    emit("event", &events[i])?;
                    events[i] = Value::Null;
                }
                continue;
            }
            if !CLAUDE_TYPES.contains(&kind.as_str()) {
                issue(&mut observation, "unsupported",
                    "unsupported_claude_record");
            }
            if !r["sessionId"].is_null() && r["sessionId"] != c.session {
                issue(&mut observation, "unsupported",
                    "session_binding_mismatch");
            }
            if !r["uuid"].is_null() && !r["uuid"].is_string() {
                issue(&mut observation, "unsupported", "invalid_uuid");
            }
            for (field, ek) in
                [("parentUuid", "parent"),
                        ("logicalParentUuid", "logicalParent"),
                        ("requestRef", "requestRef")] {
                if truth(&r[field]) {
                    if r[field].is_string() {
                        edge(&base, &mut events, &mut edges, i, ek,
                            r[field].clone(), "", "unresolved_reference");
                    } else {
                        issue(&mut observation, "unsupported", "reference_shape");
                    }
                }
            }
            if r["subtype"] == "compact_boundary" {
                events[i]["compaction"] =
                    json!({"new_root":r["parentUuid"].is_null(),"input_membership":"unknown","inspectability":"observed_metadata"});
                edge(&base, &mut events, &mut edges, i, "compaction",
                    r["logicalParentUuid"].clone(), "input_boundary",
                    "boundary_is_not_full_input_membership");
                let cm =
                    r.get("compactMetadata").cloned().unwrap_or(json!({}));
                if !cm.is_object() {
                    issue(&mut observation, "unsupported",
                        "compact_metadata_shape");
                }
                for name in ["preservedSegment", "preservedMessages"] {
                    let refs = cm.get(name).cloned().unwrap_or(json!({}));
                    if let Some(refs) = refs.as_object() {
                        for (field, value) in refs {
                            if field.ends_with("Uuid") ||
                                    ["uuids", "allUuids"].contains(&field.as_str()) {
                                let values =
                                    if value.is_array() {
                                        arr(value).to_vec()
                                    } else { vec![value.clone()] };
                                for (n, target) in values.into_iter().enumerate() {
                                    if target.is_string() {
                                        edge(&base, &mut events, &mut edges, i, "preservedSegment",
                                            target, &format!("{name}.{field}.{n}"),
                                            "unresolved_reference");
                                    } else {
                                        issue(&mut observation, "unsupported",
                                            "preserved_reference_shape");
                                    }
                                }
                            }
                        }
                    } else {
                        issue(&mut observation, "unsupported",
                            "preserved_reference_shape");
                    }
                }
            }
            if kind == "api-request" {
                events[i]["request_id"] = r["id"].clone();
                if !nonempty(&r["id"]) {
                    issue(&mut observation, "unsupported", "request_id_shape");
                }
                if !r["shapeHash"].is_string() {
                    issue(&mut observation, "unsupported",
                        "request_shape_hash_shape");
                }
                let tail = r.get("tail").cloned().unwrap_or(json!([]));
                if !tail.is_array() {
                    issue(&mut observation, "unsupported",
                        "request_tail_shape");
                } else {
                    for (n, target) in arr(&tail).iter().enumerate() {
                        if target.is_string() {
                            edge(&base, &mut events, &mut edges, i, "requestBlob",
                                target.clone(), &n.to_string(), "unresolved_reference");
                        } else {
                            issue(&mut observation, "unsupported",
                                "request_tail_shape");
                        }
                    }
                }
                if r["keep"].as_f64() != Some(0.0) && r["keep"] != false {
                    events[i]["reasons"].as_array_mut().unwrap().push(json!("request_keep_unverified"));
                }
                edge(&base, &mut events, &mut edges, i, "requestShape",
                    r["shapeHash"].clone(), "", "unresolved_reference");
            }
            if ["api-request-blob",
                            "api-request-shape"].contains(&kind.as_str()) {
                events[i]["reference_hash"] =
                    r.get("hash").unwrap_or(&r["shapeHash"]).clone();
                if !events[i]["reference_hash"].is_string() {
                    issue(&mut observation, "unsupported",
                        "request_reference_hash_shape");
                }
            }
            if ["user", "assistant"].contains(&kind.as_str()) {
                events[i]["classification"] =
                    json!(if truth(&r["isApiErrorMessage"]) ||
                        r["message"]["model"]=="<synthetic>" {"synthetic_error"}
                        else if truth(&r["isCompactSummary"]) {"compaction_summary"}
                        else if truth(&r["isVisibleInTranscriptOnly"]) ||
                        truth(&r["isMeta"]) {"display_or_meta"} else
                        {"observed_message"});
                let id = key("turn", json!([events[i]["id"]]));
                turns.push(merge(base.clone(),
                        json!({"id":id,"native_id":r["uuid"],"unit":"observed_message","event_ids":[events[i]["id"]],"anchors":events[i]["anchors"],"status":"unknown","reasons":["native_turn_boundary_unverified"]})));
                events[i]["turn_id"] = json!(id);
            }
            if let Some(emit) = sink.as_mut() {
                emit("event", &events[i])?;
                events[i] = Value::Null;
                for value in turns.drain(..) { emit("turn", &value)?; }
                for value in edges.drain(..) { emit("edge", &value)?; }
            }
        }
        if sink.is_some() { events.clear(); }
    } else if engine == "api" {
        for e in &mut events {
            let r = &e["body"];
            if r["type"] != "api-record" ||
                            r["schema"] != "uasf.synthetic-api/1" ||
                        r["conversation"] != c.session || !r["call_id"].is_string()
                {
                issue(&mut observation, "unsupported", "api_record_contract");
                continue;
            }
            let id = key("turn", json!([e["id"]]));
            let complete = r["complete"] == true;
            turns.push(merge(base.clone(),
                    json!({"id":id,"native_id":null,"call_id":r["call_id"],"unit":"submitted_call","event_ids":[e["id"]],"anchors":e["anchors"],"status":if
                            complete {"complete"} else {"incomplete"},"reasons":if
                            complete {vec![]} else
                            {vec!["stream_completion_unobserved"]}})));
            e["turn_id"] = json!(id);
        }
        issue(&mut observation, "unknown",
            "submitted_scope_only_total_conversation_unknown");
    }
    for w in &mut windows {
        w["status"] = observation["status"].clone();
        w["reasons"] = observation["reasons"].clone();
    }
    bundle.insert("capture".into(), vec![observation]);
    bundle.insert("event".into(), events);
    bundle.insert("window".into(), windows);
    bundle.insert("turn".into(), turns);
    bundle.insert("edge".into(), edges);
    Ok(bundle)
}
