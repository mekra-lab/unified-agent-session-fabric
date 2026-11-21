use std::collections::{BTreeMap, BTreeSet};
/// Iterative traversal; includes tails whose parent path reaches a cycle.
pub fn cycle_reachable(parent: &BTreeMap<String, String>,
    starts: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    let mut resolved = BTreeMap::new();
    for start in starts {
        if resolved.contains_key(&start) { continue; }
        let mut path = Vec::new();
        let mut visiting = BTreeSet::new();
        let mut node = Some(start);
        while let Some(n) = node.as_ref() {
            if resolved.contains_key(n) || visiting.contains(n) { break; }
            path.push(n.clone());
            visiting.insert(n.clone());
            node = parent.get(n).cloned();
        }
        let reaches =
            node.is_some_and(|n|
                    visiting.contains(&n) ||
                        *resolved.get(&n).unwrap_or(&false));
        for n in path { resolved.insert(n, reaches); }
    }
    resolved.into_iter().filter_map(|(n, yes)| yes.then_some(n)).collect()
}
