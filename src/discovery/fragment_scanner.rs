//! Locates embedded JSON documents inside a binary-framed member.
//!
//! The strategy is anchor-driven rather than path-driven, because Microsoft may
//! nest Fluid state differently between Loop versions:
//!
//! 1. find a known field name (the anchor) anywhere in the member's bytes;
//! 2. walk backwards to a candidate `{`;
//! 3. brace-match forwards, tracking string state so that braces and quotes
//!    inside JSON string values are ignored;
//! 4. parse the resulting slice with `serde_json`;
//! 5. accept it only if it parses as an object and still contains the anchor.
//!
//! Candidates are tried nearest-first, so the *smallest* enclosing object wins.
//! That matters: the anchor `segmentTexts` sits inside a merge-tree chunk object
//! which itself sits inside much larger structures.
//!
//! No regex is used, and no fixed byte offset is assumed.

use memchr::memmem;
use serde_json::Value;

use super::{DiscoveryError, PayloadCandidate, PayloadDiscovery};

/// Field names that mark a payload worth parsing.
///
/// Observed in test fixtures: the first five identify Fluid merge-tree chunk
/// summaries; `package` identifies the component-identity record that names the
/// Loop component and its build version.
pub const DEFAULT_ANCHORS: &[&str] = &[
    "\"chunkStartSegmentIndex\"",
    "\"segmentTexts\"",
    "\"headerMetadata\"",
    "\"orderedChunkMetadata\"",
    "\"chunkSegmentCount\"",
    "\"package\"",
];

/// Bounds that keep a malformed member from causing unbounded work.
#[derive(Debug, Clone, Copy)]
pub struct ScannerLimits {
    /// How far back from an anchor to look for the opening brace.
    pub max_lookback: usize,
    /// How many candidate opening braces to try per anchor hit.
    pub max_brace_candidates: usize,
    /// Largest object that will be brace-matched.
    pub max_object_bytes: usize,
    /// Total accepted payloads per member.
    pub max_payloads_per_member: usize,
}

impl Default for ScannerLimits {
    fn default() -> Self {
        Self {
            max_lookback: 1024 * 1024,
            max_brace_candidates: 64,
            max_object_bytes: 32 * 1024 * 1024,
            max_payloads_per_member: 4096,
        }
    }
}

#[derive(Debug)]
pub struct FragmentScanner {
    anchors: Vec<&'static str>,
    limits: ScannerLimits,
}

impl Default for FragmentScanner {
    fn default() -> Self {
        Self {
            anchors: DEFAULT_ANCHORS.to_vec(),
            limits: ScannerLimits::default(),
        }
    }
}

impl FragmentScanner {
    pub fn with_anchors(anchors: Vec<&'static str>) -> Self {
        Self {
            anchors,
            limits: ScannerLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: ScannerLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl PayloadDiscovery for FragmentScanner {
    fn name(&self) -> &'static str {
        "fragment-scanner"
    }

    fn discover(
        &self,
        member_index: usize,
        member: &[u8],
    ) -> Result<Vec<PayloadCandidate>, DiscoveryError> {
        let mut found: Vec<PayloadCandidate> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();

        for anchor in &self.anchors {
            let finder = memmem::Finder::new(anchor.as_bytes());
            for hit in finder.find_iter(member) {
                if found.len() >= self.limits.max_payloads_per_member {
                    return Ok(found);
                }
                // Anchors often co-occur inside one object (a chunk summary
                // carries several). Skip the work when the object this hit
                // resolves to has already been accepted.
                if let Some(brace) = self.nearest_brace(member, hit, 0) {
                    if spans.iter().any(|(s, _)| *s == brace) {
                        continue;
                    }
                }
                if let Some((start, end, value)) = self.extract(member, hit) {
                    if spans.contains(&(start, end)) {
                        continue;
                    }
                    spans.push((start, end));
                    found.push(PayloadCandidate {
                        member_index,
                        start,
                        end,
                        anchor,
                        value,
                    });
                }
            }
        }

        found.sort_by_key(|p| (p.start, p.end));
        Ok(found)
    }
}

impl FragmentScanner {
    fn nearest_brace(&self, member: &[u8], hit: usize, skip: usize) -> Option<usize> {
        let floor = hit.saturating_sub(self.limits.max_lookback);
        let mut cursor = hit;
        let mut skipped = 0;
        while cursor > floor {
            let pos = memchr::memrchr(b'{', &member[floor..cursor])? + floor;
            if skipped == skip {
                return Some(pos);
            }
            skipped += 1;
            cursor = pos;
        }
        None
    }

    /// Finds the smallest JSON object that encloses `hit` and parses.
    fn extract(&self, member: &[u8], hit: usize) -> Option<(usize, usize, Value)> {
        for skip in 0..self.limits.max_brace_candidates {
            let start = self.nearest_brace(member, hit, skip)?;
            let Some(end) = match_object(member, start, self.limits.max_object_bytes) else {
                continue;
            };
            // The object must actually contain the anchor, not close before it.
            if end <= hit {
                continue;
            }
            match serde_json::from_slice::<Value>(&member[start..end]) {
                Ok(value) if value.is_object() => return Some((start, end, value)),
                _ => continue,
            }
        }
        None
    }
}

/// Returns the exclusive end offset of the JSON object opening at `start`.
///
/// Quote-aware and escape-aware: braces and quotes inside string values do not
/// affect nesting depth. Returns `None` if the object does not close within
/// `max_bytes`, or if a control byte appears where JSON forbids one, which is
/// the usual sign that `start` was a brace inside binary framing rather than the
/// start of a real object.
pub fn match_object(data: &[u8], start: usize, max_bytes: usize) -> Option<usize> {
    if data.get(start) != Some(&b'{') {
        return None;
    }
    let limit = data.len().min(start.saturating_add(max_bytes));
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, &byte) in data[start..limit].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            } else if byte < 0x20 {
                // Raw control bytes are illegal inside a JSON string. Treat this
                // as evidence that `start` was not a JSON object at all.
                return None;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + offset + 1);
                }
            }
            b'\t' | b'\n' | b'\r' | b' ' => {}
            0x00..=0x1f => return None,
            _ => {}
        }
    }
    None
}
