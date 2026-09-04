//! Reading the operation log out of a container.
//!
//! Observed in test fixtures:
//! The framing carries `deltas: {firstSequenceNumber, deltas: [...]}` where each
//! entry is a JSON *string* holding one sequenced message. Its `contents` is
//! itself a JSON-encoded string, nested several levels deep, and addressed as it
//! descends:
//!
//! ```json
//! {"contents":{"type":"component",
//!   "contents":{"address":"<channel id>",
//!     "contents":{"content":{"address":"text",
//!       "contents":{"pos1":1,"seg":{"text":"typed text","props":{...}},"type":0}},
//!     "type":"op"}}}}
//! ```
//!
//! The outer `address` is the channel; the inner `content.address` names the
//! distributed data structure within it, observed as `text`.
//!
//! There is no LZ4 anywhere in the corpus: operations are plain JSON.
//!
//! Merge-tree operation types, matching the Fluid delta type numbering:
//! `0` insert, `1` remove, `2` annotate, `3` group. Group operations nest
//! ordinary operations inside themselves and are flattened here.
//!
//! This is reverse-engineered behaviour and is not based on a published
//! Microsoft Prague/Fluid file-format specification.

use serde::Serialize;
use serde_json::Value;

use crate::fluid::merge_tree::Segment;
use crate::fluid::properties::Properties;

/// Fluid merge-tree delta type numbers.
const TYPE_INSERT: i64 = 0;
const TYPE_REMOVE: i64 = 1;
const TYPE_ANNOTATE: i64 = 2;

/// One operation against a merge tree.
#[derive(Debug, Clone)]
pub enum DocumentOp {
    /// Insert segments at a position, counted in UTF-16 code units.
    Insert {
        position: usize,
        segments: Vec<Segment>,
    },
    /// Remove the range `[start, end)`.
    Remove { start: usize, end: usize },
    /// Change properties over the range `[start, end)`.
    ///
    /// Parsed and reported, but not applied: no corpus file needs it to
    /// reconstruct its text, and applying property changes correctly needs
    /// range splitting that is not yet justified.
    Annotate {
        start: usize,
        end: usize,
        properties: Properties,
    },
    /// An operation whose shape is not understood. Kept for diagnostics.
    Unknown { raw: Value },
}

impl DocumentOp {
    pub fn kind(&self) -> &'static str {
        match self {
            DocumentOp::Insert { .. } => "insert",
            DocumentOp::Remove { .. } => "remove",
            DocumentOp::Annotate { .. } => "annotate",
            DocumentOp::Unknown { .. } => "unknown",
        }
    }
}

/// An operation together with what it targets.
#[derive(Debug, Clone)]
pub struct AddressedOp {
    /// Channel the operation applies to, for example a data store guid.
    pub channel: Option<String>,
    /// Data structure within that channel, observed as `text`.
    pub data_structure: Option<String>,
    pub op: DocumentOp,
}

impl AddressedOp {
    /// `channel/data_structure`, the key operations are grouped by.
    pub fn address(&self) -> String {
        match (&self.channel, &self.data_structure) {
            (Some(channel), Some(ds)) => format!("{channel}/{ds}"),
            (Some(channel), None) => channel.clone(),
            _ => String::new(),
        }
    }
}

/// Counts of what an operation log holds, for reporting.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OperationCounts {
    pub records: usize,
    pub inserts: usize,
    pub removes: usize,
    pub annotates: usize,
    pub unknown: usize,
}

/// Parses every operation out of the raw delta strings.
pub fn parse(deltas: &[String]) -> Vec<AddressedOp> {
    let mut ops = Vec::new();
    for delta in deltas {
        let Ok(record) = serde_json::from_str::<Value>(delta) else {
            continue;
        };
        collect(&record, None, None, &mut ops);
    }
    ops
}

pub fn counts(ops: &[AddressedOp]) -> OperationCounts {
    let mut counts = OperationCounts {
        records: ops.len(),
        ..Default::default()
    };
    for op in ops {
        match op.op {
            DocumentOp::Insert { .. } => counts.inserts += 1,
            DocumentOp::Remove { .. } => counts.removes += 1,
            DocumentOp::Annotate { .. } => counts.annotates += 1,
            DocumentOp::Unknown { .. } => counts.unknown += 1,
        }
    }
    counts
}

/// Walks a message, carrying the address down and unwrapping JSON strings.
fn collect(
    node: &Value,
    channel: Option<&str>,
    data_structure: Option<&str>,
    ops: &mut Vec<AddressedOp>,
) {
    match node {
        Value::Array(items) => {
            for item in items {
                collect(item, channel, data_structure, ops);
            }
        }
        Value::Object(map) => {
            // An `address` names either the channel or, under `content`, the
            // data structure inside it. The outer one is seen first.
            let (channel, data_structure) = match map.get("address").and_then(Value::as_str) {
                Some(address) if channel.is_none() => (Some(address), data_structure),
                Some(address) => (channel, Some(address)),
                None => (channel, data_structure),
            };

            if let Some(op) = as_merge_tree_op(map) {
                ops.push(AddressedOp {
                    channel: channel.map(str::to_string),
                    data_structure: data_structure.map(str::to_string),
                    op,
                });
                return;
            }

            for value in map.values() {
                // `contents` is frequently a JSON document encoded as a string.
                if let Some(text) = value.as_str() {
                    let trimmed = text.trim_start();
                    if trimmed.starts_with('{') || trimmed.starts_with('[') {
                        if let Ok(nested) = serde_json::from_str::<Value>(text) {
                            collect(&nested, channel, data_structure, ops);
                            continue;
                        }
                    }
                }
                collect(value, channel, data_structure, ops);
            }
        }
        _ => {}
    }
}

/// Recognises a merge-tree operation by its numeric `type`.
fn as_merge_tree_op(map: &serde_json::Map<String, Value>) -> Option<DocumentOp> {
    let op_type = map.get("type")?.as_i64()?;
    let pos1 = map.get("pos1").and_then(Value::as_u64).map(|p| p as usize);

    match op_type {
        TYPE_INSERT => {
            let seg = map.get("seg")?;
            let segments = match seg {
                Value::Array(items) => items.iter().map(Segment::from_value).collect(),
                other => vec![Segment::from_value(other)],
            };
            Some(DocumentOp::Insert {
                position: pos1.unwrap_or(0),
                segments,
            })
        }
        TYPE_REMOVE => {
            let start = pos1?;
            let end = map.get("pos2").and_then(Value::as_u64)? as usize;
            Some(DocumentOp::Remove { start, end })
        }
        TYPE_ANNOTATE => {
            let start = pos1?;
            let end = map.get("pos2").and_then(Value::as_u64)? as usize;
            let properties = map
                .get("props")
                .and_then(Value::as_object)
                .map(|m| Properties(m.clone()))
                .unwrap_or_default();
            Some(DocumentOp::Annotate {
                start,
                end,
                properties,
            })
        }
        // Group operations carry their children in `ops`; the caller's walk
        // reaches them, so nothing is emitted here.
        _ => None,
    }
}
