//! Operation-log reading and replay.
//!
//! Kept behind its own module boundary: snapshot parsing never calls into it,
//! and it depends on the snapshot only for the `Segment` type the two formats
//! share.

pub mod log;
pub mod replay;

use std::collections::BTreeMap;

use serde::Serialize;

use crate::fluid::merge_tree::Segment;

pub use log::{AddressedOp, DocumentOp, OperationCounts};

/// Every operation a container holds, grouped by what it targets.
#[derive(Debug, Default)]
pub struct OperationLog {
    pub ops: Vec<AddressedOp>,
    pub counts: OperationCounts,
    /// True when the log provably begins where the snapshot ends, so it can be
    /// replayed on top of the snapshot without applying anything twice.
    pub follows_snapshot: bool,
}

/// The outcome of rebuilding one address's content from operations.
#[derive(Debug, Clone, Serialize)]
pub struct ReplaySummary {
    pub address: String,
    /// True when the snapshot's own segments were the starting point.
    pub seeded_from_snapshot: bool,
    pub channel: Option<String>,
    pub component_package: Option<String>,
    pub operations_applied: usize,
    pub operations_skipped: usize,
    pub segments: usize,
}

impl OperationLog {
    pub fn parse(deltas: &[String]) -> Self {
        let ops = log::parse(deltas);
        let counts = log::counts(&ops);
        Self {
            ops,
            counts,
            follows_snapshot: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Insert counts per address, highest first.
    pub fn addresses_by_inserts(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for op in &self.ops {
            if matches!(op.op, DocumentOp::Insert { .. }) {
                *counts.entry(op.address()).or_default() += 1;
            }
        }
        let mut ordered: Vec<(String, usize)> = counts.into_iter().collect();
        // Deterministic: most inserts first, then by address.
        ordered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        ordered
    }

    /// Replays every operation addressed to `address`, in log order, on top of
    /// `base`.
    pub fn replay_address(&self, address: &str, base: Vec<Segment>) -> replay::Replay {
        let ops: Vec<DocumentOp> = self
            .ops
            .iter()
            .filter(|op| op.address() == address)
            .map(|op| op.op.clone())
            .collect();
        replay::replay_onto(base, &ops)
    }

    /// The channel part of an address.
    pub fn channel_of(&self, address: &str) -> Option<String> {
        self.ops
            .iter()
            .find(|op| op.address() == address)
            .and_then(|op| op.channel.clone())
    }
}

/// Plain text of a replayed segment list, for reporting.
pub fn segments_text(segments: &[Segment]) -> String {
    segments.iter().filter_map(Segment::text).collect()
}
