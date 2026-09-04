//! `loop-extract inspect` - the diagnostic view of a container.
//!
//! Inspect exists to make format changes visible. When a future Loop version
//! stops extracting cleanly, this report should show where it diverged: which
//! members inflated, which payloads parsed, how many trees were found and what
//! properties were unrecognised.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::analysis::{hex, Analysis, MemberInfo, PropertyUsage};
use crate::container::FormatAssessment;
use crate::discovery::envelope::PayloadLocation;
use crate::document::model::ContentSource;
use crate::document::reconstruct::reconstruct;
use crate::fluid::merge_tree::{
    MergeTree, RoleEvidence, SourceLocation, TreeEvidence, TreeKey, TreeRole,
};
use crate::gzip::FailedCandidate;
use crate::operations::OperationCounts;
use crate::warning::Warning;

#[derive(Debug, Serialize)]
pub struct TreeReport {
    pub key: TreeKey,
    pub role: TreeRole,
    pub role_evidence: RoleEvidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<PayloadLocation>,
    pub content_hash: String,
    pub assembled_segments: usize,
    pub chunks: usize,
    pub complete: bool,
    pub copies_found: usize,
    pub duplicate_copies_collapsed: usize,
    pub sources: Vec<SourceLocation>,
    pub evidence: TreeEvidence,
}

#[derive(Debug, Serialize)]
pub struct OperationsReport {
    pub counts: OperationCounts,
    /// True when the log provably begins where the snapshot ends.
    pub follows_snapshot: bool,
    /// Insert counts per address, most first.
    pub inserts_by_address: Vec<(String, usize)>,
    pub snapshot_body_segments: usize,
    pub replayed_body_segments: usize,
    pub selected_content_source: ContentSource,
}

#[derive(Debug, Serialize)]
pub struct InspectReport {
    pub path: String,
    pub extension: String,
    pub file_size: usize,
    pub format: FormatAssessment,
    pub gzip_candidates: usize,
    pub gzip_members_inflated: usize,
    pub gzip_candidates_rejected: usize,
    pub decompressed_bytes: usize,
    pub members: Vec<MemberInfo>,
    pub rejected_candidates: Vec<FailedCandidate>,
    pub json_payloads: usize,
    pub payloads_by_anchor: BTreeMap<String, usize>,
    pub merge_tree_chunk_candidates: usize,
    pub merge_trees: usize,
    pub title_trees: usize,
    pub body_trees: usize,
    pub embedded_component_trees: usize,
    pub title_candidates: usize,
    pub body_candidates: usize,
    pub auxiliary_trees: usize,
    /// True when the container framing placed the trees, rather than heuristics.
    pub structurally_classified: bool,
    pub duplicate_copies_collapsed: usize,
    pub trees: Vec<TreeReport>,
    pub properties: BTreeMap<String, PropertyUsage>,
    pub operations: OperationsReport,
    pub warnings: Vec<Warning>,
}

impl InspectReport {
    pub fn build(analysis: &Analysis) -> Self {
        let trees: Vec<TreeReport> = analysis
            .trees
            .iter()
            .map(|tree| TreeReport {
                key: tree.key.clone(),
                role: tree.role,
                role_evidence: tree.role_evidence.clone(),
                location: tree.location.clone(),
                content_hash: hex(&tree.content_hash),
                assembled_segments: tree.segments.len(),
                chunks: tree.chunks.len(),
                complete: tree.complete,
                copies_found: tree.sources.len(),
                duplicate_copies_collapsed: tree.duplicate_copies,
                sources: tree.sources.clone(),
                evidence: tree.evidence.clone(),
            })
            .collect();

        let count_role = |role: TreeRole| analysis.trees.iter().filter(|t| t.role == role).count();

        Self {
            path: analysis.path.clone(),
            extension: analysis.extension.clone(),
            file_size: analysis.file_size,
            format: analysis.assessment.clone(),
            gzip_candidates: analysis.gzip_candidates,
            gzip_members_inflated: analysis.members.len(),
            gzip_candidates_rejected: analysis.failed_candidates.len(),
            decompressed_bytes: analysis.members.iter().map(|m| m.data.len()).sum(),
            members: analysis.member_info.clone(),
            rejected_candidates: analysis.failed_candidates.clone(),
            json_payloads: analysis.payloads.len(),
            payloads_by_anchor: analysis
                .payloads_by_anchor
                .iter()
                .map(|(k, v)| (k.trim_matches('"').to_string(), *v))
                .collect(),
            merge_tree_chunk_candidates: analysis.chunk_candidates,
            merge_trees: analysis.trees.len(),
            title_trees: count_role(TreeRole::Title),
            body_trees: count_role(TreeRole::Body),
            embedded_component_trees: count_role(TreeRole::EmbeddedComponent),
            title_candidates: count_role(TreeRole::TitleCandidate),
            body_candidates: count_role(TreeRole::BodyCandidate),
            auxiliary_trees: count_role(TreeRole::Auxiliary),
            structurally_classified: analysis.has_structural_classification(),
            duplicate_copies_collapsed: analysis.collapsed_duplicates(),
            trees,
            properties: analysis.properties.clone(),
            operations: operations_report(analysis),
            warnings: analysis.warnings.clone(),
        }
    }

    pub fn body_tree_segments(analysis: &Analysis) -> usize {
        analysis
            .body_candidate()
            .filter(|tree| tree.evidence.text_segments > 0)
            .map(|tree| tree.segments.len())
            .unwrap_or(0)
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("serialising the inspect report")
    }

    pub fn write_text(&self, out: &mut impl Write, verbose: bool) -> std::io::Result<()> {
        writeln!(out, "File:            {}", self.path)?;
        writeln!(out, "Size:            {} bytes", self.file_size)?;
        writeln!(out, "Format:          {}", self.format.format)?;
        if self.format.components.is_empty() {
            writeln!(out, "Component:       unknown")?;
        } else {
            for component in &self.format.components {
                writeln!(
                    out,
                    "Component:       {} (version {})",
                    component.name,
                    component.version.as_deref().unwrap_or("unknown")
                )?;
            }
        }
        writeln!(out, "Confidence:      {:?}", self.format.confidence)?;
        writeln!(out)?;

        writeln!(out, "Gzip members")?;
        writeln!(out, "  signatures found:      {}", self.gzip_candidates)?;
        writeln!(
            out,
            "  inflated:              {}",
            self.gzip_members_inflated
        )?;
        writeln!(
            out,
            "  rejected:              {}",
            self.gzip_candidates_rejected
        )?;
        writeln!(out, "  decompressed bytes:    {}", self.decompressed_bytes)?;
        writeln!(out)?;

        writeln!(out, "Embedded payloads")?;
        writeln!(out, "  JSON payloads:         {}", self.json_payloads)?;
        for (anchor, count) in &self.payloads_by_anchor {
            writeln!(out, "    via {anchor:<24} {count}")?;
        }
        writeln!(
            out,
            "  merge-tree chunks:     {}",
            self.merge_tree_chunk_candidates
        )?;
        writeln!(out, "  merge trees assembled: {}", self.merge_trees)?;
        writeln!(
            out,
            "  classified by:         {}",
            if self.structurally_classified {
                "container framing"
            } else {
                "content heuristics"
            }
        )?;
        if self.structurally_classified {
            writeln!(out, "    title:               {}", self.title_trees)?;
            writeln!(out, "    body:                {}", self.body_trees)?;
            writeln!(
                out,
                "    embedded components: {}",
                self.embedded_component_trees
            )?;
        }
        if self.body_candidates + self.title_candidates > 0 {
            writeln!(out, "    body candidates:     {}", self.body_candidates)?;
            writeln!(out, "    title candidates:    {}", self.title_candidates)?;
        }
        writeln!(out, "    auxiliary:           {}", self.auxiliary_trees)?;
        writeln!(
            out,
            "  duplicate copies collapsed: {}",
            self.duplicate_copies_collapsed
        )?;
        writeln!(out)?;

        writeln!(out, "Merge trees")?;
        for tree in &self.trees {
            writeln!(
                out,
                "  [{:?}] seq {:?}  segments {}/{:?}  chars {:?}  chunks {}  copies {}{}",
                tree.role,
                tree.key.sequence_number,
                tree.assembled_segments,
                tree.key.total_segment_count,
                tree.key.total_length_chars,
                tree.chunks,
                tree.copies_found,
                if tree.complete { "" } else { "  INCOMPLETE" }
            )?;
            writeln!(
                out,
                "      text {}  markers {}  unknown {}  bold {}  italic {}  lists {}  links {}",
                tree.evidence.text_segments,
                tree.evidence.marker_segments,
                tree.evidence.unknown_segments,
                tree.evidence.has_bold,
                tree.evidence.has_italic,
                tree.evidence.has_list_reference,
                tree.evidence.has_hyperlink,
            )?;
            if !tree.evidence.style_references.is_empty() {
                writeln!(
                    out,
                    "      styles: {}",
                    tree.evidence.style_references.join(", ")
                )?;
            }
            if !tree.evidence.node_types.is_empty() {
                writeln!(
                    out,
                    "      node types: {}",
                    tree.evidence.node_types.join(", ")
                )?;
            }
            if let Some(location) = &tree.location {
                if let Some(channel) = &location.channel_id {
                    write!(out, "      channel: {channel}")?;
                    if let Some(package) = &location.component_package {
                        write!(out, "  component: {package}")?;
                    }
                    if let Some(handle) = &location.handle_name {
                        write!(out, "  handle: {handle}")?;
                    }
                    writeln!(out)?;
                }
                if let Some(path) = &location.blob_path {
                    writeln!(out, "      blob path: {path}")?;
                }
            }
            if !tree.evidence.text_preview.is_empty() {
                writeln!(out, "      preview: {:?}", tree.evidence.text_preview)?;
            }
            if verbose {
                for source in &tree.sources {
                    writeln!(
                        out,
                        "      source: member {} at file offset {}, payload offset {}",
                        source.member_index, source.member_file_offset, source.payload_offset
                    )?;
                }
            }
        }
        writeln!(out)?;

        writeln!(out, "Properties encountered")?;
        for (key, usage) in &self.properties {
            writeln!(
                out,
                "  {:<40} {:?} x{}",
                key, usage.class, usage.occurrences
            )?;
        }
        writeln!(out)?;

        if verbose {
            writeln!(out, "Members")?;
            for member in &self.members {
                writeln!(
                    out,
                    "  {:>3}  offset {:>9}  compressed {:>9}  decompressed {:>9}  utf8 {:<5}  json {:>3}  chunks {:>3}",
                    member.index,
                    member.file_offset,
                    member
                        .compressed_len
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "?".to_string()),
                    member.decompressed_len,
                    member.is_utf8,
                    member.json_payloads,
                    member.merge_tree_chunks,
                )?;
            }
            writeln!(out)?;
        }

        writeln!(out, "Operations")?;
        writeln!(
            out,
            "  records:               {}",
            self.operations.counts.records
        )?;
        writeln!(
            out,
            "    inserts:             {}",
            self.operations.counts.inserts
        )?;
        writeln!(
            out,
            "    removes:             {}",
            self.operations.counts.removes
        )?;
        writeln!(
            out,
            "    annotates:           {}",
            self.operations.counts.annotates
        )?;
        writeln!(
            out,
            "  begin after snapshot:  {}",
            if self.operations.follows_snapshot {
                "yes"
            } else {
                "not established"
            }
        )?;
        for (address, inserts) in &self.operations.inserts_by_address {
            writeln!(out, "    {address}: {inserts} insert(s)")?;
        }
        writeln!(
            out,
            "  snapshot body segments: {}",
            self.operations.snapshot_body_segments
        )?;
        writeln!(
            out,
            "  replayed body segments: {}",
            self.operations.replayed_body_segments
        )?;
        writeln!(
            out,
            "  selected content source: {:?}",
            self.operations.selected_content_source
        )?;
        writeln!(out)?;

        writeln!(out, "Warnings ({})", self.warnings.len())?;
        for warning in &self.warnings {
            match warning.file_offset {
                Some(offset) => writeln!(
                    out,
                    "  [{}] at file offset {}: {}",
                    warning.code, offset, warning.message
                )?,
                None => writeln!(out, "  [{}] {}", warning.code, warning.message)?,
            }
        }
        Ok(())
    }
}

/// Writes every inflated member and every JSON payload to `dir`.
///
/// Names are deterministic so a diff between two runs, or two Loop versions, is
/// meaningful.
pub fn dump_payloads(analysis: &Analysis, dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating dump directory {}", dir.display()))?;

    let mut written = 0usize;
    for member in &analysis.members {
        let raw_path = dir.join(format!("member-{:03}.raw", member.index));
        std::fs::write(&raw_path, &member.data)
            .with_context(|| format!("writing {}", raw_path.display()))?;
        written += 1;
    }

    let mut per_member: BTreeMap<usize, usize> = BTreeMap::new();
    for payload in &analysis.payloads {
        let ordinal = per_member.entry(payload.member_index).or_default();
        let path = dir.join(format!(
            "member-{:03}-fragment-{:03}.json",
            payload.member_index, ordinal
        ));
        *ordinal += 1;
        let text = serde_json::to_string_pretty(&payload.value)
            .context("serialising a discovered payload")?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        written += 1;
    }
    Ok(written)
}

/// Helper for `MergeTree` consumers that want the assembled plain text.
pub fn tree_plain_text(tree: &MergeTree) -> String {
    tree.segments.iter().filter_map(|s| s.text()).collect()
}

/// Summarises the operation log and which source the document would use.
fn operations_report(analysis: &Analysis) -> OperationsReport {
    let (document, _) = reconstruct(analysis);
    let replayed = document
        .metadata
        .replay
        .as_ref()
        .map(|replay| replay.segments)
        .unwrap_or(0);

    OperationsReport {
        counts: analysis.operations.counts.clone(),
        follows_snapshot: analysis.operations.follows_snapshot,
        inserts_by_address: analysis.operations.addresses_by_inserts(),
        snapshot_body_segments: InspectReport::body_tree_segments(analysis),
        replayed_body_segments: replayed,
        selected_content_source: document.metadata.content_source,
    }
}
