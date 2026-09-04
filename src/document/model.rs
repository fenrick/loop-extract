//! The format-independent document model.
//!
//! Reconstruction produces this; renderers consume it. Nothing here knows about
//! Markdown, gzip or Fluid framing, so a new output format needs no parser
//! change and a format discovery needs no renderer change.

use serde::Serialize;

use crate::fluid::properties::Properties;

/// Where the reconstructed content came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContentSource {
    /// The current snapshot held a usable body.
    Snapshot,
    /// The snapshot body was empty; the body was replayed from operations.
    Operations,
    /// Snapshot content advanced by later operations.
    ///
    /// Not produced yet. Combining the two needs proof that the operation
    /// sequence begins after the exact snapshot being used, so that already
    /// materialised segments are not inserted twice.
    SnapshotPlusOperations,
}

/// How the document title was decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum TitleSelection {
    /// One candidate stood out under a documented rule.
    Resolved { title: String, rule: &'static str },
    /// Several candidates remain. One was chosen deterministically, and the
    /// rest are listed so the choice can be checked.
    Ambiguous {
        chosen: Option<String>,
        candidates: Vec<String>,
        reason: &'static str,
    },
    /// No tree looked like a title.
    None,
}

#[derive(Debug, Clone, Serialize)]
pub struct Metadata {
    pub source_path: String,
    pub file_size: usize,
    pub format: String,
    pub components: Vec<crate::container::ComponentIdentity>,
    pub content_source: ContentSource,
    pub title_selection: TitleSelection,
    /// SHA-256 of the tree the body was reconstructed from, when there was one.
    pub body_tree_hash: Option<String>,
    pub body_segment_count: usize,
    pub merge_trees: usize,
    /// Present when the body was rebuilt from operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<crate::operations::ReplaySummary>,
}

/// One run of inline content.
///
/// Nesting order, innermost first: text, italic, bold, link. So bold italic
/// linked text is `Link { text: [Bold([Italic([Text]))] }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Inline {
    Text {
        text: String,
    },
    Bold {
        content: Vec<Inline>,
    },
    Italic {
        content: Vec<Inline>,
    },
    Link {
        content: Vec<Inline>,
        target: String,
    },
}

impl Inline {
    pub fn text(value: impl Into<String>) -> Self {
        Inline::Text { text: value.into() }
    }

    /// Concatenated plain text, formatting discarded.
    pub fn plain(&self) -> String {
        match self {
            Inline::Text { text } => text.clone(),
            Inline::Bold { content } | Inline::Italic { content } => plain_of(content),
            Inline::Link { content, .. } => plain_of(content),
        }
    }
}

pub fn plain_of(inlines: &[Inline]) -> String {
    inlines.iter().map(Inline::plain).collect()
}

/// A block of document content.
///
/// Observed in test fixtures: a merge tree is a flat run of text segments
/// punctuated by markers. A marker *terminates* the run before it and carries
/// that block's properties, in the way a paragraph mark does in a word
/// processor. List nesting is therefore a property of the block, not a tree
/// structure, and is kept flat here with an explicit depth. Renderers that need
/// nesting build it from the depth.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Block {
    Paragraph {
        content: Vec<Inline>,
        #[serde(skip_serializing_if = "Properties::is_empty")]
        props: Properties,
    },
    Heading {
        level: u8,
        content: Vec<Inline>,
        #[serde(skip_serializing_if = "Properties::is_empty")]
        props: Properties,
    },
    ListItem {
        depth: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        list_id: Option<String>,
        content: Vec<Inline>,
        #[serde(skip_serializing_if = "Properties::is_empty")]
        props: Properties,
    },
    /// A marker for something embedded rather than typed: a table of contents,
    /// an image, a nested Fluid component. Its content lives in another tree.
    Embedded {
        node_type: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        content: Vec<Inline>,
        #[serde(skip_serializing_if = "Properties::is_empty")]
        props: Properties,
    },
    /// A segment matching no known shape, preserved rather than discarded.
    Unknown { raw: serde_json::Value },
}

impl Block {
    pub fn content(&self) -> &[Inline] {
        match self {
            Block::Paragraph { content, .. }
            | Block::Heading { content, .. }
            | Block::ListItem { content, .. }
            | Block::Embedded { content, .. } => content,
            Block::Unknown { .. } => &[],
        }
    }

    pub fn props(&self) -> Option<&Properties> {
        match self {
            Block::Paragraph { props, .. }
            | Block::Heading { props, .. }
            | Block::ListItem { props, .. }
            | Block::Embedded { props, .. } => Some(props),
            Block::Unknown { .. } => None,
        }
    }

    /// True when the block carries no text. Renderers skip these; the model
    /// keeps them because an empty paragraph is real document structure.
    pub fn is_empty(&self) -> bool {
        plain_of(self.content()).is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Document {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub blocks: Vec<Block>,
    pub metadata: Metadata,
}

impl Document {
    /// Blocks that carry text, in order.
    pub fn visible_blocks(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter().filter(|b| !b.is_empty())
    }
}
