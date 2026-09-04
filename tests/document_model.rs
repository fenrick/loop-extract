//! Reconstruction of blocks and inline runs from merge-tree segments, and the
//! three renderers.

use loop_extract::discovery::PayloadCandidate;
use loop_extract::document::model::{
    Block, ContentSource, Document, Inline, Metadata, TitleSelection,
};
use loop_extract::document::reconstruct::blocks_from;
use loop_extract::fluid::merge_tree::{assemble, Chunk, MergeTree};
use loop_extract::render::{markdown, text};
use loop_extract::warning::Warning;

/// Builds a one-chunk tree from a `segmentTexts` array.
fn tree(segment_texts: &str) -> MergeTree {
    let json = format!(
        r#"{{"chunkStartSegmentIndex":0,"chunkSegmentCount":99,"totalSegmentCount":99,
             "totalLengthChars":100,"chunkSequenceNumber":1,"segmentTexts":{segment_texts}}}"#
    );
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let payload = PayloadCandidate {
        member_index: 0,
        start: 0,
        end: json.len(),
        anchor: "\"segmentTexts\"",
        value,
    };
    let chunk = Chunk::from_payload(&payload, 0, json.as_bytes()).unwrap();
    let mut warnings = Vec::new();
    assemble(vec![chunk], &mut warnings).remove(0)
}

fn blocks(segment_texts: &str) -> Vec<Block> {
    let mut warnings: Vec<Warning> = Vec::new();
    blocks_from(&tree(segment_texts).segments, &mut warnings)
}

fn document(segment_texts: &str, title: Option<&str>) -> Document {
    Document {
        title: title.map(str::to_string),
        blocks: blocks(segment_texts),
        metadata: Metadata {
            source_path: "test".into(),
            file_size: 0,
            format: "test".into(),
            components: Vec::new(),
            content_source: ContentSource::Snapshot,
            title_selection: TitleSelection::None,
            body_tree_hash: None,
            body_segment_count: 0,
            merge_trees: 1,
            replay: None,
        },
    }
}

const PARA: &str = r#"{"marker":{"refType":1},"props":{"nodeType":"Paragraph"}}"#;

#[test]
fn a_marker_closes_the_text_run_before_it() {
    let found = blocks(&format!(r#"["one",{PARA},"two",{PARA}]"#));

    assert_eq!(found.len(), 2);
    assert_eq!(markdown::inlines(found[0].content()), "one");
    assert_eq!(markdown::inlines(found[1].content()), "two");
}

#[test]
fn a_leading_marker_makes_an_empty_block_not_a_lost_one() {
    // Observed: bodies often open with markers that have no text before them.
    let found = blocks(&format!(r#"[{PARA},"first real text",{PARA}]"#));

    assert_eq!(found.len(), 2);
    assert!(found[0].is_empty());
    assert!(!found[1].is_empty());
}

#[test]
fn text_after_the_last_marker_still_forms_a_block() {
    let found = blocks(&format!(r#"["closed",{PARA},"dangling"]"#));

    assert_eq!(found.len(), 2);
    assert_eq!(markdown::inlines(found[1].content()), "dangling");
}

#[test]
fn consecutive_segments_sharing_formatting_join_into_one_run() {
    let found = blocks(&format!(
        r#"[{{"text":"bold ","props":{{"format!bold":true}}}},
            {{"text":"still bold","props":{{"format!bold":true}}}},
            {{"text":" plain","props":{{}}}},{PARA}]"#
    ));

    match &found[0] {
        Block::Paragraph { content, .. } => {
            assert_eq!(
                content.len(),
                2,
                "expected one bold run and one plain run: {content:?}"
            );
            assert_eq!(
                content[0],
                Inline::Bold {
                    content: vec![Inline::text("bold still bold")]
                }
            );
            assert_eq!(content[1], Inline::text(" plain"));
        }
        other => panic!("expected a paragraph, got {other:?}"),
    }
}

#[test]
fn bold_and_italic_nest_with_bold_outermost() {
    let found = blocks(&format!(
        r#"[{{"text":"both","props":{{"format!bold":true,"format!italic":true}}}},{PARA}]"#
    ));

    assert_eq!(
        found[0].content()[0],
        Inline::Bold {
            content: vec![Inline::Italic {
                content: vec![Inline::text("both")]
            }]
        }
    );
}

#[test]
fn a_hyperlink_wraps_its_formatting() {
    let found = blocks(&format!(
        r#"[{{"text":"click","props":{{"format!bold":true,"hyperlink!url":"https://example.org"}}}},{PARA}]"#
    ));

    match &found[0].content()[0] {
        Inline::Link { content, target } => {
            assert_eq!(target, "https://example.org");
            assert_eq!(
                content[0],
                Inline::Bold {
                    content: vec![Inline::text("click")]
                }
            );
        }
        other => panic!("expected a link, got {other:?}"),
    }
}

#[test]
fn a_heading_style_makes_a_heading_block() {
    let found = blocks(
        r#"["Section title",{"marker":{"refType":1},
            "props":{"nodeType":"Paragraph","style!reference":"Heading 2"}}]"#,
    );

    match &found[0] {
        Block::Heading { level, .. } => assert_eq!(*level, 2),
        other => panic!("expected a heading, got {other:?}"),
    }
}

#[test]
fn an_unrecognised_style_leaves_a_paragraph() {
    // Observed: `style!reference: "Body"` occurs and is not a heading.
    let found = blocks(
        r#"["ordinary",{"marker":{"refType":1},
            "props":{"nodeType":"Paragraph","style!reference":"Body"}}]"#,
    );
    assert!(matches!(found[0], Block::Paragraph { .. }));
}

#[test]
fn a_list_reference_makes_a_list_item_at_its_depth() {
    let found = blocks(
        r#"["item",{"marker":{"refType":1},
            "props":{"nodeType":"Paragraph","list!reference":"2;list!list-abc0"}}]"#,
    );

    match &found[0] {
        Block::ListItem { depth, list_id, .. } => {
            assert_eq!(*depth, 2);
            assert_eq!(list_id.as_deref(), Some("list!list-abc0"));
        }
        other => panic!("expected a list item, got {other:?}"),
    }
}

#[test]
fn a_non_paragraph_marker_becomes_an_embedded_block() {
    let found = blocks(
        r#"[{"marker":{"refType":1},"props":{"nodeType":"TableOfContents","component!display":"block"}}]"#,
    );

    match &found[0] {
        Block::Embedded { node_type, .. } => assert_eq!(node_type, "TableOfContents"),
        other => panic!("expected an embedded block, got {other:?}"),
    }
}

#[test]
fn markdown_renders_nested_lists_by_indentation() {
    let list = |depth: u32, body: &str| {
        format!(
            r#""{body}",{{"marker":{{"refType":1}},"props":{{"nodeType":"Paragraph","list!reference":"{depth};list!list-a0"}}}}"#
        )
    };
    let segments = format!(
        "[{},{},{}]",
        list(0, "Parent item"),
        list(1, "Child item"),
        list(0, "Second parent")
    );

    let rendered = markdown::render(&document(&segments, None));

    assert_eq!(
        rendered,
        "- Parent item\n    - Child item\n- Second parent\n"
    );
}

#[test]
fn a_new_list_id_starts_a_new_list() {
    let segments = r#"["first",{"marker":{"refType":1},"props":{"nodeType":"Paragraph","list!reference":"0;list!list-a0"}},
            "second",{"marker":{"refType":1},"props":{"nodeType":"Paragraph","list!reference":"0;list!list-b0"}}]"#
        .to_string();

    let rendered = markdown::render(&document(&segments, None));

    assert_eq!(rendered, "- first\n\n- second\n");
}

#[test]
fn markdown_puts_the_title_first_as_a_level_one_heading() {
    let rendered = markdown::render(&document(
        &format!(r#"["body",{PARA}]"#),
        Some("Example Page 09/02"),
    ));
    assert_eq!(rendered, "# Example Page 09/02\n\nbody\n");
}

#[test]
fn a_body_heading_sits_below_the_title() {
    // The title is the page's own H1, so `Heading 1` inside the body is H2.
    let segments = r#"["Section",{"marker":{"refType":1},"props":{"nodeType":"Paragraph","style!reference":"Heading 1"}}]"#;

    let with_title = markdown::render(&document(segments, Some("Doc")));
    let without_title = markdown::render(&document(segments, None));

    assert!(with_title.contains("## Section"));
    assert!(without_title.starts_with("# Section"));
}

#[test]
fn emphasis_markers_sit_inside_the_surrounding_spaces() {
    // Observed: bold lead-ins carry their trailing space inside the bold run,
    // which Markdown will not render as emphasis.
    let segments = format!(
        r#"[{{"text":"Lead-in: ","props":{{"format!bold":true}}}},{{"text":"rest","props":{{}}}},{PARA}]"#
    );

    let rendered = markdown::render(&document(&segments, None));

    assert_eq!(rendered, "**Lead-in:** rest\n");
}

#[test]
fn markdown_escapes_characters_that_would_change_the_structure() {
    let segments = format!(r#"[{{"text":"a * b _ c [d] `e`","props":{{}}}},{PARA}]"#);
    let rendered = markdown::render(&document(&segments, None));
    assert_eq!(rendered, "a \\* b \\_ c \\[d\\] \\`e\\`\n");
}

#[test]
fn markdown_escapes_a_leading_hash_so_text_does_not_become_a_heading() {
    let segments = format!(r##"[{{"text":"# not a heading","props":{{}}}},{PARA}]"##);
    assert_eq!(
        markdown::render(&document(&segments, None)),
        "\\# not a heading\n"
    );
}

#[test]
fn plain_text_drops_formatting_but_keeps_structure() {
    let segments = r#"[{"text":"Lead-in: ","props":{"format!bold":true}},{"text":"rest","props":{}},
            {"marker":{"refType":1},"props":{"nodeType":"Paragraph","list!reference":"1;list!list-a0"}}]"#
        .to_string();

    let rendered = text::render(&document(&segments, Some("Title")));

    assert_eq!(rendered, "Title\n\n    - Lead-in: rest\n");
}

#[test]
fn empty_blocks_do_not_produce_blank_output() {
    let rendered = markdown::render(&document(
        &format!(r#"[{PARA},{PARA},"real",{PARA}]"#),
        None,
    ));
    assert_eq!(rendered, "real\n");
}

#[test]
fn rendering_is_deterministic() {
    let segments = format!(r#"["one",{PARA},"two",{PARA}]"#);
    let first = markdown::render(&document(&segments, Some("T")));
    let second = markdown::render(&document(&segments, Some("T")));
    assert_eq!(first, second);
}
