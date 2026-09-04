//! JSON rendering: the parsed model plus the diagnostics needed to check it.
//!
//! Personal data is removed unless it was explicitly asked for. The parser
//! always keeps attribution; filtering happens here, in the output layer, so
//! later analysis stays possible without exposing names and email addresses in
//! an ordinary export.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::analysis::{hex, Analysis};
use crate::document::model::{Block, Document, Metadata};
use crate::fluid::merge_tree::TreeRole;
use crate::fluid::properties::{is_personal_data, Properties};
use crate::warning::Warning;

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Keep author names, email addresses, object ids and timestamps.
    pub include_attribution: bool,
}

#[derive(Debug, Serialize)]
struct TreeSummary {
    role: TreeRole,
    content_hash: String,
    segments: usize,
    total_length_chars: Option<usize>,
    sequence_number: Option<i64>,
    complete: bool,
    copies_found: usize,
    text_preview: String,
}

#[derive(Debug, Serialize)]
struct Source {
    gzip_members: usize,
    gzip_candidates_rejected: usize,
    json_payloads: usize,
    merge_tree_chunks: usize,
    trees: Vec<TreeSummary>,
    /// Every property key seen, with how well it is understood.
    properties: std::collections::BTreeMap<String, crate::analysis::PropertyUsage>,
}

#[derive(Debug, Serialize)]
struct Output<'a> {
    metadata: &'a Metadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a String>,
    blocks: &'a [Block],
    source: Source,
    warnings: Vec<&'a Warning>,
}

pub fn render(
    document: &Document,
    analysis: &Analysis,
    extra_warnings: &[Warning],
    options: Options,
) -> Result<String> {
    let redacted;
    let document = if options.include_attribution {
        document
    } else {
        redacted = redact(document);
        &redacted
    };

    let output = Output {
        metadata: &document.metadata,
        title: document.title.as_ref(),
        blocks: &document.blocks,
        source: Source {
            gzip_members: analysis.members.len(),
            gzip_candidates_rejected: analysis.failed_candidates.len(),
            json_payloads: analysis.payloads.len(),
            merge_tree_chunks: analysis.chunk_candidates,
            trees: analysis
                .trees
                .iter()
                .map(|tree| TreeSummary {
                    role: tree.role,
                    content_hash: hex(&tree.content_hash),
                    segments: tree.segments.len(),
                    total_length_chars: tree.key.total_length_chars,
                    sequence_number: tree.key.sequence_number,
                    complete: tree.complete,
                    copies_found: tree.sources.len(),
                    text_preview: tree.evidence.text_preview.clone(),
                })
                .collect(),
            properties: analysis.properties.clone(),
        },
        warnings: analysis.warnings.iter().chain(extra_warnings).collect(),
    };

    serde_json::to_string_pretty(&output).context("serialising the document as JSON")
}

/// Removes personal data from every property map in the document.
fn redact(document: &Document) -> Document {
    let mut copy = document.clone();
    for block in &mut copy.blocks {
        match block {
            Block::Paragraph { props, .. }
            | Block::Heading { props, .. }
            | Block::ListItem { props, .. }
            | Block::Embedded { props, .. } => redact_props(props),
            Block::Unknown { raw } => redact_value(raw),
        }
    }
    copy
}

fn redact_props(props: &mut Properties) {
    props.0.retain(|key, _| !is_personal_data(key));
}

fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|key, _| !is_personal_data(key));
            for nested in map.values_mut() {
                redact_value(nested);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        _ => {}
    }
}
