//! Markdown rendering.
//!
//! The goal is the document's logical structure, not a byte-identical copy of
//! Microsoft's own Markdown export.

use crate::document::model::{Block, Document, Inline};

/// Spaces per nesting level. Four keeps nested bullets unambiguous in every
/// Markdown flavour.
const INDENT: &str = "    ";

pub fn render(document: &Document) -> String {
    let mut groups: Vec<String> = Vec::new();

    if let Some(title) = &document.title {
        if !title.is_empty() {
            groups.push(format!("# {}", escape(title)));
        }
    }

    // Consecutive list items form one block of lines; everything else stands
    // alone, separated by a blank line.
    let mut list_lines: Vec<String> = Vec::new();
    let mut list_id: Option<String> = None;

    for block in &document.blocks {
        match block {
            Block::ListItem {
                depth,
                list_id: id,
                content,
                ..
            } => {
                if block.is_empty() {
                    continue;
                }
                // A different list id starts a new list.
                if !list_lines.is_empty() && id != &list_id {
                    groups.push(list_lines.join("\n"));
                    list_lines.clear();
                }
                list_id = id.clone();
                list_lines.push(format!(
                    "{}- {}",
                    INDENT.repeat(*depth as usize),
                    inlines(content)
                ));
            }
            other => {
                if !list_lines.is_empty() {
                    groups.push(list_lines.join("\n"));
                    list_lines.clear();
                    list_id = None;
                }
                if other.is_empty() {
                    continue;
                }
                match other {
                    Block::Heading { level, content, .. } => {
                        let level = heading_level(document, *level);
                        groups.push(format!(
                            "{} {}",
                            "#".repeat(level as usize),
                            inlines(content)
                        ));
                    }
                    Block::Paragraph { content, .. } | Block::Embedded { content, .. } => {
                        groups.push(inlines(content));
                    }
                    // Embedded markers without text and unparsed segments carry
                    // no renderable content. They stay in the JSON output.
                    Block::ListItem { .. } | Block::Unknown { .. } => {}
                }
            }
        }
    }
    if !list_lines.is_empty() {
        groups.push(list_lines.join("\n"));
    }

    let mut out = groups.join("\n\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// The document title is the page's own H1, so a `Heading 1` inside the body is
/// a section beneath it and shifts down one level.
fn heading_level(document: &Document, level: u8) -> u8 {
    let offset = u8::from(document.title.is_some());
    (level + offset).min(6)
}

pub fn inlines(content: &[Inline]) -> String {
    content.iter().map(inline).collect()
}

fn inline(value: &Inline) -> String {
    match value {
        Inline::Text { text } => escape(text),
        Inline::Bold { content } => wrap(content, "**"),
        Inline::Italic { content } => wrap(content, "*"),
        Inline::Link { content, target } => {
            format!("[{}]({})", inlines(content), escape_target(target))
        }
    }
}

/// Emphasis cannot span leading or trailing whitespace in Markdown, so the
/// spaces are moved outside the markers.
///
/// Observed: bold lead-ins are written with the trailing space inside the bold
/// run, as `"Lead-in heading: "`.
fn wrap(content: &[Inline], marker: &str) -> String {
    let rendered = inlines(content);
    let trimmed = rendered.trim();
    if trimmed.is_empty() {
        return rendered;
    }
    let leading = &rendered[..rendered.len() - rendered.trim_start().len()];
    let trailing = &rendered[rendered.trim_end().len()..];
    format!("{leading}{marker}{trimmed}{marker}{trailing}")
}

/// Escapes the characters that would otherwise change the structure.
///
/// Deliberately minimal: over-escaping makes ordinary prose unreadable, and the
/// output is meant to be read as well as parsed.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, character) in text.char_indices() {
        let at_start = index == 0;
        match character {
            '\\' | '*' | '_' | '`' | '[' | ']' => {
                out.push('\\');
                out.push(character);
            }
            '#' | '>' | '-' | '+' if at_start => {
                out.push('\\');
                out.push(character);
            }
            _ => out.push(character),
        }
    }
    out
}

fn escape_target(target: &str) -> String {
    target.replace(' ', "%20").replace(')', "%29")
}
