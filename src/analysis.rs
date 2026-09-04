//! The shared pipeline: bytes -> gzip members -> embedded payloads -> merge
//! trees. Both `inspect` and `extract` run this, so the two commands can never
//! disagree about what a file contains.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::container::{self, FormatAssessment};
use crate::discovery::envelope::{self, EnvelopeIndex};
use crate::discovery::{fragment_scanner::FragmentScanner, PayloadCandidate, PayloadDiscovery};
use crate::fluid::merge_tree::{self, Chunk, MergeTree};
use crate::fluid::properties::{self, PropertyClass};
use crate::gzip::{self, FailedCandidate, GzipMember};
use crate::input::SourceFile;
use crate::operations::OperationLog;
use crate::warning::{Warning, Warnings};

#[derive(Debug, Clone, Serialize)]
pub struct MemberInfo {
    pub index: usize,
    pub file_offset: usize,
    pub compressed_len: Option<usize>,
    pub decompressed_len: usize,
    pub is_utf8: bool,
    pub truncated: bool,
    pub partial: bool,
    pub sha256: String,
    pub json_payloads: usize,
    pub merge_tree_chunks: usize,
    /// Whether the container framing decoded for this member, and why not.
    pub envelope: EnvelopeStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum EnvelopeStatus {
    /// Decoded, with this many blob paths recovered.
    Decoded { blob_paths: usize, channels: usize },
    /// Not an envelope, or a tag this reader does not know.
    Unreadable { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertyUsage {
    pub class: PropertyClass,
    pub occurrences: usize,
}

pub struct Analysis {
    pub path: String,
    pub extension: String,
    pub file_size: usize,
    pub members: Vec<GzipMember>,
    pub member_info: Vec<MemberInfo>,
    pub failed_candidates: Vec<FailedCandidate>,
    pub gzip_candidates: usize,
    pub payloads: Vec<PayloadCandidate>,
    pub payloads_by_anchor: BTreeMap<&'static str, usize>,
    pub chunk_candidates: usize,
    pub trees: Vec<MergeTree>,
    /// One index per member whose framing decoded.
    pub envelopes: Vec<EnvelopeIndex>,
    /// Operations found in the container, grouped by what they address.
    pub operations: OperationLog,
    pub assessment: FormatAssessment,
    pub properties: BTreeMap<String, PropertyUsage>,
    pub warnings: Vec<Warning>,
}

impl Analysis {
    pub fn run(source: &SourceFile, limits: gzip::Limits) -> Self {
        let mut warnings = Warnings::default();

        let scan = gzip::scan(&source.bytes, limits);
        warnings.extend(scan.warnings);
        for failure in &scan.failed {
            warnings.push(
                Warning::new(
                    "gzip_candidate_rejected",
                    format!("gzip signature did not inflate: {}", failure.error),
                )
                .at_member(usize::MAX, failure.file_offset),
            );
        }

        let scanner = FragmentScanner::default();
        let mut payloads: Vec<PayloadCandidate> = Vec::new();
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut member_info: Vec<MemberInfo> = Vec::new();
        let mut envelopes: Vec<EnvelopeIndex> = Vec::new();

        for member in &scan.members {
            // The framing is read first: it is what tells a page title from a
            // table cell. A member that does not decode costs nothing, because
            // the fragment scanner does not depend on it.
            let (envelope_index, envelope_status) = match envelope::read(&member.data) {
                Ok(index) => {
                    let status = EnvelopeStatus::Decoded {
                        blob_paths: index.blob_paths.len(),
                        channels: index.channel_packages.len(),
                    };
                    (Some(index), status)
                }
                Err(error) => (
                    None,
                    EnvelopeStatus::Unreadable {
                        reason: error.to_string(),
                    },
                ),
            };
            if let EnvelopeStatus::Unreadable { reason } = &envelope_status {
                // Only worth reporting for members that hold document content.
                if member.data.windows(9).any(|w| w == b"treeNodes") {
                    warnings.push(
                        Warning::new(
                            "envelope_unreadable",
                            format!(
                                "the container framing in this member did not decode ({reason}); \
                                 falling back to content heuristics for tree classification"
                            ),
                        )
                        .at_member(member.index, member.file_offset),
                    );
                }
            }

            let discovered = match scanner.discover(member.index, &member.data) {
                Ok(found) => found,
                Err(error) => {
                    warnings.push(
                        Warning::new("payload_discovery_failed", error.to_string())
                            .at_member(member.index, member.file_offset),
                    );
                    Vec::new()
                }
            };

            let mut member_chunks = 0usize;
            for payload in &discovered {
                let raw = &member.data[payload.start..payload.end];
                if let Some(mut chunk) = Chunk::from_payload(payload, member.file_offset, raw) {
                    chunk.location = envelope_index
                        .as_ref()
                        .and_then(|index| index.locate(member.index, payload.start));
                    member_chunks += 1;
                    chunks.push(chunk);
                }
            }
            if let Some(index) = &envelope_index {
                envelopes.push(index.clone());
            }

            member_info.push(MemberInfo {
                index: member.index,
                file_offset: member.file_offset,
                compressed_len: member.compressed_len,
                decompressed_len: member.data.len(),
                is_utf8: member.is_utf8(),
                truncated: member.truncated,
                partial: member.partial,
                sha256: hex(&member.sha256),
                json_payloads: discovered.len(),
                merge_tree_chunks: member_chunks,
                envelope: envelope_status,
            });
            payloads.extend(discovered);
        }

        let mut payloads_by_anchor: BTreeMap<&'static str, usize> = BTreeMap::new();
        for payload in &payloads {
            *payloads_by_anchor.entry(payload.anchor).or_default() += 1;
        }

        let chunk_candidates = chunks.len();
        let mut tree_warnings: Vec<Warning> = Vec::new();
        let trees = merge_tree::assemble(chunks, &mut tree_warnings);
        warnings.extend(tree_warnings);

        // Deltas can repeat across duplicated members; keep the first copy of
        // each so a replay never applies an operation twice.
        let mut seen_deltas: std::collections::BTreeSet<&str> = Default::default();
        let mut deltas: Vec<String> = Vec::new();
        for envelope in &envelopes {
            for delta in &envelope.deltas {
                if seen_deltas.insert(delta.as_str()) {
                    deltas.push(delta.clone());
                }
            }
        }
        let mut operations = OperationLog::parse(&deltas);
        operations.follows_snapshot = !envelopes.is_empty()
            && envelopes
                .iter()
                .all(EnvelopeIndex::operations_follow_snapshot);

        let components = container::components(&payloads);
        let assessment = container::assess(
            scan.members.len(),
            payloads.len(),
            chunk_candidates,
            components,
        );

        if !assessment.is_known_loop_component() {
            warnings.push(Warning::new(
                "component_identity_missing",
                format!(
                    "no recognised Loop component identifier found (expected one of: {})",
                    container::KNOWN_COMPONENTS.join(", ")
                ),
            ));
        }

        let properties = collect_properties(&trees, &mut warnings);

        Self {
            path: source.path.display().to_string(),
            extension: source.extension(),
            file_size: source.size(),
            members: scan.members,
            member_info,
            failed_candidates: scan.failed,
            gzip_candidates: scan.candidate_offsets.len(),
            payloads,
            payloads_by_anchor,
            chunk_candidates,
            trees,
            envelopes,
            operations,
            assessment,
            properties,
            warnings: warnings.into_vec(),
        }
    }

    /// The tree holding the document body, whether decided structurally or by
    /// heuristic.
    pub fn body_candidate(&self) -> Option<&MergeTree> {
        self.trees
            .iter()
            .find(|t| t.role == merge_tree::TreeRole::Body)
            .or_else(|| self.trees.iter().find(|t| t.role.is_body()))
    }

    pub fn title_candidates(&self) -> impl Iterator<Item = &MergeTree> {
        self.trees.iter().filter(|t| t.role.is_title())
    }

    /// True when the container framing placed at least one tree.
    pub fn has_structural_classification(&self) -> bool {
        self.trees.iter().any(|t| t.role.is_structural())
    }

    /// Chunk copies collapsed as byte-identical, across all trees.
    pub fn collapsed_duplicates(&self) -> usize {
        self.trees.iter().map(|t| t.duplicate_copies).sum()
    }
}

fn collect_properties(
    trees: &[MergeTree],
    warnings: &mut Warnings,
) -> BTreeMap<String, PropertyUsage> {
    let mut counts: BTreeMap<String, PropertyUsage> = BTreeMap::new();
    for tree in trees {
        for segment in &tree.segments {
            let Some(props) = segment.props() else {
                continue;
            };
            for key in props.keys() {
                let entry = counts.entry(key.clone()).or_insert(PropertyUsage {
                    class: properties::classify(key),
                    occurrences: 0,
                });
                entry.occurrences += 1;
            }
        }
    }
    let unknown: Vec<&String> = counts
        .iter()
        .filter(|(_, usage)| usage.class == PropertyClass::Unknown)
        .map(|(key, _)| key)
        .collect();
    if !unknown.is_empty() {
        warnings.push(Warning::new(
            "unknown_properties",
            format!(
                "{} property key(s) not seen before, preserved but not interpreted: {}",
                unknown.len(),
                unknown
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    counts
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
