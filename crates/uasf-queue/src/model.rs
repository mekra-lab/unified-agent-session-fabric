//! Scheduling is pure: time and readiness are explicit inputs, never sleeps.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub const SLOTS: usize = 2;
pub const WAITING: usize = 32;
pub const PER_SOURCE: usize = 16;
#[derive(Clone, Debug)]
pub struct Ticket { pub id: u64, pub source: String, pub deadline: u64, pub ready: bool }
#[derive(Default)]
pub struct Queue { sources: BTreeMap<String, VecDeque<Ticket>>, round: VecDeque<String>, active: BTreeSet<u64> }
impl Queue {
    pub fn len(&self) -> usize { self.sources.values().map(VecDeque::len).sum() }
    pub fn active(&self) -> usize { self.active.len() }
    pub fn submit(&mut self, t: Ticket) -> Result<(), &'static str> {
        if self.active.contains(&t.id) || self.sources.values().flatten().any(|x| x.id == t.id) { return Err("query_duplicate_request"); }
        if self.len() >= WAITING { return Err("query_queue_full"); }
        let queue = self.sources.entry(t.source.clone()).or_default();
        if queue.len() >= PER_SOURCE { return Err("query_source_queue_full"); }
        if queue.is_empty() { self.round.push_back(t.source.clone()); }
        queue.push_back(t); Ok(())
    }
    pub fn ready(&mut self, id: u64, ready: bool) {
        for t in self.sources.values_mut().flatten() { if t.id == id { t.ready = ready; } }
    }
    pub fn cancel_waiting(&mut self, id: u64) {
        for q in self.sources.values_mut() { q.retain(|t| t.id != id); }
        self.round.retain(|s| !self.sources[s].is_empty());
        self.sources.retain(|_, q| !q.is_empty());
    }
    pub fn expired(&self, now: u64) -> Vec<u64> {
        self.sources.values().flatten().filter(|t| t.deadline <= now).map(|t| t.id).collect()
    }
    pub fn dispatch(&mut self, now: u64) -> Option<Ticket> {
        if self.active.len() >= SLOTS { return None; }
        for _ in 0..self.round.len() {
            let source = self.round.pop_front()?;
            let q = self.sources.get_mut(&source).unwrap();
            // An unready entry does not participate in the ready FIFO.
            let selected = q.iter().position(|t| t.ready && t.deadline > now).and_then(|i| q.remove(i));
            if !q.is_empty() { self.round.push_back(source); } else { self.sources.remove(&source); }
            if let Some(t) = selected { self.active.insert(t.id); return Some(t); }
        }
        None
    }
    /// Only call after the guardian AND its entire worker tree have exited.
    pub fn reaped(&mut self, id: u64) { self.active.remove(&id); }
}
