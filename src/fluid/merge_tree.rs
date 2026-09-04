//! Fluid merge-tree chunk parsing and tree assembly.
//!
//! Observed in test fixtures:
//! Document content lives in merge-tree *chunk summaries*. A chunk carries an
//! ordered `segmentTexts` array plus counters describing its place in the whole
//! tree:
//!
//! ```json
//! {"chunkStartSegmentIndex":80,"chunkSegmentCount":54,"chunkLengthChars":4637,
//!  "totalLengthChars":14654,"totalSegmentCount":134,"chunkSequenceNumber":304,
//!  "segmentTexts":[ ... ]}
//! ```
//!
//! A tree may be split across chunks: one fixture stores its 134-segment body as
//! segments 0..79 and 80..133. Chunks must therefore be grouped and
//! reassembled in order, not treated as independent documents.
//!
//! The same chunk also appears more than once in a file - once inside a large
//! aggregate member and once as a standalone member - byte for byte identical.
//! Those duplicates are collapsed by content hash.
//!
//! This is reverse-engineered behaviour and is not based on a published
//! Microsoft Prague/Fluid file-format specification.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::properties::Properties;
use crate::discovery::envelope::PayloadLocation;
use crate::discovery::PayloadCandidate;
use crate::warning::Warning;

/// One entry of `segmentTexts`.
#[derive(Debug, Clone)]
pub enum Segment {
    Text {
        text: String,
        props: Properties,
    },
    Marker {
        ref_type: Option<i64>,
        props: Properties,
    },
    /// An entry matching no known shape. Preserved so it reaches diagnostics.
    Unknown {
        raw: Value,
    },
}

impl Segment {
    pub fn props(&self) -> Option<&Properties> {
        match self {
            Segment::Text { props, .. } | Segment::Marker { props, .. } => Some(props),
            Segment::Unknown { .. } => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Segment::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    /// Parses one `segmentTexts` entry.
    ///
    /// Observed: an entry is either a bare string (text with no properties) or
    /// an object with a `text` or `marker` field.
    pub fn from_value(value: &Value) -> Self {
        if let Some(text) = value.as_str() {
            return Segment::Text {
                text: text.to_string(),
                props: Properties::default(),
            };
        }
        let Some(object) = value.as_object() else {
            return Segment::Unknown { raw: value.clone() };
        };
        let props = object
            .get("props")
            .and_then(|p| p.as_object())
            .map(|m| Properties(m.clone()))
            .unwrap_or_default();

        if let Some(text) = object.get("text").and_then(|t| t.as_str()) {
            Segment::Text {
                text: text.to_string(),
                props,
            }
        } else if let Some(marker) = object.get("marker") {
            let ref_type = marker.get("refType").and_then(|r| r.as_i64());
            Segment::Marker { ref_type, props }
        } else {
            Segment::Unknown { raw: value.clone() }
        }
    }
}

/// The counters a chunk carries about itself and its tree.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChunkSummary {
    #[serde(default)]
    chunk_start_segment_index: usize,
    chunk_segment_count: Option<usize>,
    chunk_length_chars: Option<usize>,
    total_length_chars: Option<usize>,
    total_segment_count: Option<usize>,
    chunk_sequence_number: Option<i64>,
    segment_texts: Option<Vec<Value>>,
}

/// Where a chunk was found.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceLocation {
    pub member_index: usize,
    /// Offset of the gzip signature within the source file.
    pub member_file_offset: usize,
    /// Offset of the JSON payload within the decompressed member.
    pub payload_offset: usize,
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub start_segment_index: usize,
    pub segment_count: Option<usize>,
    pub length_chars: Option<usize>,
    pub total_length_chars: Option<usize>,
    pub total_segment_count: Option<usize>,
    pub sequence_number: Option<i64>,
    pub segments: Vec<Segment>,
    pub source: SourceLocation,
    /// SHA-256 of the raw JSON bytes this chunk was parsed from.
    pub sha256: [u8; 32],
    /// Where the container framing says this chunk lives, when the framing
    /// decoded. Set by the analysis pipeline after parsing.
    pub location: Option<PayloadLocation>,
}

impl Chunk {
    /// Parses a discovered payload as a merge-tree chunk.
    ///
    /// Returns `None` when the payload has no `segmentTexts`, which is the
    /// normal outcome for payloads found by non-merge-tree anchors.
    pub fn from_payload(
        payload: &PayloadCandidate,
        member_file_offset: usize,
        raw: &[u8],
    ) -> Option<Self> {
        let summary: ChunkSummary = serde_json::from_value(payload.value.clone()).ok()?;
        let segment_values = summary.segment_texts?;
        let segments = segment_values.iter().map(Segment::from_value).collect();
        Some(Self {
            start_segment_index: summary.chunk_start_segment_index,
            segment_count: summary.chunk_segment_count,
            length_chars: summary.chunk_length_chars,
            total_length_chars: summary.total_length_chars,
            total_segment_count: summary.total_segment_count,
            sequence_number: summary.chunk_sequence_number,
            segments,
            source: SourceLocation {
                member_index: payload.member_index,
                member_file_offset,
                payload_offset: payload.start,
            },
            sha256: Sha256::digest(raw).into(),
            location: None,
        })
    }

    /// Identifies the tree this chunk belongs to. See [`GroupKey`].
    fn tree_key(&self) -> TreeKey {
        TreeKey {
            total_segment_count: self.total_segment_count,
            total_length_chars: self.total_length_chars,
            sequence_number: self.sequence_number,
        }
    }

    /// True when this chunk is the entire tree, needing no assembly.
    fn is_self_contained(&self) -> bool {
        if self.start_segment_index != 0 {
            return false;
        }
        match (self.segment_count, self.total_segment_count) {
            (Some(count), Some(total)) => count == total,
            (None, Some(total)) => self.segments.len() == total,
            _ => true,
        }
    }

    fn group_key(&self) -> GroupKey {
        if self.is_self_contained() {
            GroupKey::SelfContained(self.sha256)
        } else {
            GroupKey::Split(self.tree_key())
        }
    }
}

/// How chunks are grouped into trees.
///
/// The counters a chunk carries do *not* identify a tree on their own: in
/// one fixture built around an embedded table, dozens of one-segment cell trees share
/// identical `totalSegmentCount`, `totalLengthChars` and `chunkSequenceNumber`.
/// Grouping by counters alone reported 88 spurious conflicts.
///
/// So a chunk that already holds its whole tree is identified by its own content
/// hash, and only genuinely split trees are grouped by counters. In the corpus
/// only two fixtures contain split trees, one each.
///
/// Known limitation: two distinct components whose trees are byte-identical
/// (two empty cells, or two cells holding the same short label) collapse into one
/// tree. Distinguishing them needs the container framing, which is not decoded.
/// `copies_found` in the report makes the collapse visible.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    SelfContained([u8; 32]),
    Split(TreeKey),
}

/// Groups chunks into trees.
///
/// ASSUMPTION, corpus-supported but not proven: chunks sharing
/// `totalSegmentCount`, `totalLengthChars` and `chunkSequenceNumber` belong to
/// the same merge tree. In the corpus this separates every tree cleanly and
/// joins the two body chunks of the split tree. A collision would show up
/// as a `chunk_conflict` warning rather than as silently merged text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct TreeKey {
    pub total_segment_count: Option<usize>,
    pub total_length_chars: Option<usize>,
    pub sequence_number: Option<i64>,
}

/// What the segments of a tree reveal about its role and formatting.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TreeEvidence {
    pub text_segments: usize,
    pub marker_segments: usize,
    pub unknown_segments: usize,
    pub has_add_title_marker: bool,
    pub has_bold: bool,
    pub has_italic: bool,
    pub has_list_reference: bool,
    pub has_hyperlink: bool,
    pub has_attribution: bool,
    pub style_references: Vec<String>,
    pub node_types: Vec<String>,
    /// First 80 characters of concatenated text. Diagnostic only.
    pub text_preview: String,
}

/// What a tree is.
///
/// The first three are decided from the container framing and are reliable. The
/// `*Candidate` variants are the content heuristic, used only when the framing
/// did not decode; they are reported as candidates because a page title and a
/// table cell are structurally identical when the framing is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TreeRole {
    /// The page title, per the container framing.
    Title,
    /// The page body, per the container framing.
    Body,
    /// Content of a component embedded in the page: a table cell, a callout.
    EmbeddedComponent,
    /// Heuristic: the longest tree in the file.
    BodyCandidate,
    /// Heuristic: short, has text, carries no list structure.
    TitleCandidate,
    /// Everything else: empty placeholders, unclassifiable trees.
    Auxiliary,
}

impl TreeRole {
    /// True when the container framing decided this, not a heuristic.
    pub fn is_structural(self) -> bool {
        matches!(
            self,
            TreeRole::Title | TreeRole::Body | TreeRole::EmbeddedComponent
        )
    }

    /// True when the framing named the component, so the role is certain.
    ///
    /// `EmbeddedComponent` is deliberately excluded: it means only that the
    /// framing placed the tree somewhere this tool does not recognise, which is
    /// also what an older Loop version looks like. Such a tree stays eligible
    /// for the content heuristic when no body was found.
    pub fn is_confirmed(self) -> bool {
        matches!(self, TreeRole::Title | TreeRole::Body)
    }

    /// True when this tree holds the document body, however it was decided.
    pub fn is_body(self) -> bool {
        matches!(self, TreeRole::Body | TreeRole::BodyCandidate)
    }

    /// True when this tree may hold the document title, however it was decided.
    pub fn is_title(self) -> bool {
        matches!(self, TreeRole::Title | TreeRole::TitleCandidate)
    }
}

/// Why a tree was given its role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RoleEvidence {
    /// The component package that owns the channel, for example
    /// `LoopCanvasComponentSingleton`.
    ComponentPackage { package: String },
    /// A Fluid handle points at the channel, for example `CanvasComponentHandle`.
    FluidHandle { handle: String },
    /// The framing placed the tree, but under a component with no special
    /// meaning to this tool.
    ChannelPath { path: String },
    /// No framing was available; decided from the tree's own content.
    Heuristic { rule: &'static str },
}

#[derive(Debug, Clone)]
pub struct MergeTree {
    pub key: TreeKey,
    pub role: TreeRole,
    pub segments: Vec<Segment>,
    /// Every distinct chunk that contributed, in segment order.
    pub chunks: Vec<Chunk>,
    /// Locations of all copies found, including exact duplicates.
    pub sources: Vec<SourceLocation>,
    /// Copies collapsed as byte-identical.
    pub duplicate_copies: usize,
    /// True when the assembled segment count matches `totalSegmentCount`.
    pub complete: bool,
    /// SHA-256 over the ordered chunk hashes. Stable identity for the tree.
    pub content_hash: [u8; 32],
    pub evidence: TreeEvidence,
    /// Where the container framing places this tree, when it decoded.
    pub location: Option<PayloadLocation>,
    pub role_evidence: RoleEvidence,
}

impl MergeTree {
    pub fn text_len(&self) -> usize {
        self.key.total_length_chars.unwrap_or(0)
    }
}

/// Assembles chunks into deduplicated, ordered merge trees.
pub fn assemble(chunks: Vec<Chunk>, warnings: &mut Vec<Warning>) -> Vec<MergeTree> {
    let mut grouped: BTreeMap<GroupKey, Vec<Chunk>> = BTreeMap::new();
    for chunk in chunks {
        grouped.entry(chunk.group_key()).or_default().push(chunk);
    }

    let mut trees: Vec<MergeTree> = Vec::new();

    for (_group_key, mut group) in grouped {
        let key = group[0].tree_key();
        // Deterministic order before any deduplication decision is made.
        group.sort_by_key(|c| (c.start_segment_index, c.source));

        let mut kept: Vec<Chunk> = Vec::new();
        let mut sources: Vec<SourceLocation> = Vec::new();
        let mut duplicate_copies = 0usize;

        for chunk in group {
            sources.push(chunk.source);
            let existing_position = kept
                .iter()
                .position(|k| k.start_segment_index == chunk.start_segment_index);
            match existing_position.map(|position| (position, &kept[position])) {
                Some((position, existing)) if existing.sha256 == chunk.sha256 => {
                    duplicate_copies += 1;
                    // Copies are byte-identical, but only the one inside the
                    // aggregate member carries container framing, and that
                    // member is not always the one kept. Take the placement
                    // from whichever copy has it.
                    if kept[position].location.is_none() {
                        kept[position].location = chunk.location;
                    }
                }
                Some((_, existing)) => {
                    // Two chunks claim the same position but differ. Never merge
                    // them: keep the first by source order and report it.
                    duplicate_copies += 1;
                    warnings.push(Warning::new(
                        "chunk_conflict",
                        format!(
                            "two different chunks both claim segment index {} of the tree with \
                             sequence number {:?}; kept the copy from member {} at file offset {} \
                             and ignored the copy from member {} at file offset {}",
                            chunk.start_segment_index,
                            key.sequence_number,
                            existing.source.member_index,
                            existing.source.member_file_offset,
                            chunk.source.member_index,
                            chunk.source.member_file_offset,
                        ),
                    ));
                }
                None => kept.push(chunk),
            }
        }

        kept.sort_by_key(|c| c.start_segment_index);

        let mut segments: Vec<Segment> = Vec::new();
        let mut expected_next = 0usize;
        let mut contiguous = true;
        for chunk in &kept {
            if chunk.start_segment_index != expected_next {
                contiguous = false;
                warnings.push(Warning::new(
                    "chunk_gap",
                    format!(
                        "tree with sequence number {:?} jumps from segment {} to {}; the assembled \
                         text may be missing a run of segments",
                        key.sequence_number, expected_next, chunk.start_segment_index
                    ),
                ));
            }
            expected_next = chunk.start_segment_index + chunk.segments.len();
            segments.extend(chunk.segments.iter().cloned());
        }

        let complete = contiguous
            && key
                .total_segment_count
                .map(|t| t == segments.len())
                .unwrap_or(true);
        if !complete {
            warnings.push(Warning::new(
                "tree_incomplete",
                format!(
                    "tree with sequence number {:?} assembled {} of {:?} segments",
                    key.sequence_number,
                    segments.len(),
                    key.total_segment_count
                ),
            ));
        }

        let mut hasher = Sha256::new();
        for chunk in &kept {
            hasher.update(chunk.sha256);
        }
        let content_hash = hasher.finalize().into();
        let evidence = collect_evidence(&segments);

        let location = kept.iter().find_map(|chunk| chunk.location.clone());

        trees.push(MergeTree {
            key,
            role: TreeRole::Auxiliary,
            segments,
            chunks: kept,
            sources,
            duplicate_copies,
            complete,
            content_hash,
            evidence,
            location,
            role_evidence: RoleEvidence::Heuristic {
                rule: "not yet classified",
            },
        });
    }

    // Stable order: longest first, then by first source position.
    trees.sort_by(|a, b| {
        b.text_len()
            .cmp(&a.text_len())
            .then_with(|| a.sources.first().cmp(&b.sources.first()))
    });

    assign_roles(&mut trees);
    trees
}

/// Assigns roles, preferring the container framing over content heuristics.
///
/// Order, as observed to be reliable:
///
/// 1. a Fluid handle naming the channel;
/// 2. the component package that owns the channel;
/// 3. any other framing placement, which means an embedded component;
/// 4. failing all of those, the content heuristic, and only then are the roles
///    reported as candidates.
fn assign_roles(trees: &mut [MergeTree]) {
    for tree in trees.iter_mut() {
        if let Some((role, evidence)) = structural_role(tree.location.as_ref()) {
            tree.role = role;
            tree.role_evidence = evidence;
        }
    }

    // The framing may decode without naming a body, for example when a file is
    // too old to carry one. Fall back for whatever is still unclassified.
    if !trees.iter().any(|tree| tree.role == TreeRole::Body) {
        heuristic_roles(trees);
    }
}

/// Component package names observed to identify the page's own content.
const PACKAGE_TITLE: &str = "LoopPageTitleSingleton";
const PACKAGE_BODY: &str = "LoopCanvasComponentSingleton";

/// Fluid handle names observed on the page root.
const HANDLE_HEADER: &str = "HeaderComponentHandle";
const HANDLE_CANVAS: &str = "CanvasComponentHandle";

fn structural_role(location: Option<&PayloadLocation>) -> Option<(TreeRole, RoleEvidence)> {
    let location = location?;

    if let Some(handle) = &location.handle_name {
        match handle.as_str() {
            HANDLE_HEADER => {
                return Some((
                    TreeRole::Title,
                    RoleEvidence::FluidHandle {
                        handle: handle.clone(),
                    },
                ))
            }
            HANDLE_CANVAS => {
                return Some((
                    TreeRole::Body,
                    RoleEvidence::FluidHandle {
                        handle: handle.clone(),
                    },
                ))
            }
            _ => {}
        }
    }

    if let Some(package) = &location.component_package {
        let role = match package.as_str() {
            PACKAGE_TITLE => Some(TreeRole::Title),
            PACKAGE_BODY => Some(TreeRole::Body),
            _ => None,
        };
        if let Some(role) = role {
            return Some((
                role,
                RoleEvidence::ComponentPackage {
                    package: package.clone(),
                },
            ));
        }
    }

    // Placed by the framing, but under something with no special meaning here:
    // a table, a callout, a mention. Its text belongs to that component, not to
    // the page body.
    let path = location.blob_path.clone()?;
    Some((
        TreeRole::EmbeddedComponent,
        RoleEvidence::ChannelPath { path },
    ))
}

/// The content heuristic, used when the framing did not place the trees.
///
/// This cannot separate a page title from a table cell; both are short, plain
/// and structurally identical. Results are therefore reported as candidates.
fn heuristic_roles(trees: &mut [MergeTree]) {
    /// Trees at or below this many characters are short enough to be a title.
    /// Chosen from the corpus, where observed titles run 12 to 46 characters and
    /// the shortest body runs 273.
    const MAX_TITLE_CHARS: usize = 200;
    const BODY_RULE: &str = "the longest tree in the file that carries text";
    const TITLE_RULE: &str = "short, carries text, and has no list structure";

    let body_index = trees
        .iter()
        .enumerate()
        .filter(|(_, tree)| tree.evidence.text_segments > 0 && !tree.role.is_confirmed())
        .max_by_key(|(index, tree)| (tree.text_len(), std::cmp::Reverse(*index)))
        .map(|(index, _)| index);

    for (index, tree) in trees.iter_mut().enumerate() {
        if tree.role.is_confirmed() {
            continue;
        }
        if Some(index) == body_index {
            tree.role = TreeRole::BodyCandidate;
            tree.role_evidence = RoleEvidence::Heuristic { rule: BODY_RULE };
        } else if tree.evidence.text_segments > 0
            && tree.text_len() <= MAX_TITLE_CHARS
            && !tree.evidence.has_list_reference
        {
            tree.role = TreeRole::TitleCandidate;
            tree.role_evidence = RoleEvidence::Heuristic { rule: TITLE_RULE };
        }
    }
}

fn collect_evidence(segments: &[Segment]) -> TreeEvidence {
    let mut evidence = TreeEvidence::default();
    let mut preview = String::new();

    for segment in segments {
        match segment {
            Segment::Text { text, .. } => {
                evidence.text_segments += 1;
                if preview.chars().count() < 80 {
                    preview.push_str(text);
                }
            }
            Segment::Marker { .. } => evidence.marker_segments += 1,
            Segment::Unknown { .. } => evidence.unknown_segments += 1,
        }
        let Some(props) = segment.props() else {
            continue;
        };
        evidence.has_bold |= props.bold() == Some(true);
        evidence.has_italic |= props.italic() == Some(true);
        evidence.has_list_reference |= props.list_reference().is_some();
        evidence.has_hyperlink |= props.hyperlink_url().is_some();
        evidence.has_attribution |= props.attribution().is_some();
        evidence.has_add_title_marker |= props.is_add_title_placeholder();
        if let Some(style) = props.style_reference() {
            if !evidence.style_references.iter().any(|s| s == style) {
                evidence.style_references.push(style.to_string());
            }
        }
        if let Some(node_type) = props.node_type() {
            if !evidence.node_types.iter().any(|n| n == node_type) {
                evidence.node_types.push(node_type.to_string());
            }
        }
    }

    evidence.style_references.sort();
    evidence.node_types.sort();
    evidence.text_preview = preview
        .chars()
        .take(80)
        .collect::<String>()
        .replace('\n', " ");
    evidence
}
