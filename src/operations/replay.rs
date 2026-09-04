//! Rebuilding a merge tree by applying its operations in order.
//!
//! Positions in merge-tree operations count UTF-16 code units, because the
//! Fluid runtime that wrote them is JavaScript. Text segments therefore measure
//! their length in UTF-16 units here too, and a marker counts as one unit.
//!
//! Only insert and remove are applied. Annotate is parsed and counted, but
//! applying it needs range splitting that no corpus file requires to recover
//! its text.

use crate::fluid::merge_tree::Segment;
use crate::warning::Warning;

use super::log::DocumentOp;

#[derive(Debug, Default)]
pub struct Replay {
    pub segments: Vec<Segment>,
    pub applied: usize,
    pub skipped: usize,
    pub warnings: Vec<Warning>,
}

/// Length of a segment in UTF-16 code units, matching merge-tree positions.
fn segment_len(segment: &Segment) -> usize {
    match segment {
        Segment::Text { text, .. } => text.encode_utf16().count(),
        // A marker occupies one position.
        Segment::Marker { .. } | Segment::Unknown { .. } => 1,
    }
}

/// Applies operations in order, starting from an empty document.
pub fn replay(ops: &[DocumentOp]) -> Replay {
    replay_onto(Vec::new(), ops)
}

/// Applies operations in order on top of an existing segment list.
///
/// Seeding with the snapshot's own segments is only sound when the operation
/// log begins where the snapshot ends; see
/// `EnvelopeIndex::operations_follow_snapshot`.
pub fn replay_onto(base: Vec<Segment>, ops: &[DocumentOp]) -> Replay {
    let mut result = Replay {
        segments: base,
        ..Default::default()
    };

    for op in ops {
        match op {
            DocumentOp::Insert { position, segments } => {
                if insert(&mut result.segments, *position, segments) {
                    result.applied += 1;
                } else {
                    result.skipped += 1;
                    result.warnings.push(Warning::new(
                        "operation_position_out_of_range",
                        format!(
                            "an insert at position {position} is past the end of the reconstructed \
                             text; it was appended instead"
                        ),
                    ));
                }
            }
            DocumentOp::Remove { start, end } => {
                if remove(&mut result.segments, *start, *end) {
                    result.applied += 1;
                } else {
                    result.skipped += 1;
                    result.warnings.push(Warning::new(
                        "operation_position_out_of_range",
                        format!("a remove of [{start}, {end}) does not fit the reconstructed text"),
                    ));
                }
            }
            DocumentOp::Annotate { .. } => {
                result.skipped += 1;
            }
            DocumentOp::Unknown { .. } => {
                result.skipped += 1;
            }
        }
    }

    result
}

/// Splits `segments` so that a boundary exists at `position`.
///
/// Returns the index of the segment that starts at `position`, or `None` when
/// the position is past the end.
fn split_at(segments: &mut Vec<Segment>, position: usize) -> Option<usize> {
    let mut offset = 0usize;
    for index in 0..segments.len() {
        if offset == position {
            return Some(index);
        }
        let len = segment_len(&segments[index]);
        if position < offset + len {
            // The position falls inside this segment. Only text can be split.
            let Segment::Text { text, props } = &segments[index] else {
                return None;
            };
            let inner = position - offset;
            let byte = utf16_to_byte_offset(text, inner)?;
            let (head, tail) = text.split_at(byte);
            let props = props.clone();
            let head = Segment::Text {
                text: head.to_string(),
                props: props.clone(),
            };
            let tail = Segment::Text {
                text: tail.to_string(),
                props,
            };
            segments[index] = head;
            segments.insert(index + 1, tail);
            return Some(index + 1);
        }
        offset += len;
    }
    (offset == position).then_some(segments.len())
}

fn insert(segments: &mut Vec<Segment>, position: usize, new: &[Segment]) -> bool {
    match split_at(segments, position) {
        Some(index) => {
            for (offset, segment) in new.iter().enumerate() {
                segments.insert(index + offset, segment.clone());
            }
            true
        }
        None => {
            segments.extend(new.iter().cloned());
            false
        }
    }
}

fn remove(segments: &mut Vec<Segment>, start: usize, end: usize) -> bool {
    if end <= start {
        return false;
    }
    let Some(_) = split_at(segments, end) else {
        return false;
    };
    let Some(from) = split_at(segments, start) else {
        return false;
    };
    // `end` may have moved if splitting at `start` inserted a segment before it.
    let mut offset = 0usize;
    let mut to = segments.len();
    for (index, segment) in segments.iter().enumerate() {
        if offset == end {
            to = index;
            break;
        }
        offset += segment_len(segment);
    }
    if from > to {
        return false;
    }
    segments.drain(from..to);
    true
}

/// Converts a UTF-16 offset within `text` into a byte offset.
///
/// Returns `None` when the offset falls inside a surrogate pair, which would
/// mean splitting a single character in half.
fn utf16_to_byte_offset(text: &str, utf16_offset: usize) -> Option<usize> {
    let mut units = 0usize;
    for (byte, character) in text.char_indices() {
        if units == utf16_offset {
            return Some(byte);
        }
        units += character.len_utf16();
        if units > utf16_offset {
            return None;
        }
    }
    (units == utf16_offset).then_some(text.len())
}
