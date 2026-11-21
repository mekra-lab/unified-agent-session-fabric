//! Explicit erasure authority and reversible dependency withholding.
//! Only caller-supplied tombstones authorize erasure; this policy never writes them.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use uasf_json::{json, Value};
use crate::{graph::cycle_reachable, model::*, store::Core};
pub(crate) struct Access {
    by_anchor: BTreeMap<(String, String, String), String>,
    blocked: BTreeMap<String, BTreeSet<String>>,
    blocked_sessions: BTreeMap<String, BTreeSet<String>>,
    pub sessions: BTreeSet<String>,
    captures: BTreeMap<String, Capture>,
    ranges: BTreeMap<(String, String, String), Vec<(u64, u64)>>,
}
impl Access {
    pub fn new(core: &Core, selection: Option<&[Value]>) -> Result<Self> {
        let captures: BTreeMap<String, Capture> =
            core.durable("captures")?.into_iter().map(|v|
                            {
                                let c: Capture = uasf_json::from_value(v)?;
                                Ok((c.id(), c))
                            }).collect::<Result<_>>()?;
        let provenance: BTreeMap<_, _> =
            core.durable("provenance")?.into_iter().map(|v|
                        (v["id"].as_str().unwrap().to_owned(), v)).collect();
        let engines: BTreeMap<_, _> =
            core.durable("sources")?.into_iter().map(|s|
                        (s["id"].as_str().unwrap().to_owned(),
                            s["engine"].clone())).collect();
        let mut owners: BTreeMap<(String, String), Vec<String>> =
            BTreeMap::new();
        let mut result =
            Self {
                by_anchor: BTreeMap::new(),
                blocked: BTreeMap::new(),
                blocked_sessions: BTreeMap::new(),
                sessions: BTreeSet::new(),
                captures: captures.clone(),
                ranges: BTreeMap::new(),
            };
        for t in core.durable("tombstones")? {
            // Historical inferred closures are not explicit deletion authority.
            if t["kind"] == "session" && t["reason"] == "explicit_session_deletion" {
                result.sessions.insert(t["session_id"].as_str().ok_or(Error::Invalid("tombstone_session"))?.into());
            } else if t["kind"] == "range" {
                let start = t["start"].as_u64().ok_or(Error::Invalid("tombstone_range"))?;
                let end = t["end"].as_u64().ok_or(Error::Invalid("tombstone_range"))?;
                result.ranges.entry(anchor_key(&t)).or_default().push((start, end));
            }
        }
        for (id, c) in &captures {
            result.by_anchor.insert((c.source.clone(), c.generation.clone(),
                    c.capture.clone()), id.clone());
            if let Some(w) = &c.window {
                owners.entry((c.source.clone(),
                                w.clone())).or_default().push(id.clone());
            }
        }
        let mut parents = BTreeMap::new();
        let mut boundary_sets: BTreeMap<String, BTreeSet<String>> =
            BTreeMap::new();
        for (id, c) in &captures {
            let p = provenance.get(id).cloned().unwrap_or(Value::Null);
            let trusted = &p["trusted"];
            if p["observation"] == "deleted" {
                result.block(id, "provider_reports_deleted");
            }
            if engines.get(&c.source) == Some(&json!("codex")) {
                if c.window.is_none() {
                    result.block(id, "window_ownership_missing");
                }
                if p["observation"] != "verified" || !truth(trusted) {
                    result.block(id, "window_dependency_unverified");
                }
            }
            if !result.capture_ranges(id).is_empty() && truth(&trusted["has_compaction"]) {
                result.block(id, "compaction_input_membership_unknown");
            }
            if c.window.is_none() || !truth(trusted) { continue; }
            if trusted["shape_status"] != "complete" {
                result.block(id, "window_dependency_incomplete");
                continue;
            }
            let base = &trusted["history_base"];
            if base.is_null() { continue; }
            let owners = owners.get(&(c.source.clone(), base["thread_id"].as_str().unwrap_or("").into()));
            // Check all durable owners before applying the query boundary.
            if owners.into_iter().flatten().any(|id| result.sessions.contains(&captures[id].session_id())) {
                result.block(id, "deleted_parent_session");
            }
            let candidates: Vec<_> = owners.into_iter().flatten().filter(|id|
                selection.is_none_or(|sel| sel.contains(&json!(id)))).collect();
            if candidates.len() != 1 {
                result.block(id, "parent_ownership_missing_or_ambiguous");
                continue;
            }
            let parent = candidates[0];
            let pm = &provenance[parent]["trusted"];
            if !truth(pm) || pm["shape_status"] != "complete" {
                result.block(id, "parent_boundary_unverified");
                continue;
            }
            let boundaries =
                boundary_sets.entry(parent.clone()).or_insert_with(||
                        arr(&pm["boundaries"]).iter().map(Value::to_string).collect());
            if !boundaries.contains(&json!([base["end_byte_offset"],base["end_ordinal_exclusive"]]).to_string())
                {
                result.block(id, "parent_boundary_conflict");
                continue;
            }
            if c.session_id() != captures[parent].session_id() {
                if trusted["forked_from_id"] != captures[parent].session ||
                        trusted["forked_from_ordinal_exclusive"] !=
                            base["end_ordinal_exclusive"] {
                    result.block(id, "fork_dependency_unverified");
                    continue;
                }
            } else if truth(&trusted["forked_from_id"]) {
                result.block(id, "fork_dependency_conflict");
                continue;
            }
            parents.insert(id.clone(), parent.clone());
            if result.capture_ranges(parent).iter().any(|(start, end)|
                *start < base["end_byte_offset"].as_u64().unwrap_or(0) && *end > 0) {
                result.block(id, "inherited_deleted_range");
            }
        }
        for id in cycle_reachable(&parents, parents.keys().cloned()) {
            result.block(&id, "dependency_cycle");
        }
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, p) in parents { children.entry(p).or_default().push(id); }
        let mut pending: VecDeque<String> =
            result.blocked.keys().cloned().collect();
        while let Some(p) = pending.pop_front() {
            for id in children.get(&p).into_iter().flatten() {
                if !result.blocked.contains_key(id) {
                    result.block(id, "upstream_body_blocked");
                    pending.push_back(id.clone());
                }
            }
        }
        for (id, reasons) in &result.blocked {
            result.blocked_sessions.entry(captures[id].session_id()).or_default().extend(reasons.iter().cloned());
        }
        Ok(result)
    }
    fn block(&mut self, id: &str, reason: &str) {
        self.blocked.entry(id.into()).or_default().insert(reason.into());
    }
    fn capture_ranges(&self, id: &str) -> Vec<(u64, u64)> {
        let c = &self.captures[id];
        if self.sessions.contains(&c.session_id()) {
            vec![(0, c.length)]
        } else {
            self.ranges.get(&(c.source.clone(), c.generation.clone(), c.capture.clone())).cloned().unwrap_or_default()
        }
    }
    pub fn explicitly_deleted(&self, v: &Value, kind: &str) -> bool {
        let sid = v.get("session_id").or_else(|| if kind == "session" { v.get("id") } else { None });
        if sid.and_then(Value::as_str).is_some_and(|s| self.sessions.contains(s)) {
            return true;
        }
        // Aggregate boundaries do not authorize erasing the whole aggregate.
        if matches!(kind, "capture" | "session") { return false; }
        arr(&v["anchors"]).iter().any(|a| {
            let k = anchor_key(a);
            if self.by_anchor.get(&k).is_some_and(|id| self.sessions.contains(&self.captures[id].session_id())) {
                return true;
            }
            self.ranges.get(&k).into_iter().flatten().any(|(start, end)|
                a["start"].as_u64().is_some_and(|n| n < *end) &&
                a["end"].as_u64().is_some_and(|n| *start < n))
        })
    }
    pub fn public(&self, mut v: Value, kind: &str) -> Value {
        if self.explicitly_deleted(&v, kind) {
            if v.get("body").is_some() { v["body"] = Value::Null; }
            v["status"] = json!("deleted");
            v["body_access"] = json!("deleted");
            v["reasons"] = json!(["explicit_deletion"]);
            return v;
        }
        let mut affected = BTreeSet::new();
        for a in arr(&v["anchors"]) {
            let k =
                (a["source"].as_str().unwrap_or("").into(),
                    a["generation"].as_str().unwrap_or("").into(),
                    a["capture"].as_str().unwrap_or("").into());
            if let Some(id) = self.by_anchor.get(&k) {
                affected.extend(self.blocked.get(id).into_iter().flatten().cloned());
            }
        }
        if kind == "evidence" {
            affected.extend(self.blocked_sessions.get(v["session_id"].as_str().unwrap_or("")).into_iter().flatten().cloned());
        }
        if affected.is_empty() {
            v["body_access"] = json!("available");
        } else {
            if v.get("body").is_some() { v["body"] = Value::Null; }
            v["body_access"] = json!("blocked");
            v["body_reasons"] = json!(affected);
        }
        v
    }
}

fn anchor_key(a: &Value) -> (String, String, String) {
    (a["source"].as_str().unwrap_or("").into(),
     a["generation"].as_str().unwrap_or("").into(),
     a["capture"].as_str().unwrap_or("").into())
}
