//! Turning merge-tree segments into document blocks.
//!
//! Observed in test fixtures, confirmed across every one that carries body
//! text, including two sets of meeting notes and a page built largely from
//! embedded components:
//!
//! A marker terminates the text run before it and carries that block's
//! properties. The very first segments of a body are often markers with nothing
//! before them, which is how an empty leading paragraph or a table-of-contents
//! widget is represented.
//!
//! ```text
//! TEXT  "Lead-in heading: "             {format!bold: true}
//! TEXT  "the sentence that follows"    {}
//! MARK                                  {nodeType: Paragraph,
//!                                        list!reference: "0;list!list-...030"}
//! ```
//!
//! becomes one list item at depth 0 whose content is bold text followed by
//! plain text.
//!
//! This is reverse-engineered behaviour and is not based on a published
//! Microsoft Prague/Fluid file-format specification.

use crate::analysis::{hex, Analysis};
use crate::fluid::merge_tree::{MergeTree, RoleEvidence, Segment, TreeRole};
use crate::fluid::properties::Properties;
use crate::operations::{segments_text, ReplaySummary};
use crate::warning::Warning;

use super::model::{Block, ContentSource, Document, Inline, Metadata, TitleSelection};

/// Rule name recorded when the title is resolved by the bare-tree test.
const RULE_STRUCTURAL_PACKAGE: &str =
    "the container framing places this tree under the page title component";
const RULE_STRUCTURAL_HANDLE: &str =
    "the container framing points the page header handle at this tree's channel";
const RULE_BARE_TREE: &str =
    "the only title candidate whose text carries no formatting and whose closing marker \
     carries only markerId and nodeType";
const RULE_SOLE_CANDIDATE: &str = "the only title candidate in the file";

/// Reconstructs a document from an analysed container.
pub fn reconstruct(analysis: &Analysis) -> (Document, Vec<Warning>) {
    let mut warnings: Vec<Warning> = Vec::new();

    let body = analysis.body_candidate();
    let snapshot_has_text = body
        .map(|tree| tree.evidence.text_segments > 0)
        .unwrap_or(false);

    // Extraction precedence: the snapshot when it holds a usable body,
    // otherwise the operation log. The two are never combined, because
    // combining them safely needs proof that the operations begin after this
    // exact snapshot; without that, already materialised segments would be
    // inserted twice.
    let (segments, content_source, replay) = if snapshot_has_text {
        (
            body.map(|t| t.segments.clone()).unwrap_or_default(),
            ContentSource::Snapshot,
            None,
        )
    } else if let Some((summary, segments)) = replay_body(analysis, &mut warnings) {
        let source = if summary.seeded_from_snapshot {
            ContentSource::SnapshotPlusOperations
        } else {
            ContentSource::Operations
        };
        (segments, source, Some(summary))
    } else {
        if body.is_none() {
            warnings.push(Warning::new(
                "no_body_tree",
                "no merge tree in the snapshot carries text, and no operations rebuilt one",
            ));
        }
        (
            body.map(|t| t.segments.clone()).unwrap_or_default(),
            ContentSource::Snapshot,
            None,
        )
    };

    let blocks = blocks_from(&segments, &mut warnings);

    let title_selection = select_title(analysis, &mut warnings);
    let title = match &title_selection {
        TitleSelection::Resolved { title, .. } => Some(title.clone()),
        TitleSelection::Ambiguous { chosen, .. } => chosen.clone(),
        TitleSelection::None => None,
    };

    let metadata = Metadata {
        source_path: analysis.path.clone(),
        file_size: analysis.file_size,
        format: analysis.assessment.format.clone(),
        components: analysis.assessment.components.clone(),
        content_source,
        title_selection,
        body_tree_hash: body.map(|t| hex(&t.content_hash)),
        body_segment_count: segments.len(),
        merge_trees: analysis.trees.len(),
        replay,
    };

    (
        Document {
            title,
            blocks,
            metadata,
        },
        warnings,
    )
}

/// Splits a run of segments into blocks at marker boundaries.
pub fn blocks_from(segments: &[Segment], warnings: &mut Vec<Warning>) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut run: Vec<(&str, &Properties)> = Vec::new();

    for segment in segments {
        match segment {
            Segment::Text { text, props } => run.push((text.as_str(), props)),
            Segment::Marker { props, .. } => {
                blocks.push(block_from(std::mem::take(&mut run), props));
            }
            Segment::Unknown { raw } => {
                warnings.push(Warning::new(
                    "unknown_segment",
                    "a segment matched no known shape and was preserved unparsed",
                ));
                blocks.push(Block::Unknown { raw: raw.clone() });
            }
        }
    }

    // Text after the final marker still forms a block.
    if !run.is_empty() {
        blocks.push(block_from(run, &Properties::default()));
    }

    blocks
}

/// Builds one block from a finished text run and the properties of the marker
/// that closed it.
fn block_from(run: Vec<(&str, &Properties)>, props: &Properties) -> Block {
    let content = inlines_from(&run);
    let props = props.clone();

    if let Some(level) = heading_level(&props) {
        return Block::Heading {
            level,
            content,
            props,
        };
    }
    if let Some(list) = props.list_reference() {
        return Block::ListItem {
            depth: list.depth.unwrap_or(0),
            list_id: list.list_id,
            content,
            props,
        };
    }
    match props.node_type() {
        // `Paragraph` is the ordinary case. Anything else marks embedded
        // content whose body lives elsewhere.
        Some(node_type) if node_type != "Paragraph" => Block::Embedded {
            node_type: node_type.to_string(),
            content,
            props,
        },
        _ => Block::Paragraph { content, props },
    }
}

/// Reads `style!reference` as a heading level.
///
/// Observed values: `Heading 1`, `Heading 2`, `Body`. Only the `Heading N` form
/// produces a heading; anything else leaves the block a paragraph.
fn heading_level(props: &Properties) -> Option<u8> {
    let style = props.style_reference()?;
    let rest = style.strip_prefix("Heading ")?;
    rest.trim()
        .parse::<u8>()
        .ok()
        .filter(|level| (1..=6).contains(level))
}

/// Groups consecutive segments that share formatting into inline runs.
fn inlines_from(run: &[(&str, &Properties)]) -> Vec<Inline> {
    let mut inlines: Vec<Inline> = Vec::new();
    let mut index = 0usize;

    while index < run.len() {
        let style = Style::of(run[index].1);
        let mut text = String::new();
        while index < run.len() && Style::of(run[index].1) == style {
            text.push_str(run[index].0);
            index += 1;
        }
        if text.is_empty() {
            continue;
        }
        inlines.push(style.wrap(Inline::text(text)));
    }

    inlines
}

/// The formatting that distinguishes one inline run from the next.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    link: Option<String>,
}

impl Style {
    fn of(props: &Properties) -> Self {
        Self {
            bold: props.bold() == Some(true),
            italic: props.italic() == Some(true),
            link: props.hyperlink_url().map(str::to_string),
        }
    }

    fn wrap(&self, inner: Inline) -> Inline {
        let mut current = inner;
        if self.italic {
            current = Inline::Italic {
                content: vec![current],
            };
        }
        if self.bold {
            current = Inline::Bold {
                content: vec![current],
            };
        }
        if let Some(target) = &self.link {
            current = Inline::Link {
                content: vec![current],
                target: target.clone(),
            };
        }
        current
    }
}

/// Chooses the document title from the title candidates.
///
/// Observed in test fixtures: the page title tree is *bare*. Its text segments
/// carry no formatting or locale properties, and its closing marker carries only
/// `markerId` and `nodeType`. A heading inside an embedded card, by contrast,
/// carries `format!bold` and `content!locale`, and its marker carries
/// `paragraph!textAlignment` and `paragraph!writingMode`. That test separates
/// the title in every fixture that stores its title in its own tree.
///
/// It does *not* separate a title from a table cell, which is bare too. In a
/// document with embedded components the result is reported as ambiguous rather
/// than guessed. Recovering the Fluid channel path for each tree resolves it
/// properly; see `discovery::envelope`.
fn select_title(analysis: &Analysis, warnings: &mut Vec<Warning>) -> TitleSelection {
    // The container framing is authoritative when it decoded.
    if let Some(tree) = analysis
        .trees
        .iter()
        .find(|tree| tree.role == TreeRole::Title)
    {
        let rule = match &tree.role_evidence {
            RoleEvidence::FluidHandle { .. } => RULE_STRUCTURAL_HANDLE,
            _ => RULE_STRUCTURAL_PACKAGE,
        };
        return TitleSelection::Resolved {
            title: tree_text(tree),
            rule,
        };
    }

    let candidates: Vec<&MergeTree> = analysis
        .trees
        .iter()
        .filter(|tree| tree.role == TreeRole::TitleCandidate)
        .collect();

    if candidates.is_empty() {
        return TitleSelection::None;
    }
    if candidates.len() == 1 {
        return TitleSelection::Resolved {
            title: tree_text(candidates[0]),
            rule: RULE_SOLE_CANDIDATE,
        };
    }

    let bare: Vec<&MergeTree> = candidates.iter().copied().filter(|t| is_bare(t)).collect();
    if bare.len() == 1 {
        return TitleSelection::Resolved {
            title: tree_text(bare[0]),
            rule: RULE_BARE_TREE,
        };
    }

    // Still ambiguous. Choose deterministically by content hash so the output
    // never depends on file order, and say so.
    let pool = if bare.is_empty() { candidates } else { bare };
    let mut ordered: Vec<&MergeTree> = pool.clone();
    ordered.sort_by_key(|t| t.content_hash);
    let chosen = ordered.first().map(|t| tree_text(t));

    warnings.push(Warning::new(
        "title_ambiguous",
        format!(
            "{} trees could be the document title; chose {:?} by content hash. Structural \
             classification from the container framing is needed to decide this properly.",
            pool.len(),
            chosen.as_deref().unwrap_or("")
        ),
    ));

    TitleSelection::Ambiguous {
        chosen,
        candidates: pool.iter().map(|t| tree_text(t)).collect(),
        reason: "several trees are structurally indistinguishable from the title",
    }
}

/// True when nothing about the tree's properties suggests it is anything but a
/// plain title. See [`select_title`].
fn is_bare(tree: &MergeTree) -> bool {
    for segment in &tree.segments {
        let Some(props) = segment.props() else {
            return false;
        };
        match segment {
            Segment::Text { .. } => {
                if props.keys().any(|key| key != "attribution") {
                    return false;
                }
            }
            Segment::Marker { .. } => {
                if props
                    .keys()
                    .any(|key| !matches!(key.as_str(), "markerId" | "nodeType" | "attribution"))
                {
                    return false;
                }
            }
            Segment::Unknown { .. } => return false,
        }
    }
    true
}

fn tree_text(tree: &MergeTree) -> String {
    tree.segments
        .iter()
        .filter_map(Segment::text)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Rebuilds the body from the operation log when the snapshot has none.
///
/// Chooses which address to replay by asking the container framing which
/// channel is the page canvas. Failing that, the address with the most inserts
/// wins, which is deterministic and reported.
fn replay_body(
    analysis: &Analysis,
    warnings: &mut Vec<Warning>,
) -> Option<(ReplaySummary, Vec<Segment>)> {
    let log = &analysis.operations;
    if log.is_empty() {
        return None;
    }

    let by_inserts = log.addresses_by_inserts();
    if by_inserts.is_empty() {
        return None;
    }

    let package_of = |address: &str| -> Option<String> {
        let channel = log.channel_of(address)?;
        analysis
            .envelopes
            .iter()
            .find_map(|envelope| envelope.channel_packages.get(&channel).cloned())
    };

    let chosen = by_inserts
        .iter()
        .find(|(address, _)| package_of(address).as_deref() == Some(CANVAS_PACKAGE))
        .or_else(|| by_inserts.first())
        .map(|(address, _)| address.clone())?;

    if package_of(&chosen).as_deref() != Some(CANVAS_PACKAGE) {
        warnings.push(Warning::new(
            "operation_target_unconfirmed",
            format!(
                "the container framing does not confirm which channel is the page body; replayed \
                 the address with the most inserts ({chosen})"
            ),
        ));
    }
    if by_inserts.len() > 1 {
        warnings.push(Warning::new(
            "operations_span_several_channels",
            format!(
                "operations target {} channels; only {chosen} was replayed",
                by_inserts.len()
            ),
        ));
    }

    // Seed from the snapshot when the log provably starts where it ends. The
    // snapshot for an operation-backed page is usually a few empty markers, and
    // the first operations insert at positions that only make sense on top of
    // them.
    let channel = log.channel_of(&chosen);
    let base: Vec<Segment> = if log.follows_snapshot {
        channel
            .as_ref()
            .and_then(|channel| {
                analysis.trees.iter().find(|tree| {
                    tree.location
                        .as_ref()
                        .and_then(|location| location.channel_id.as_deref())
                        == Some(channel.as_str())
                })
            })
            .map(|tree| tree.segments.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let seeded = !base.is_empty();

    let result = log.replay_address(&chosen, base);
    warnings.extend(result.warnings.iter().cloned());

    if segments_text(&result.segments).trim().is_empty() {
        return None;
    }

    warnings.push(Warning::new(
        "body_rebuilt_from_operations",
        format!(
            "the snapshot holds no body text; rebuilt {} segment(s) from {} operation(s){}",
            result.segments.len(),
            result.applied,
            if seeded {
                " applied on top of the snapshot"
            } else {
                ""
            }
        ),
    ));

    let summary = ReplaySummary {
        address: chosen.clone(),
        seeded_from_snapshot: seeded,
        channel,
        component_package: package_of(&chosen),
        operations_applied: result.applied,
        operations_skipped: result.skipped,
        segments: result.segments.len(),
    };
    Some((summary, result.segments))
}

/// The component that owns a Loop page canvas.
const CANVAS_PACKAGE: &str = "LoopCanvasComponentSingleton";
