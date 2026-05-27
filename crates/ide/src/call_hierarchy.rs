//! Call hierarchy — incoming/outgoing call resolution.
//! The aggregation logic lives here (not in navigation.rs) matching
//! RA's layout where call_hierarchy.rs is self-contained.

use std::collections::HashMap;

use base_db::TextRange;
use ide_db::FileDb;
use url::Url;

// Re-export raw edge API for backward compatibility
pub use crate::navigation::{call_edges_from, call_edges_to, call_hierarchy_item};
pub use ide_db::ide_types::{CallEdge, IncomingCallItem, OutgoingCallItem};

/// Per-caller/callee grouping.
///
/// Uses an order-preserving HashMap so results appear in a stable
/// order (RA uses `FxIndexMap`; we use `Vec` + dedup for simplicity
/// since we don't have `indexmap` as a dependency).
#[derive(Default)]
struct CallLocations {
    /// (caller_name, caller_uri) → call site ranges.
    entries: Vec<((String, Url), Vec<TextRange>)>,
    index: HashMap<(String, Url), usize>,
}

impl CallLocations {
    fn add(&mut self, key: (String, Url), range: TextRange) {
        if let Some(&idx) = self.index.get(&key) {
            self.entries[idx].1.push(range);
        } else {
            let idx = self.entries.len();
            self.index.insert(key.clone(), idx);
            self.entries.push((key, vec![range]));
        }
    }

    fn into_incoming_items(self) -> Vec<IncomingCallItem> {
        self.entries
            .into_iter()
            .map(|((caller, caller_uri), ranges)| IncomingCallItem { caller, caller_uri, ranges })
            .collect()
    }
}

/// Per-callee grouping for outgoing calls.
#[derive(Default)]
struct OutgoingLocations {
    entries: Vec<(String, Vec<TextRange>)>,
    index: HashMap<String, usize>,
}

impl OutgoingLocations {
    fn add(&mut self, callee: String, range: TextRange) {
        if let Some(&idx) = self.index.get(&callee) {
            self.entries[idx].1.push(range);
        } else {
            let idx = self.entries.len();
            self.index.insert(callee.clone(), idx);
            self.entries.push((callee, vec![range]));
        }
    }

    fn into_outgoing_items(self) -> Vec<OutgoingCallItem> {
        self.entries
            .into_iter()
            .map(|(callee, ranges)| OutgoingCallItem { callee, ranges })
            .collect()
    }
}

/// Aggregate incoming call edges into per-caller items.
///
/// Multiple call sites from the same caller function are grouped
/// into one `IncomingCallItem` with multiple ranges.
pub fn incoming_calls<'a, F, I>(files: I, target: &str) -> Vec<IncomingCallItem>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let edges = call_edges_to(files, target);
    let mut locs = CallLocations::default();
    for edge in edges {
        locs.add((edge.caller, edge.caller_uri), edge.call_range);
    }
    locs.into_incoming_items()
}

/// Aggregate outgoing call edges into per-callee items.
///
/// Multiple calls to the same target from within a function body
/// are grouped into one `OutgoingCallItem` with multiple ranges.
///
/// For scattered functions, all clauses' outgoing calls are
/// aggregated (Sail-specific extension).
pub fn outgoing_calls<'a, F, I>(files: I, source: &str) -> Vec<OutgoingCallItem>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let edges = call_edges_from(files, source);
    let mut locs = OutgoingLocations::default();
    for edge in edges {
        locs.add(edge.callee, edge.call_range);
    }
    locs.into_outgoing_items()
}
