use std::collections::{BTreeMap, BTreeSet};
use uasf_json::{json, Value};
use crate::{access::Access, graph::cycle_reachable, model::*, store::Core};
type Projection = (String, Vec<String>, Value);
fn envelope(op: &str, status: &str, reasons: Vec<String>, data: Value)
    -> Value {
    let mut result = json!({"schema":"uasf-core.query/1","op":op,"status":status,"reasons":sorted_unique(reasons)});
    result["data"] = data;
    result
}
fn fail(status: &str, reason: &str, data: Value) -> Projection {
    (status.into(), vec![reason.into()], data)
}
fn id(v: &Value) -> String { v["id"].as_str().unwrap_or("").into() }
struct View<'a> {
    core: &'a Core,
    access: Access,
    spool: Option<&'a crate::output::Spool>,
}
impl View<'_> {
    fn load(&self, kind: &str, filter: Option<(&str, &[String])>) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        self.core.visit_objects(kind, filter, |v| {
            let v = self.access.public(v, kind);
            values.push(if let Some(spool) = self.spool { spool.compact(v)? } else { v });
            Ok(())
        })?;
        // Storage visitation uses physical row order. Restore the established
        // object-ID order on compact values, without sorting body JSON in SQL.
        values.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(values)
    }
    fn objects_in(&self, kind: &str, field: &str, ids: &[String]) -> Result<Vec<Value>> { self.load(kind, Some((field, ids))) }
    fn objects(&self, kind: &str) -> Result<Vec<Value>> { self.load(kind, None) }
}
struct Selection {
    session: Value,
    captures: Vec<Value>,
    events: Vec<Value>,
    windows: Vec<Value>,
    turns: Vec<Value>,
    edges: Vec<Value>,
}
fn selection(view: &View, source: &str, session: &str, boundary: &Value,
    events_only: bool) -> Result<std::result::Result<Selection, Projection>> {
    let sid = session_key(source, session);
    let Some(session) =
        view.objects_in("session", "id", std::slice::from_ref(&sid))?.into_iter().find(|s|
                s["id"] ==
                    sid) else {
            return Ok(Err(fail("unknown", "session_not_indexed", json!({}))));
        };
    if !boundary.is_array() || arr(boundary).is_empty() ||
            arr(boundary).iter().any(|c| !c.is_string()) {
        return Ok(Err(fail("incomplete", "explicit_capture_boundary_required",
                        json!({}))));
    }
    let boundary = strings(boundary);
    let distinct: BTreeSet<_> = boundary.iter().collect();
    if distinct.len() != boundary.len() {
        return Ok(Err(fail("unsupported", "duplicate_boundary_capture",
                        json!({}))));
    }
    let caps: BTreeMap<_, _> =
        view.objects_in("capture", "id", &boundary)?.into_iter().map(|c| (id(&c), c)).collect();
    if boundary.iter().any(|id|
                !caps.contains_key(id) || caps[id]["source"] != source) {
        return Ok(Err(fail("unsupported",
                        "capture_boundary_source_or_id_mismatch", json!({}))));
    }
    if !boundary.iter().any(|id| caps[id]["session_id"] == sid) {
        return Ok(Err(fail("incomplete",
                        "session_capture_missing_at_boundary", json!({}))));
    }
    let order: BTreeMap<_, _> =
        boundary.iter().enumerate().map(|(i, c)| (c.clone(), i)).collect();
    let get =
        |kind: &str| -> Result<Vec<Value>>
            {
                view.objects_in(kind, "capture_id", &boundary)
            };
    let mut events = get("event")?;
    events.sort_by_key(|e|
            (order[e["capture_id"].as_str().unwrap()],
                e["position"].as_u64().unwrap_or(0)));
    let engine = session["engine"].as_str().unwrap_or("");
    let windows =
        if !events_only && engine == "codex" {
            get("window")?
        } else { vec![] };
    let turns =
        if !events_only && engine != "claude" {
            get("turn")?
        } else { vec![] };
    let edges =
        if !events_only && ["codex", "claude"].contains(&engine) {
            get("edge")?
        } else { vec![] };
    Ok(Ok(Selection {
                session,
                captures: boundary.iter().map(|id|
                            caps[id].clone()).collect(),
                events,
                windows,
                turns,
                edges,
            }))
}
fn selected_turns(events: &[&Value], turns: &BTreeMap<String, &Value>)
    -> Vec<Value> {
    let event_ids: BTreeSet<_> = events.iter().map(|e| id(e)).collect();
    let mut order = vec![];
    let mut seen = BTreeSet::new();
    for e in events {
        if let Some(tid) = e["turn_id"].as_str() {
            if seen.insert(tid.to_owned()) { order.push(tid.to_owned()); }
        }
    }
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut membership = BTreeMap::new();
    for tid in &order {
        let members: BTreeSet<_> =
            strings(&turns[tid]["event_ids"]).into_iter().collect();
        for eid in &members {
            owners.entry(eid.clone()).or_default().push(tid.clone());
        }
        membership.insert(tid.clone(), members);
    }
    let mut by_turn: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for e in events {
        for tid in owners.get(&id(e)).into_iter().flatten() {
            by_turn.entry(tid.clone()).or_default().push(e);
        }
    }
    order.into_iter().map(|tid|
                {
                    let mut t = Value::Object(turns[&tid].as_object().unwrap().iter()
                        .filter(|(k, _)| !["event_ids", "anchors"].contains(&k.as_str()))
                        .map(|(k,v)| (k.clone(),v.clone())).collect());
                    let selected =
                        by_turn.get(&tid).cloned().unwrap_or_default();
                    t["event_ids"] =
                        Value::Array(selected.iter().map(|e|e["id"].clone()).collect());
                    t["anchors"] =
                        Value::Array(selected.iter().flat_map(|e|arr(&e["anchors"]).iter().cloned()).collect());
                    if !membership[&tid].is_subset(&event_ids) {
                        t["status"] = json!("incomplete");
                        t["reasons"] = json!(["turn_cut_by_selected_boundary"]);
                    }
                    t
                }).collect()
}
fn codex(mut s: Selection, leaf: &Value) -> Projection {
    let mut windows: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for w in &s.windows {
        windows.entry(w["window_id"].as_str().unwrap_or("").into()).or_default().push(w);
    }
    let mut out =
        json!({"projection":"codex_selected_history","leaf":leaf,"chain":[],"turn_ids":null,"turns":[],"events":[],"excluded_turns":[],"excluded_events":[],"edges":[],"comparison":"not_compared","model_input":"unknown"});
    let bad: Vec<_> =
        s.captures.iter().filter(|c| c["status"] != "complete").collect();
    if !bad.is_empty() {
        let state = worst(bad.iter().map(|c| c["status"].as_str().unwrap()));
        out["edges"] = json!(s.edges);
        if state == "deleted" {
            out["unreconstructed_event_refs"] =
                json!(s.events.iter().map(|e|json!({"id":e["id"],"anchors":e["anchors"],"status":e["status"],"parser_version":e["parser_version"]})).collect::<Vec<_>>());
        }
        return (state.into(),
                bad.iter().flat_map(|c| strings(&c["reasons"])).collect(),
                out);
    }
    if !nonempty(leaf) {
        return fail("incomplete", "explicit_leaf_required", out);
    }
    let leaf = leaf.as_str().unwrap();
    if windows.get(leaf).map(Vec::len) != Some(1) {
        return fail("incomplete", "leaf_missing_or_ambiguous", out);
    }
    if windows[leaf][0]["session_id"] != s.session["id"] {
        return fail("unsupported", "leaf_session_mismatch", out);
    }
    let caps: BTreeMap<_, _> =
        s.captures.iter().map(|c| (id(c), c)).collect();
    let mut by_capture: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for e in &s.events {
        by_capture.entry(e["capture_id"].as_str().unwrap().into()).or_default().push(e);
    }
    let mut seen = BTreeSet::new();
    let mut parts = vec![];
    let mut exclusions = vec![];
    let mut structural = vec![];
    let mut current = Some(leaf.to_owned());
    let mut incoming = Value::Null;
    let mut child = Value::Null;
    while let Some(cur) = current {
        if !seen.insert(cur.clone()) {
            return fail("unsupported", "history_base_cycle", out);
        }
        let candidates = windows.get(&cur).cloned().unwrap_or_default();
        if candidates.is_empty() {
            return fail("incomplete", "parent_window_missing", out);
        }
        if candidates.len() != 1 {
            return fail("unsupported", "ambiguous_window_capture", out);
        }
        let w = candidates[0];
        let cap = caps[w["capture_id"].as_str().unwrap()];
        let events = &by_capture[w["capture_id"].as_str().unwrap()];
        let mut end = cap["length"].as_u64().unwrap();
        let mut cut = Value::Null;
        if !incoming.is_null() {
            end = incoming["end_byte_offset"].as_u64().unwrap();
            cut = incoming["end_ordinal_exclusive"].clone();
            let n = cut.as_u64().unwrap();
            let first = events.first().unwrap()["ordinal"].as_u64().unwrap();
            let last = events.last().unwrap()["ordinal"].as_u64().unwrap();
            if n < first || n > last.saturating_add(1) {
                return fail("incomplete", "end_ordinal_out_of_range", out);
            }
            if end > cap["length"].as_u64().unwrap() {
                return fail("incomplete", "end_byte_out_of_range", out);
            }
            let mut bounds: BTreeMap<_, _> =
                events.iter().map(|e|
                            (e["anchors"][0]["start"].as_u64().unwrap(),
                                e["ordinal"].clone())).collect();
            bounds.insert(cap["length"].as_u64().unwrap(),
                json!(last.saturating_add(1)));
            if !bounds.contains_key(&end) {
                return fail("unsupported", "end_byte_not_line_boundary", out);
            }
            if bounds[&end] != cut {
                return fail("unsupported", "ordinal_byte_boundary_mismatch",
                        out);
            }
            if child["session_id"] != w["session_id"] {
                if child["forked_from_id"] != cap["session"] ||
                        child["forked_from_ordinal_exclusive"] != cut {
                    return fail("unsupported",
                            "cross_session_parent_without_matching_fork", out);
                }
            } else if truth(&child["forked_from_id"]) {
                return fail("unsupported", "fork_session_binding_mismatch",
                        out);
            }
            let tail: Vec<_> =
                events.iter().filter(|e|
                                e["anchors"][0]["start"].as_u64().unwrap() >=
                                    end).cloned().collect();
            for e in &tail {
                let anchors: Vec<_> =
                    arr(&child["anchors"]).iter().chain(arr(&e["anchors"])).cloned().collect();
                structural.push(json!({"id":key("edge",json!([child["id"],e["id"],"revert_excluded"])),"kind":"revert_excluded","event_id":child["id"],"target":e["id"],"anchors":anchors,"status":"complete","reasons":[],"assessment":"excluded_from_selected_history","parser_version":child["parser_version"],"model_input":"unknown"}));
            }
            exclusions.extend(tail);
        }
        out["chain"].as_array_mut().unwrap().push(json!({"window":cur,"capture_id":cap["id"],"cut":cut,"end_byte_offset":end,"anchors":w["anchors"]}));
        parts.push(events.iter().filter(|e|
                            e["anchors"][0]["end"].as_u64().unwrap() <= end &&
                                e["type"] != "session_meta").cloned().collect::<Vec<_>>());
        child = w.clone();
        incoming = w["history_base"].clone();
        current = incoming["thread_id"].as_str().map(str::to_owned);
    }
    let active: Vec<_> = parts.into_iter().rev().flatten().collect();
    let turns: BTreeMap<_, _> =
        s.turns.iter().map(|t| (id(t), t)).collect();
    let active_turns = selected_turns(&active, &turns);
    let native_ids: Vec<_> =
        active_turns.iter().map(|t| t["native_id"].clone()).collect();
    let distinct: BTreeSet<_> =
        native_ids.iter().map(Value::to_string).collect();
    if distinct.len() != native_ids.len() {
        return fail("unsupported", "duplicate_active_turn_id", out);
    }
    let selected_caps: BTreeSet<_> =
        arr(&out["chain"]).iter().map(|c|
                    c["capture_id"].as_str().unwrap().to_owned()).collect();
    let window_by_capture: BTreeMap<_, _> =
        s.windows.iter().map(|w|
                    (w["capture_id"].as_str().unwrap(), w)).collect();
    let mut compactions: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for e in &active {
        if let Some(context) = e["compaction"]["context_window_id"].as_str() {
            compactions.entry(context.into()).or_default().push(*e);
        }
    }
    let mut edges = vec![];
    for e in &s.edges {
        if !selected_caps.contains(e["capture_id"].as_str().unwrap()) {
            continue;
        }
        let mut e = e.clone();
        if ["history_base", "fork"].contains(&e["kind"].as_str().unwrap()) {
            let owner = window_by_capture[e["capture_id"].as_str().unwrap()];
            let base = &owner["history_base"];
            let inherited =
                if let Some(parent) = base["thread_id"].as_str() {
                    let parent = windows[parent][0];
                    by_capture[parent["capture_id"].as_str().unwrap()].iter().filter(|r|
                                    r["anchors"][0]["end"].as_u64().unwrap() <=
                                        base["end_byte_offset"].as_u64().unwrap()).flat_map(|r|
                                arr(&r["anchors"]).to_vec()).collect::<Vec<_>>()
                } else { vec![] };
            e["status"] = json!("complete");
            e["reasons"] = json!([]);
            e["target_anchors"] = json!(inherited);
        } else if e["kind"] == "compaction" {
            let target = &e["target"];
            let targets =
                if let Some(t) = target.as_str() {
                    compactions.get(t).cloned().unwrap_or_default()
                } else if target.is_null() {
                    vec![]
                } else {
                    active.iter().copied().filter(|r|
                                r["compaction"]["context_window_id"] == *target).collect()
                };
            if targets.len() == 1 && !target.is_null() {
                e["target_event_id"] = targets[0]["id"].clone();
                e["target_anchors"] = targets[0]["anchors"].clone();
                e["assessment"] = json!("previous_compaction_reference");
                e["input_membership"] = json!("unknown");
            }
        }
        edges.push(e);
    }
    edges.extend(structural);
    out["edges"] = Value::Array(edges);
    out["turn_ids"] = Value::Array(native_ids);
    out["turns"] = Value::Array(active_turns);
    out["excluded_turns"] = Value::Array(selected_turns(&exclusions,&turns));
    let indices: BTreeMap<_,_> = s.events.iter().enumerate().map(|(i,e)| (e["id"].as_str().unwrap(),i)).collect();
    let active_order: Vec<_> = active.iter().map(|e| indices[e["id"].as_str().unwrap()]).collect();
    let excluded_order: Vec<_> = exclusions.iter().map(|e| indices[e["id"].as_str().unwrap()]).collect();
    out["events"] = Value::Array(active_order.into_iter().map(|i| std::mem::take(&mut s.events[i])).collect());
    out["excluded_events"] = Value::Array(excluded_order.into_iter().map(|i| std::mem::take(&mut s.events[i])).collect());
    ("complete".into(), vec![], out)
}
fn claude(s: Selection, leaf: &Value) -> Projection {
    let caps: Vec<_> =
        s.captures.iter().filter(|c|
                    c["session_id"] == s.session["id"]).collect();
    if caps.len() != 1 {
        return fail("incomplete", "claude_capture_selection_ambiguous",
                json!({}));
    }
    let cap = caps[0];
    let events: Vec<_> =
        s.events.into_iter().filter(|e|
                        e["capture_id"] == cap["id"]).collect();
    if ["unsupported",
                    "incomplete"].contains(&cap["status"].as_str().unwrap()) {
        return (cap["status"].as_str().unwrap().into(),
                strings(&cap["reasons"]),
                json!({"projection":"claude_observed_graph","events":events,"edges":[],"observed_chain":[],"comparison":"not_compared","active_context":"unknown","model_input":"unknown","exposed":"unknown"}));
    }
    let mut indices: BTreeMap<(&str, String), Vec<&Value>> = BTreeMap::new();
    for e in &events {
        for (field, kind) in
            [("native_id", "uuid"), ("request_id", "request"),
                    ("reference_hash",
                        if e["type"] == "api-request-blob" {
                            "blob"
                        } else { "shape" })] {
            if truth(&e[field]) {
                indices.entry((kind,
                                e[field].to_string())).or_default().push(e);
            }
        }
    }
    let mut status =
        if cap["status"] == "complete" {
            "unknown".into()
        } else { cap["status"].as_str().unwrap().to_owned() };
    let mut reasons = strings(&cap["reasons"]);
    reasons.extend(["native_authority_not_compared".into(),
                "model_input_unknown".into()]);
    if indices.values().any(|v| v.len() > 1) {
        status = worst([status.as_str(), "unsupported"]).into();
        reasons.push("duplicate_native_reference".into());
    }
    let mut edges = vec![];
    for mut e in s.edges {
        if e["capture_id"] != cap["id"] { continue; }
        let kind =
            match e["kind"].as_str().unwrap() {
                "requestRef" => "request",
                "requestBlob" => "blob",
                "requestShape" => "shape",
                _ => "uuid",
            };
        let targets =
            indices.get(&(kind,
                                e["target"].to_string())).cloned().unwrap_or_default();
        if targets.len() == 1 {
            let target = targets[0];
            e["target_event_id"] = target["id"].clone();
            e["target_anchors"] = target["anchors"].clone();
            e["status"] =
                json!(if
                    target["status"]=="deleted"{"deleted"}else{"complete"});
            e["reasons"] = json!([]);
            if e["kind"] == "compaction" {
                e["assessment"] = json!("boundary_reference_only");
                e["input_membership"] = json!("unknown");
            }
        } else {
            e["status"] = json!("unknown");
            e["reasons"] = json!(["unresolved_or_ambiguous_reference"]);
            reasons.push(format!("unresolved_{}",e["kind"].as_str().unwrap()));
            if e["kind"] == "parent" {
                status = worst([status.as_str(), "incomplete"]).into();
            }
        }
        edges.push(e);
    }
    let parent: BTreeMap<_, _> =
        edges.iter().filter(|e|
                        e["kind"] ==
                            "parent").filter_map(|e|
                    e["target_event_id"].as_str().map(|target|
                            (e["event_id"].as_str().unwrap().to_owned(),
                                target.to_owned()))).collect();
    if !cycle_reachable(&parent, events.iter().map(id)).is_empty() {
        status = worst([status.as_str(), "unsupported"]).into();
        reasons.push("parent_cycle".into());
    }
    let mut chain = vec![];
    if !leaf.is_null() {
        let matches =
            indices.get(&("uuid",
                                leaf.to_string())).cloned().unwrap_or_default();
        if matches.len() != 1 {
            status = worst([status.as_str(), "incomplete"]).into();
            reasons.push("leaf_missing_or_ambiguous".into());
        } else {
            let mut current = Some(id(matches[0]));
            let mut seen = BTreeSet::new();
            while let Some(n) = current {
                if !seen.insert(n.clone()) { break; }
                chain.push(n.clone());
                current = parent.get(&n).cloned();
            }
        }
    } else { reasons.push("observed_graph_without_leaf_selection".into()); }
    let mut out = json!({"projection":"claude_observed_graph","leaf":leaf,"uuidless_event_ids":events.iter().filter(|e|!truth(&e["native_id"])).map(id).collect::<Vec<_>>(),"observed_chain":chain,"comparison":"not_compared","active_context":"unknown","model_input":"unknown","exposed":"unknown","outside_chain_is_excluded":false});
    out["events"] = Value::Array(events);
    out["edges"] = Value::Array(edges);
    (status, reasons, out)
}
fn history(view: &View, r: &Value) -> Result<Projection> {
    let s =
        match selection(view, r["source"].as_str().unwrap(),
                    r["session"].as_str().unwrap(), &r["boundary"], false)? {
            Ok(s) => s,
            Err(p) => return Ok(p),
        };
    Ok(match s.session["engine"].as_str().unwrap() {
            "codex" => codex(s, &r["leaf"]),
            "claude" => claude(s, &r["leaf"]),
            "api" => {
                let sid = &s.session["id"];
                let caps: Vec<_> =
                    s.captures.iter().filter(|c|
                                c["session_id"] == *sid).collect();
                let state =
                    worst(caps.iter().map(|c|
                                    c["status"].as_str().unwrap()).chain(["unknown"]));
                (state.into(),
                    caps.iter().flat_map(|c| strings(&c["reasons"])).collect(),
                    json!({"projection":"submitted_api_observations","total_conversation":"unknown","events":s.events.iter().filter(|e|e["session_id"]==*sid).collect::<Vec<_>>(),"turns":s.turns.iter().filter(|t|t["session_id"]==*sid).collect::<Vec<_>>(),"edges":[]}))
            }
            _ => fail("unsupported", "unsupported_engine", json!({})),
        })
}
fn suffix(s: &str, n: usize) -> String {
    s.chars().skip(s.chars().count().saturating_sub(n)).collect()
}
fn sessions(view: &View, source: &Value) -> Result<Vec<Value>> {
    let mut result: Vec<_> =
        view.objects("session")?.into_iter().filter(|s|
                    source.is_null() || s["source"] == *source).collect();
    let names: BTreeMap<_, _> =
        view.core.durable("names")?.into_iter().map(|n|
                    (id(&n), n["display_name"].clone())).collect();
    let mut titles: BTreeMap<String, BTreeMap<String, String>> =
        BTreeMap::new();
    let mut events: Vec<_> =
        view.objects("event")?.into_iter().filter(|e|
                    !e["body"].is_null() &&
                        ["custom-title",
                                    "ai-title"].contains(&e["type"].as_str().unwrap_or(""))).collect();
    events.sort_by_key(|e| e["position"].as_u64());
    for e in events {
        let r = &e["body"];
        if let Some(t) = r.get("customTitle").unwrap_or(&r["title"]).as_str()
            {
            if !t.trim().is_empty() {
                titles.entry(e["session_id"].as_str().unwrap().into()).or_default().insert(e["capture_id"].as_str().unwrap().into(),
                    t.trim().into());
            }
        }
    }
    for s in &mut result {
        let distinct: BTreeSet<_> =
            titles.get(&id(s)).into_iter().flat_map(|m|
                            m.values()).cloned().collect();
        let observed =
            if distinct.len() == 1 {
                distinct.iter().next().cloned()
            } else { None };
        let name = if s["status"] == "deleted" { Value::Null } else { names.get(&id(s)).cloned().unwrap_or(Value::Null) };
        s["display_name"] = name.clone();
        let engine = s["engine"].as_str().unwrap();
        let mut chars = engine.chars();
        let capital =
            chars.next().map(|c|
                            c.to_uppercase().collect::<String>()).unwrap_or_default() +
                &chars.as_str().to_lowercase();
        s["display_label"] =
            json!(name.as_str().filter(|s|!s.is_empty()).map(str::to_owned).or(observed).unwrap_or_else(||format!("{capital} {}",suffix(s["session"].as_str().unwrap(),8))));
        if distinct.len() > 1 {
            s["reasons"].as_array_mut().unwrap().push(json!("title_order_between_captures_unknown"));
        }
        s["host"] = Value::Null;
    }
    let original = result.clone();
    let mut counts = BTreeMap::new();
    for s in &result {
        *counts.entry(s["display_label"].as_str().unwrap().to_owned()).or_insert(0)
            += 1;
    }
    for s in &mut result {
        let label = s["display_label"].as_str().unwrap();
        if counts[label] > 1 {
            let session = s["session"].as_str().unwrap();
            let peers: Vec<_> =
                original.iter().filter(|p| p["id"] != s["id"]).collect();
            let mut length = 8;
            while length < session.chars().count() &&
                    peers.iter().any(|p|
                            suffix(p["session"].as_str().unwrap(), length) ==
                                suffix(session, length)) {
                length += 4;
            }
            let mut new = format!("{label} · {}",suffix(session,length));
            if peers.iter().any(|p| p["session"] == session) {
                new += &format!(" · {}",s["source"].as_str().unwrap());
            }
            s["display_label"] = json!(new);
        }
    }
    Ok(result)
}
fn access_in(value: &Value, state: &str) -> bool {
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        match v {
            Value::Object(o) => {
                if o.get("body_access") == Some(&json!(state)) {
                    return true;
                }
                stack.extend(o.iter().filter(|(k, _)|
                                k.as_str() != "body").map(|(_, v)| v));
            }
            Value::Array(a) => stack.extend(a),
            _ => {}
        }
    }
    false
}
pub(crate) fn query(core: &Core, r: &Value) -> Result<Value> { query_with_spool(core, r, None) }
pub(crate) fn query_with_spool(core: &Core, r: &Value, spool: Option<&crate::output::Spool>) -> Result<Value> {
    if !r.is_object() {
        return Ok(envelope("invalid", "unsupported",
                    vec!["query_object_required".into()], json!({})));
    }
    let op = r["op"].as_str().unwrap_or("invalid");
    let allowed: &[&str] =
        match op {
            "sources" => &["op"],
            "sessions" => &["op", "source"],
            "history" | "status" =>
                &["op", "source", "session", "leaf", "boundary"],
            "lineage" =>
                &["op", "source", "session", "leaf", "boundary", "target",
                            "kind"],
            "turn" =>
                &["op", "source", "session", "turn_id", "leaf", "boundary"],
            "evidence" => &["op", "source", "session"],
            "events" => &["op", "source", "session", "boundary"],
            _ => &[],
        };
    if allowed.is_empty() ||
            r.as_object().unwrap().keys().any(|k|
                    !allowed.contains(&k.as_str())) {
        return Ok(envelope(op, "unsupported",
                    vec!["query_fields_or_operation".into()], json!({})));
    }
    for field in ["source", "session", "turn_id", "target", "kind", "leaf"] {
        if let Some(v) = r.get(field) {
            if !(nonempty(v) || (field == "leaf" && v.is_null())) {
                return Ok(envelope(op, "unsupported",
                            vec!["query_field_type".into()], json!({})));
            }
        }
    }
    if !["sources", "sessions"].contains(&op) &&
            ["source", "session"].iter().any(|k| !nonempty(&r[k])) {
        return Ok(envelope(op, "unsupported",
                    vec!["source_and_session_required".into()], json!({})));
    }
    if let Err(error) = core.scope_prepare(r) {
        core.scope_reset();
        return match error {
            Error::Invalid(reason) => Ok(envelope(op, if reason=="index_requires_rebuild" || reason=="query_scope_unavailable" {"incomplete"} else {"unsupported"}, vec![reason.into()],json!({}))),
            other => Err(other),
        };
    }
    let selection =
        r["boundary"].as_array().filter(|a| a.iter().all(Value::is_string));
    let view =
        View {
            core,
            spool,
            access: Access::new(core, selection.map(Vec::as_slice))?,
        };
    let (mut status, mut reasons, mut data) =
        match op {
            "sources" =>
                ("complete".into(), vec![],
                    json!({"sources":core.durable("sources")?})),
            "sessions" =>
                ("complete".into(), vec![],
                    json!({"sessions":sessions(&view,&r["source"])?})),
            "evidence" => {
                let sid =
                    session_key(r["source"].as_str().unwrap(),
                        r["session"].as_str().unwrap());
                let facts: Vec<_> =
                    core.durable("evidence")?.into_iter().filter(|e|
                                    e["session_id"] ==
                                        sid).map(|e| view.access.public(e, "evidence")).collect();
                let absent: Vec<_> =
                    ["received", "exposed",
                                        "used"].into_iter().filter(|f|
                                !facts.iter().any(|e| e["fact"] == *f)).collect();
                ("complete".into(), vec![],
                    json!({"facts":facts,"automatic_promotion":false,"absent_facts":absent}))
            }
            "events" =>
                match selection_fn(&view, r)? {
                    Ok(s) => {
                        let sid = &s.session["id"];
                        let caps: Vec<_> =
                            s.captures.iter().filter(|c|
                                        c["session_id"] == *sid).collect();
                        (worst(caps.iter().map(|c|
                                            c["status"].as_str().unwrap())).into(),
                            caps.iter().flat_map(|c| strings(&c["reasons"])).collect(),
                            json!({"events":s.events.into_iter().filter(|e|e["session_id"]==*sid).collect::<Vec<_>>()}))
                    }
                    Err(p) => p,
                },
            _ => history(&view, r)?,
        };
    if op == "status" {
        let deleted = data.as_object().into_iter().flat_map(|o| o.values()).flat_map(arr)
            .any(|v| v["body_access"] == "deleted");
        if deleted { status = worst([status.as_str(), "deleted"]).into(); }
        let blocked: BTreeSet<_> =
            data.as_object().into_iter().flat_map(|o|
                                o.values()).flat_map(arr).flat_map(|v|
                        strings(&v["body_reasons"])).collect();
        data =
            json!({"projection":data["projection"],"comparison":data.get("comparison").cloned().unwrap_or(json!("not_compared")),"model_input":"unknown","boundary":r["boundary"],"leaf":r["leaf"]});
        if !blocked.is_empty() {
            data["body_access"] = json!("blocked");
            data["body_reasons"] = json!(blocked);
        } else if deleted {
            data["body_access"] = json!("deleted");
        }
    } else if op == "turn" {
        let Some(wanted) =
            r["turn_id"].as_str() else {
                return Ok(envelope(op, "unsupported",
                            vec!["full_turn_reference_required".into()], json!({})));
            };
        let selected: BTreeSet<_> =
            arr(&data["events"]).iter().filter_map(|e|
                        e["turn_id"].as_str().map(str::to_owned)).collect();
        let turns =
            if !arr(&data["turns"]).is_empty() {
                arr(&data["turns"]).to_vec()
            } else {
                view.objects("turn")?.into_iter().filter(|t|
                            t["id"] == wanted && selected.contains(wanted)).collect()
            };
        let mut matched: Vec<_> =
            turns.into_iter().chain(arr(&data["excluded_turns"]).to_vec()).filter(|t|
                        t["id"] == wanted).collect();
        if status == "deleted" {
            matched = view.objects("turn")?.into_iter().filter(|t|
                t["id"] == wanted && t["source"] == r["source"] && arr(&r["boundary"]).contains(&t["capture_id"])).collect();
        }
        if matched.is_empty() {
            status = worst([status.as_str(), "unknown"]).into();
            reasons.push("turn_not_resolved_at_boundary".into());
        }
        data =
            json!({"turns":matched,"selection":if
                    status=="deleted"{"deleted"}else if selected.contains(wanted){"active"}else{"excluded_or_unresolved"}});
    } else if op == "lineage" {
        let mut edges = arr(&data["edges"]).to_vec();
        if let Some(target) = r["target"].as_str() {
            let mut ids = BTreeSet::from([target.to_owned()]);
            for t in
                view.objects("turn")?.into_iter().filter(|t|
                        t["id"] == target) {
                ids.extend(strings(&t["event_ids"]));
            }
            let active: BTreeSet<_> =
                arr(&data["events"]).iter().map(id).collect();
            let includes = !ids.is_disjoint(&active);
            edges.retain(|e|
                    ids.contains(e["event_id"].as_str().unwrap_or("")) ||
                            ids.contains(e["target_event_id"].as_str().unwrap_or("")) ||
                        (e["kind"] == "revert_excluded" && includes));
        }
        if r.get("kind").is_some() {
            edges.retain(|e| e["kind"] == r["kind"]);
        }
        data =
            json!({"edges":edges,"input_membership":"unknown","model_input":"unknown"});
    }
    if access_in(&data, "blocked") {
        status = worst([status.as_str(), "unknown"]).into();
        reasons.push("body_blocked".into());
    }
    if access_in(&data, "deleted") {
        status = worst([status.as_str(), "deleted"]).into();
    }
    Ok(envelope(op, &status, reasons, data))
}
fn selection_fn(view: &View, r: &Value)
    -> Result<std::result::Result<Selection, Projection>> {
    selection(view, r["source"].as_str().unwrap(),
        r["session"].as_str().unwrap(), &r["boundary"], true)
}
