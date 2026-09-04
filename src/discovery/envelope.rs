//! Narrow reader for the binary member framing.
//!
//! Purpose: recover *where* each payload sits, not to decode the format fully.
//! A merge-tree chunk on its own cannot say whether it is the page title, the
//! page body or a table cell, because all three are structurally identical.
//! The framing knows, and this module asks it.
//!
//! # The framing, as observed
//!
//! A member is a tagged value stream with an interned string table. Decoding it
//! yields a Fluid summary:
//!
//! ```text
//! {mrv, cv, lsn, snapshot: {id, sequenceNumber, treeNodes: [...]}, blobs: [...], deltas: {...}}
//! ```
//!
//! `treeNodes` is a tree of `{name, children}` and `{name, nodeType, value}`
//! nodes, where `value` is a content id. `blobs` is a flat list of
//! `{id, data}` records holding the content those ids refer to. Walking the
//! tree gives every blob a path such as:
//!
//! ```text
//! /.app/.channels/E/.channels/text/content/header
//! /.app/.channels/76803cb0-1356-4408-87b0-072f2d0301cb/.channels/text/content/header
//! ```
//!
//! The second path's channel id happens to recur unchanged in unrelated
//! fixtures written months apart, which suggests it is a stable identifier for
//! the Loop header component. That is an observation about the fixtures, not a
//! documented Microsoft constant, so nothing here matches on it.
//!
//! Classification uses the component name instead. Each channel's `.component`
//! blob names the component that owns it, and those names are unambiguous:
//! `LoopCanvasComponentSingleton` is the page body, `LoopPageTitleSingleton` is
//! the page title, `TableroComponentType` is a table,
//! `BlockCalloutComponentType` is a callout card.
//!
//! # Tag table
//!
//! Established by decoding 13 files to completion; 12 of the 13 decode with no
//! byte left over, which is the evidence that these widths are right. Tags not
//! listed here have not been seen, and meeting one stops the decode rather than
//! risking a silent desync.
//!
//! ```text
//! 0x01                                    integer zero
//! 0x03 <u8>                               integer
//! 0x05 <u16le>                            integer
//! 0x07 <u32le>                            integer
//! 0x0b / 0x0c                             true / false
//! 0x0d                                    null
//! 0x0e <u8 len> <bytes>                   string literal
//! 0x0f <u16le len> <bytes>                string literal
//! 0x10 <u32le len> <bytes>                string literal
//! 0x11 <u8 id>                            interned string reference
//! 0x12 <u16le id>                         interned string reference
//! 0x13 <u32le id>                         interned string reference
//! 0x14 <u8 id> <u8 len> <bytes>           intern a string
//! 0x15 <u32le id> <u32le len> <bytes>     intern a string (wide)
//! 0x21 <u8 len> <bytes>                   blob
//! 0x22 <u16le len> <bytes>                blob
//! 0x23 <u32le len> <bytes>                blob
//! 0x31 / 0x32                             open / close array
//! 0x33 / 0x34                             open / close map
//! ```
//!
//! ASSUMPTION, not confirmed: the `0x07`, `0x10`, `0x13` and `0x23` widths are
//! extrapolated from the pattern of their narrower siblings. They do not occur
//! in the corpus, so they are untested.
//!
//! This is reverse-engineered behaviour and is not based on a published
//! Microsoft Prague/Fluid file-format specification.

use std::collections::BTreeMap;

use serde::Serialize;
use thiserror::Error;

use super::{DiscoveryError, PayloadCandidate, PayloadDiscovery};

#[derive(Debug, Error)]
pub enum EnvelopeError {
    #[error("unknown tag 0x{tag:02x} at offset {offset}")]
    UnknownTag { tag: u8, offset: usize },
    #[error("member ends part way through a value at offset {offset}")]
    Truncated { offset: usize },
    #[error("value nesting deeper than {limit} levels")]
    TooDeep { limit: usize },
    #[error("this member is not an envelope: {0}")]
    NotAnEnvelope(&'static str),
}

/// Maximum nesting accepted while parsing, to bound recursion on damaged input.
const MAX_DEPTH: usize = 256;

/// A decoded value from the framing.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    /// A byte range within the decompressed member.
    Blob {
        start: usize,
        len: usize,
    },
    Array(Vec<Value>),
    /// Key order is preserved; keys are not required to be strings.
    Map(Vec<(Value, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries
                .iter()
                .find(|(k, _)| matches!(k, Value::Str(name) if name == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_blob(&self) -> Option<(usize, usize)> {
        match self {
            Value::Blob { start, len } => Some((*start, *len)),
            _ => None,
        }
    }
}

/// Where one payload sits inside the container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PayloadLocation {
    pub member_index: usize,
    /// Full summary-tree path, for example
    /// `/.app/.channels/E/.channels/text/content/header`.
    pub blob_path: Option<String>,
    /// The first channel on that path: the component or data store id.
    pub channel_id: Option<String>,
    /// The Fluid handle that points at this channel, when one does.
    pub handle_name: Option<String>,
    /// The channel one level up, when the path nests channels.
    pub parent_channel: Option<String>,
    /// Component package that owns the channel, for example
    /// `LoopCanvasComponentSingleton`.
    pub component_package: Option<String>,
}

/// What one member's framing says about its contents.
#[derive(Debug, Clone, Default)]
pub struct EnvelopeIndex {
    /// Blob id to summary-tree path.
    pub blob_paths: BTreeMap<String, String>,
    /// Blob id to its byte range in the decompressed member.
    pub blob_ranges: BTreeMap<String, (usize, usize)>,
    /// Channel id to the component package that owns it.
    pub channel_packages: BTreeMap<String, String>,
    /// Fluid handle name to the url it points at, for example
    /// `CanvasComponentHandle` to `/E`.
    pub handles: BTreeMap<String, String>,
    /// Raw operation-log messages, each a JSON document as a string.
    pub deltas: Vec<String>,
    /// `snapshot.sequenceNumber`: the last operation folded into the snapshot.
    pub snapshot_sequence_number: Option<i64>,
    /// `deltas.firstSequenceNumber`: the first operation held separately.
    pub first_delta_sequence_number: Option<i64>,
}

impl EnvelopeIndex {
    /// True when the operation log begins exactly where the snapshot ends, so
    /// replaying it on top of the snapshot applies nothing twice.
    ///
    /// Observed in every corpus file that carries both:
    /// `firstSequenceNumber == snapshot.sequenceNumber + 1`.
    pub fn operations_follow_snapshot(&self) -> bool {
        match (
            self.snapshot_sequence_number,
            self.first_delta_sequence_number,
        ) {
            (Some(snapshot), Some(first)) => first == snapshot + 1,
            _ => false,
        }
    }
}

impl EnvelopeIndex {
    /// Finds the blob whose byte range contains `offset`, and describes it.
    pub fn locate(&self, member_index: usize, offset: usize) -> Option<PayloadLocation> {
        let (id, _) = self
            .blob_ranges
            .iter()
            .find(|(_, (start, len))| offset >= *start && offset < start + len)?;
        let path = self.blob_paths.get(id)?;
        let channels = channel_segments(path);
        let channel_id = channels.first().map(|s| s.to_string());
        let parent_channel = (channels.len() > 1).then(|| channels[0].to_string());

        let component_package = channel_id
            .as_ref()
            .and_then(|id| self.channel_packages.get(id))
            .cloned();
        let handle_name = channel_id.as_ref().and_then(|id| {
            self.handles
                .iter()
                .find(|(_, url)| url.trim_start_matches('/') == id)
                .map(|(name, _)| name.clone())
        });

        Some(PayloadLocation {
            member_index,
            blob_path: Some(path.clone()),
            channel_id,
            handle_name,
            parent_channel,
            component_package,
        })
    }
}

/// The segments that follow each `.channels` element of a path.
fn channel_segments(path: &str) -> Vec<&str> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    parts
        .iter()
        .enumerate()
        .filter(|(index, part)| **part == ".channels" && index + 1 < parts.len())
        .map(|(index, _)| parts[index + 1])
        .collect()
}

/// Decodes a member and indexes what it says about payload locations.
///
/// Returns `Err` when the member is not an envelope or uses a tag this reader
/// does not know. Callers fall back to content heuristics; nothing depends on
/// this succeeding.
pub fn read(member: &[u8]) -> Result<EnvelopeIndex, EnvelopeError> {
    let root = parse(member)?;
    let snapshot = root
        .get("snapshot")
        .ok_or(EnvelopeError::NotAnEnvelope("no snapshot value"))?;
    let tree_nodes = snapshot
        .get("treeNodes")
        .and_then(Value::as_array)
        .ok_or(EnvelopeError::NotAnEnvelope("no treeNodes array"))?;

    let mut index = EnvelopeIndex::default();
    walk_tree(tree_nodes, "", &mut index.blob_paths);

    if let Some(blobs) = root.get("blobs").and_then(Value::as_array) {
        for record in blobs {
            let Some(id) = record.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(range) = record.get("data").and_then(Value::as_blob) else {
                continue;
            };
            index.blob_ranges.insert(id.to_string(), range);
        }
    }

    index.snapshot_sequence_number = match snapshot.get("sequenceNumber") {
        Some(Value::Int(number)) => Some(*number),
        _ => None,
    };
    if let Some(Value::Int(first)) = root
        .get("deltas")
        .and_then(|d| d.get("firstSequenceNumber"))
    {
        index.first_delta_sequence_number = Some(*first);
    }

    // Operations live alongside the snapshot, as JSON strings.
    if let Some(deltas) = root
        .get("deltas")
        .and_then(|d| d.get("deltas"))
        .and_then(Value::as_array)
    {
        index.deltas = deltas
            .iter()
            .filter_map(|d| d.as_str().map(str::to_string))
            .collect();
    }

    collect_component_metadata(member, &mut index);
    Ok(index)
}

/// Depth-first walk assigning a path to every content id in the summary tree.
fn walk_tree(nodes: &[Value], prefix: &str, paths: &mut BTreeMap<String, String>) {
    for node in nodes {
        let Some(name) = node.get("name").and_then(Value::as_str) else {
            continue;
        };
        let path = format!("{prefix}/{name}");
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            walk_tree(children, &path, paths);
        } else if let Some(value) = node.get("value").and_then(Value::as_str) {
            paths.insert(value.to_string(), path);
        }
    }
}

/// Reads component packages and Fluid handles out of the blobs.
///
/// Observed shapes:
/// * a `.component` blob holds `{"pkg":"[\"Root\",\"LoopCanvasComponentSingleton\"]", ...}`
///   where `pkg` is a JSON array encoded as a string; its last element names the
///   component;
/// * a root map blob holds
///   `{"CanvasComponentHandle":{"type":"Plain","value":{"handle":{"type":"__fluid_handle__","url":"/E"}}}}`.
fn collect_component_metadata(member: &[u8], index: &mut EnvelopeIndex) {
    let ranges: Vec<(String, (usize, usize))> = index
        .blob_ranges
        .iter()
        .map(|(id, range)| (id.clone(), *range))
        .collect();

    for (id, (start, len)) in ranges {
        let Some(end) = start.checked_add(len).filter(|end| *end <= member.len()) else {
            continue;
        };
        let data = &member[start..end];
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(data) else {
            continue;
        };

        if let Some(package) = component_package(&json) {
            if let Some(path) = index.blob_paths.get(&id) {
                if let Some(channel) = channel_segments(path).first() {
                    index
                        .channel_packages
                        .insert((*channel).to_string(), package);
                }
            }
        }
        collect_handles(&json, &mut index.handles);
    }
}

/// The last element of a `pkg` array, which names the component.
fn component_package(json: &serde_json::Value) -> Option<String> {
    let raw = json.get("pkg")?.as_str()?;
    let parts: Vec<String> = serde_json::from_str(raw).ok()?;
    parts.last().cloned()
}

/// Collects `<name>: {... "url": "/E"}` Fluid handles at any depth.
fn collect_handles(json: &serde_json::Value, handles: &mut BTreeMap<String, String>) {
    let serde_json::Value::Object(map) = json else {
        if let serde_json::Value::Array(items) = json {
            items.iter().for_each(|item| collect_handles(item, handles));
        }
        return;
    };
    for (key, value) in map {
        if let Some(url) = handle_url(value) {
            handles.insert(key.clone(), url);
        }
        collect_handles(value, handles);
    }
}

fn handle_url(value: &serde_json::Value) -> Option<String> {
    let handle = value.get("value")?.get("handle")?;
    if handle.get("type")?.as_str()? != "__fluid_handle__" {
        return None;
    }
    Some(handle.get("url")?.as_str()?.to_string())
}

// ---------------------------------------------------------------- decoding --

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    strings: BTreeMap<u32, String>,
}

/// Decodes a whole member into one value.
pub fn parse(member: &[u8]) -> Result<Value, EnvelopeError> {
    let mut decoder = Decoder {
        data: member,
        pos: 0,
        strings: BTreeMap::new(),
    };
    let value = decoder.value(0)?;
    Ok(value)
}

impl<'a> Decoder<'a> {
    fn byte(&mut self) -> Result<u8, EnvelopeError> {
        let byte = *self
            .data
            .get(self.pos)
            .ok_or(EnvelopeError::Truncated { offset: self.pos })?;
        self.pos += 1;
        Ok(byte)
    }

    fn uint(&mut self, width: usize) -> Result<u64, EnvelopeError> {
        let end = self.pos + width;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(EnvelopeError::Truncated { offset: self.pos })?;
        self.pos = end;
        Ok(slice
            .iter()
            .rev()
            .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)))
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], EnvelopeError> {
        let end = self.pos + len;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(EnvelopeError::Truncated { offset: self.pos })?;
        self.pos = end;
        Ok(slice)
    }

    fn string(&mut self, width: usize) -> Result<String, EnvelopeError> {
        let len = self.uint(width)? as usize;
        let bytes = self.bytes(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    /// Reads the next value, consuming any string definitions in the way.
    ///
    /// A definition populates the string table and yields no value, so it is
    /// skipped over rather than returned.
    fn value(&mut self, depth: usize) -> Result<Value, EnvelopeError> {
        if depth > MAX_DEPTH {
            return Err(EnvelopeError::TooDeep { limit: MAX_DEPTH });
        }
        loop {
            let offset = self.pos;
            let tag = self.byte()?;
            return Ok(match tag {
                0x14 => {
                    let id = u32::from(self.byte()?);
                    let text = self.string(1)?;
                    self.strings.insert(id, text);
                    continue;
                }
                0x15 => {
                    let id = self.uint(4)? as u32;
                    let text = self.string(4)?;
                    self.strings.insert(id, text);
                    continue;
                }
                0x01 => Value::Int(0),
                0x03 => Value::Int(i64::from(self.byte()?)),
                0x05 => Value::Int(self.uint(2)? as i64),
                0x07 => Value::Int(self.uint(4)? as i64),
                0x0b => Value::Bool(true),
                0x0c => Value::Bool(false),
                0x0d => Value::Null,
                0x0e => Value::Str(self.string(1)?),
                0x0f => Value::Str(self.string(2)?),
                0x10 => Value::Str(self.string(4)?),
                0x11..=0x13 => {
                    let width = match tag {
                        0x11 => 1,
                        0x12 => 2,
                        _ => 4,
                    };
                    let id = self.uint(width)? as u32;
                    Value::Str(self.strings.get(&id).cloned().unwrap_or_default())
                }
                0x21..=0x23 => {
                    let width = match tag {
                        0x21 => 1,
                        0x22 => 2,
                        _ => 4,
                    };
                    let len = self.uint(width)? as usize;
                    let start = self.pos;
                    self.bytes(len)?;
                    Value::Blob { start, len }
                }
                0x31 => {
                    let mut items = Vec::new();
                    while !self.at_close(0x32)? {
                        items.push(self.value(depth + 1)?);
                    }
                    self.pos += 1;
                    Value::Array(items)
                }
                0x33 => {
                    let mut entries = Vec::new();
                    while !self.at_close(0x34)? {
                        let key = self.value(depth + 1)?;
                        let value = self.value(depth + 1)?;
                        entries.push((key, value));
                    }
                    self.pos += 1;
                    Value::Map(entries)
                }
                other => return Err(EnvelopeError::UnknownTag { tag: other, offset }),
            });
        }
    }

    /// True when the next value token is the given closing tag.
    ///
    /// String definitions may sit between the last entry and the close, so they
    /// are consumed here too.
    fn at_close(&mut self, close: u8) -> Result<bool, EnvelopeError> {
        loop {
            let tag = *self
                .data
                .get(self.pos)
                .ok_or(EnvelopeError::Truncated { offset: self.pos })?;
            match tag {
                0x14 => {
                    self.pos += 1;
                    let id = u32::from(self.byte()?);
                    let text = self.string(1)?;
                    self.strings.insert(id, text);
                }
                0x15 => {
                    self.pos += 1;
                    let id = self.uint(4)? as u32;
                    let text = self.string(4)?;
                    self.strings.insert(id, text);
                }
                other => return Ok(other == close),
            }
        }
    }
}

/// Yields the JSON blobs the framing describes.
///
/// The fragment scanner finds the same payloads without needing the framing to
/// decode, so this exists to honour the [`PayloadDiscovery`] interface and for
/// comparing the two paths, not as the production discovery route.
#[derive(Debug, Default)]
pub struct EnvelopeDecoder;

impl PayloadDiscovery for EnvelopeDecoder {
    fn name(&self) -> &'static str {
        "envelope"
    }

    fn discover(
        &self,
        member_index: usize,
        member: &[u8],
    ) -> Result<Vec<PayloadCandidate>, DiscoveryError> {
        let index = read(member).map_err(|e| DiscoveryError::Unsupported(e.to_string()))?;
        let mut found = Vec::new();
        for (start, len) in index.blob_ranges.values() {
            let Some(end) = start.checked_add(*len).filter(|end| *end <= member.len()) else {
                continue;
            };
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&member[*start..end]) {
                if value.is_object() {
                    found.push(PayloadCandidate {
                        member_index,
                        start: *start,
                        end,
                        anchor: "envelope-blob",
                        value,
                    });
                }
            }
        }
        found.sort_by_key(|p| (p.start, p.end));
        Ok(found)
    }
}
