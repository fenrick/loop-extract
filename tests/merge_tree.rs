//! Segment parsing, tree assembly, deduplication and role assignment.

use loop_extract::discovery::PayloadCandidate;
use loop_extract::fluid::merge_tree::{assemble, Chunk, Segment, TreeRole};
use loop_extract::fluid::properties::{classify, ListReference, PropertyClass};
use loop_extract::warning::Warning;

fn chunk(json: &str) -> Chunk {
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let payload = PayloadCandidate {
        member_index: 0,
        start: 0,
        end: json.len(),
        anchor: "\"segmentTexts\"",
        value,
    };
    Chunk::from_payload(&payload, 0, json.as_bytes()).expect("a chunk with segmentTexts")
}

fn chunk_at(json: &str, member_index: usize, file_offset: usize) -> Chunk {
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let payload = PayloadCandidate {
        member_index,
        start: 0,
        end: json.len(),
        anchor: "\"segmentTexts\"",
        value,
    };
    Chunk::from_payload(&payload, file_offset, json.as_bytes()).expect("a chunk with segmentTexts")
}

fn text_of(segments: &[Segment]) -> String {
    segments.iter().filter_map(Segment::text).collect()
}

#[test]
fn a_bare_string_entry_is_a_text_segment() {
    // Observed: `"segmentTexts":["Meeting ", {...}]` mixes bare strings and
    // objects in one array.
    let tree =
        chunk(r#"{"totalSegmentCount":1,"chunkSegmentCount":1,"segmentTexts":["Meeting "]}"#);
    assert_eq!(text_of(&tree.segments), "Meeting ");
    assert!(tree.segments[0].props().unwrap().is_empty());
}

#[test]
fn a_text_object_keeps_its_properties() {
    let tree = chunk(
        r#"{"totalSegmentCount":1,"chunkSegmentCount":1,
            "segmentTexts":[{"text":"Heading","props":{"format!bold":true}}]}"#,
    );
    let props = tree.segments[0].props().unwrap();
    assert_eq!(props.bold(), Some(true));
}

#[test]
fn a_marker_object_becomes_a_marker_segment() {
    let tree = chunk(
        r#"{"totalSegmentCount":1,"chunkSegmentCount":1,
            "segmentTexts":[{"marker":{"refType":1},"props":{"nodeType":"Paragraph"}}]}"#,
    );
    match &tree.segments[0] {
        Segment::Marker { ref_type, props } => {
            assert_eq!(*ref_type, Some(1));
            assert_eq!(props.node_type(), Some("Paragraph"));
        }
        other => panic!("expected a marker, got {other:?}"),
    }
}

#[test]
fn an_unrecognised_entry_is_preserved_not_discarded() {
    let tree = chunk(
        r#"{"totalSegmentCount":1,"chunkSegmentCount":1,"segmentTexts":[{"mystery":{"v":9}}]}"#,
    );
    match &tree.segments[0] {
        Segment::Unknown { raw } => assert_eq!(raw["mystery"]["v"], 9),
        other => panic!("expected the entry to be preserved, got {other:?}"),
    }
}

#[test]
fn a_payload_without_segment_texts_is_not_a_chunk() {
    let value: serde_json::Value = serde_json::from_str(r#"{"package":{"name":"x"}}"#).unwrap();
    let payload = PayloadCandidate {
        member_index: 0,
        start: 0,
        end: 0,
        anchor: "\"package\"",
        value,
    };
    assert!(Chunk::from_payload(&payload, 0, b"{}").is_none());
}

#[test]
fn byte_identical_copies_collapse_into_one_tree() {
    // Observed: every tree appears at least twice, once inside an aggregate
    // member and once standalone, byte for byte identical.
    let json = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":2,"totalSegmentCount":2,
                   "totalLengthChars":5,"chunkSequenceNumber":7,"segmentTexts":["ab","cde"]}"#;
    let mut warnings: Vec<Warning> = Vec::new();

    let trees = assemble(
        vec![chunk_at(json, 0, 100), chunk_at(json, 3, 900)],
        &mut warnings,
    );

    assert_eq!(trees.len(), 1);
    assert_eq!(trees[0].duplicate_copies, 1);
    assert_eq!(trees[0].sources.len(), 2);
    assert_eq!(text_of(&trees[0].segments), "abcde");
    assert!(warnings.is_empty());
}

#[test]
fn distinct_trees_sharing_counters_do_not_merge() {
    // Regression: a page built around an embedded table holds dozens of
    // one-segment cells whose totalSegmentCount, totalLengthChars and chunkSequenceNumber
    // are all identical. Grouping on those counters merged them and produced 88
    // spurious conflicts.
    let a = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"totalSegmentCount":1,
                "totalLengthChars":7,"chunkSequenceNumber":561,"segmentTexts":["Cell A"]}"#;
    let b = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"totalSegmentCount":1,
                "totalLengthChars":7,"chunkSequenceNumber":561,"segmentTexts":["Cell B"]}"#;
    let mut warnings: Vec<Warning> = Vec::new();

    let trees = assemble(vec![chunk_at(a, 0, 10), chunk_at(b, 1, 20)], &mut warnings);

    assert_eq!(trees.len(), 2);
    assert!(
        warnings.is_empty(),
        "no conflict should be reported: {warnings:?}"
    );
    let texts: Vec<String> = trees.iter().map(|t| text_of(&t.segments)).collect();
    assert!(texts.contains(&"Cell A".to_string()));
    assert!(texts.contains(&"Cell B".to_string()));
}

#[test]
fn a_split_tree_is_reassembled_in_segment_order() {
    // Observed in a real container: 134 segments stored as 0..79 and 80..133.
    let tail = r#"{"chunkStartSegmentIndex":2,"chunkSegmentCount":2,"totalSegmentCount":4,
                   "totalLengthChars":8,"chunkSequenceNumber":304,"segmentTexts":["cc","dd"]}"#;
    let head = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":2,"totalSegmentCount":4,
                   "totalLengthChars":8,"chunkSequenceNumber":304,"segmentTexts":["aa","bb"]}"#;
    let mut warnings: Vec<Warning> = Vec::new();

    // Deliberately out of order on input.
    let trees = assemble(
        vec![chunk_at(tail, 5, 500), chunk_at(head, 1, 100)],
        &mut warnings,
    );

    assert_eq!(trees.len(), 1);
    assert_eq!(text_of(&trees[0].segments), "aabbccdd");
    assert_eq!(trees[0].chunks.len(), 2);
    assert!(trees[0].complete);
    assert!(warnings.is_empty());
}

#[test]
fn a_missing_chunk_is_reported_not_silently_joined() {
    let head = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":2,"totalSegmentCount":6,
                   "totalLengthChars":12,"chunkSequenceNumber":9,"segmentTexts":["aa","bb"]}"#;
    let tail = r#"{"chunkStartSegmentIndex":4,"chunkSegmentCount":2,"totalSegmentCount":6,
                   "totalLengthChars":12,"chunkSequenceNumber":9,"segmentTexts":["ee","ff"]}"#;
    let mut warnings: Vec<Warning> = Vec::new();

    let trees = assemble(
        vec![chunk_at(head, 0, 10), chunk_at(tail, 1, 20)],
        &mut warnings,
    );

    assert_eq!(trees.len(), 1);
    assert!(!trees[0].complete);
    assert!(warnings.iter().any(|w| w.code == "chunk_gap"));
    assert!(warnings.iter().any(|w| w.code == "tree_incomplete"));
}

#[test]
fn conflicting_chunks_at_one_position_are_reported_and_not_merged() {
    let a = r#"{"chunkStartSegmentIndex":2,"chunkSegmentCount":1,"totalSegmentCount":3,
                "totalLengthChars":6,"chunkSequenceNumber":11,"segmentTexts":["xx"]}"#;
    let b = r#"{"chunkStartSegmentIndex":2,"chunkSegmentCount":1,"totalSegmentCount":3,
                "totalLengthChars":6,"chunkSequenceNumber":11,"segmentTexts":["yy"]}"#;
    let mut warnings: Vec<Warning> = Vec::new();

    let trees = assemble(vec![chunk_at(a, 0, 10), chunk_at(b, 1, 20)], &mut warnings);

    assert_eq!(trees.len(), 1);
    assert!(warnings.iter().any(|w| w.code == "chunk_conflict"));
    let text = text_of(&trees[0].segments);
    assert!(
        text == "xx" || text == "yy",
        "one copy kept, never both: {text}"
    );
}

#[test]
fn assembly_is_deterministic_regardless_of_input_order() {
    let a = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"totalSegmentCount":1,
                "totalLengthChars":300,"chunkSequenceNumber":1,"segmentTexts":["long body text"]}"#;
    let b = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"totalSegmentCount":1,
                "totalLengthChars":10,"chunkSequenceNumber":2,"segmentTexts":["Title"]}"#;
    let mut w1: Vec<Warning> = Vec::new();
    let mut w2: Vec<Warning> = Vec::new();

    let forward = assemble(vec![chunk_at(a, 0, 10), chunk_at(b, 1, 20)], &mut w1);
    let backward = assemble(vec![chunk_at(b, 1, 20), chunk_at(a, 0, 10)], &mut w2);

    let hashes = |trees: &[loop_extract::fluid::merge_tree::MergeTree]| {
        trees.iter().map(|t| t.content_hash).collect::<Vec<_>>()
    };
    assert_eq!(hashes(&forward), hashes(&backward));
    assert_eq!(forward[0].role, TreeRole::BodyCandidate);
}

#[test]
fn the_longest_tree_is_the_body_candidate_and_a_short_one_a_title_candidate() {
    let body = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"totalSegmentCount":1,
                   "totalLengthChars":4304,"chunkSequenceNumber":96,"segmentTexts":["body"]}"#;
    let title = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"totalSegmentCount":1,
                    "totalLengthChars":46,"chunkSequenceNumber":96,"segmentTexts":["Example Page 09/02"]}"#;
    let mut warnings: Vec<Warning> = Vec::new();

    let trees = assemble(
        vec![chunk_at(body, 0, 10), chunk_at(title, 1, 20)],
        &mut warnings,
    );

    assert_eq!(trees[0].role, TreeRole::BodyCandidate);
    assert_eq!(trees[1].role, TreeRole::TitleCandidate);
}

#[test]
fn a_tree_with_no_text_is_auxiliary() {
    // Observed: several corpus files hold only empty paragraph markers in the
    // snapshot; their text lives in the operation log.
    let empty = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"totalSegmentCount":1,
                    "totalLengthChars":1,"chunkSequenceNumber":0,
                    "segmentTexts":[{"marker":{"refType":1},"props":{"nodeType":"Paragraph"}}]}"#;
    let mut warnings: Vec<Warning> = Vec::new();

    let trees = assemble(vec![chunk(empty)], &mut warnings);

    assert_eq!(trees[0].role, TreeRole::Auxiliary);
    assert_eq!(trees[0].evidence.text_segments, 0);
}

#[test]
fn evidence_records_formatting_and_structure() {
    let json = r#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":3,"totalSegmentCount":3,
                   "totalLengthChars":20,"chunkSequenceNumber":1,"segmentTexts":[
                     {"text":"Lead-in: ","props":{"format!bold":true}},
                     {"text":"rest","props":{"format!italic":true,"hyperlink!url":"https://example.org"}},
                     {"marker":{"refType":1},"props":{"nodeType":"Paragraph","style!reference":"Heading 1",
                        "list!reference":"1;list!list-abc0","placeholder!attributeText":"placeholder!addTitle"}}]}"#;
    let mut warnings: Vec<Warning> = Vec::new();

    let trees = assemble(vec![chunk(json)], &mut warnings);
    let evidence = &trees[0].evidence;

    assert!(evidence.has_bold);
    assert!(evidence.has_italic);
    assert!(evidence.has_hyperlink);
    assert!(evidence.has_list_reference);
    assert!(evidence.has_add_title_marker);
    assert_eq!(evidence.style_references, vec!["Heading 1".to_string()]);
    assert_eq!(evidence.node_types, vec!["Paragraph".to_string()]);
    assert_eq!(evidence.text_segments, 2);
    assert_eq!(evidence.marker_segments, 1);
}

#[test]
fn a_list_reference_splits_into_depth_and_list_id() {
    let parsed = ListReference::parse("1;list!list-abcdef0").unwrap();
    assert_eq!(parsed.depth, Some(1));
    assert_eq!(parsed.list_id.as_deref(), Some("list!list-abcdef0"));
}

#[test]
fn a_list_reference_of_an_unexpected_shape_keeps_its_raw_value() {
    let parsed = ListReference::parse("something-else").unwrap();
    assert_eq!(parsed.depth, None);
    assert_eq!(parsed.raw, "something-else");
}

#[test]
fn property_keys_are_classified_by_how_well_they_are_understood() {
    assert_eq!(classify("format!bold"), PropertyClass::Rendered);
    assert_eq!(classify("list!itemFormat-4"), PropertyClass::Recognised);
    assert_eq!(classify("list!list-abc0"), PropertyClass::Recognised);
    assert_eq!(classify("@hyperlink!url"), PropertyClass::Recognised);
    assert_eq!(classify("future!somethingNew"), PropertyClass::Unknown);
}

#[test]
fn personal_data_keys_are_identified_for_the_renderer_to_strip() {
    use loop_extract::fluid::properties::is_personal_data;
    assert!(is_personal_data("attribution"));
    assert!(is_personal_data("@hyperlink!url"));
    assert!(!is_personal_data("hyperlink!url"));
    assert!(!is_personal_data("format!bold"));
}
