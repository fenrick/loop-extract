//! Plain-text rendering: formatting removed, structure kept readable.

use crate::document::model::{plain_of, Block, Document};

const INDENT: &str = "    ";

pub fn render(document: &Document) -> String {
    let mut groups: Vec<String> = Vec::new();

    if let Some(title) = &document.title {
        if !title.is_empty() {
            groups.push(title.clone());
        }
    }

    let mut list_lines: Vec<String> = Vec::new();

    for block in &document.blocks {
        match block {
            Block::ListItem { depth, content, .. } if !block.is_empty() => {
                // The bullet is structure, not formatting, so it stays.
                list_lines.push(format!(
                    "{}- {}",
                    INDENT.repeat(*depth as usize),
                    plain_of(content)
                ));
            }
            _ => {
                if !list_lines.is_empty() {
                    groups.push(list_lines.join("\n"));
                    list_lines.clear();
                }
                if !block.is_empty() {
                    groups.push(plain_of(block.content()));
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
