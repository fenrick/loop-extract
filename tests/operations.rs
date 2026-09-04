//! Operation-log parsing and replay.

use loop_extract::fluid::merge_tree::Segment;
use loop_extract::operations::log::{counts, parse, DocumentOp};
use loop_extract::operations::replay::{replay, replay_onto};
use loop_extract::operations::segments_text;
use loop_extract::operations::OperationLog;

/// Wraps a merge-tree operation in the nesting observed in real containers.
fn delta(channel: &str, op_json: &str) -> String {
    let inner = format!(
        r#"{{"type":"component","contents":{{"address":"{channel}",
            "contents":{{"content":{{"address":"text","contents":{op_json}}},"type":"op"}}}}}}"#
    );
    let escaped = serde_json::to_string(&inner).unwrap();
    format!(r#"{{"clientSequenceNumber":1,"type":"op","contents":{escaped}}}"#)
}

fn insert(pos: usize, text: &str) -> String {
    format!(r#"{{"pos1":{pos},"seg":{{"text":"{text}"}},"type":0}}"#)
}

fn text_of(segments: &[Segment]) -> String {
    segments_text(segments)
}

#[test]
fn an_insert_is_found_through_the_nested_json_strings() {
    let ops = parse(&[delta("chan-1", &insert(0, "hello"))]);

    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].channel.as_deref(), Some("chan-1"));
    assert_eq!(ops[0].data_structure.as_deref(), Some("text"));
    assert_eq!(ops[0].address(), "chan-1/text");
    match &ops[0].op {
        DocumentOp::Insert { position, segments } => {
            assert_eq!(*position, 0);
            assert_eq!(segments[0].text(), Some("hello"));
        }
        other => panic!("expected an insert, got {other:?}"),
    }
}

#[test]
fn remove_and_annotate_are_parsed_and_counted() {
    let ops = parse(&[
        delta("c", &insert(0, "abc")),
        delta("c", r#"{"pos1":1,"pos2":2,"type":1}"#),
        delta(
            "c",
            r#"{"pos1":0,"pos2":3,"props":{"format!bold":true},"type":2}"#,
        ),
    ]);

    let totals = counts(&ops);
    assert_eq!(totals.inserts, 1);
    assert_eq!(totals.removes, 1);
    assert_eq!(totals.annotates, 1);
}

#[test]
fn malformed_deltas_are_skipped_without_losing_the_rest() {
    let ops = parse(&[
        "not json at all".to_string(),
        delta("c", &insert(0, "kept")),
        "{".to_string(),
    ]);
    assert_eq!(ops.len(), 1);
}

#[test]
fn inserts_rebuild_text_in_position_order() {
    let ops = [
        DocumentOp::Insert {
            position: 0,
            segments: vec![text_segment("world")],
        },
        DocumentOp::Insert {
            position: 0,
            segments: vec![text_segment("hello ")],
        },
        DocumentOp::Insert {
            position: 11,
            segments: vec![text_segment("!")],
        },
    ];

    let result = replay(&ops);

    assert_eq!(text_of(&result.segments), "hello world!");
    assert_eq!(result.applied, 3);
}

#[test]
fn an_insert_inside_a_segment_splits_it() {
    let ops = [
        DocumentOp::Insert {
            position: 0,
            segments: vec![text_segment("abcd")],
        },
        DocumentOp::Insert {
            position: 2,
            segments: vec![text_segment("XY")],
        },
    ];

    let result = replay(&ops);

    assert_eq!(text_of(&result.segments), "abXYcd");
    assert_eq!(
        result.segments.len(),
        3,
        "the original segment should have been split"
    );
}

#[test]
fn a_marker_occupies_one_position() {
    let ops = [
        DocumentOp::Insert {
            position: 0,
            segments: vec![marker_segment()],
        },
        DocumentOp::Insert {
            position: 1,
            segments: vec![text_segment("after")],
        },
    ];

    let result = replay(&ops);

    assert_eq!(text_of(&result.segments), "after");
    assert!(matches!(result.segments[0], Segment::Marker { .. }));
}

#[test]
fn a_remove_deletes_exactly_its_range() {
    let ops = [
        DocumentOp::Insert {
            position: 0,
            segments: vec![text_segment("abcdef")],
        },
        DocumentOp::Remove { start: 1, end: 3 },
    ];

    let result = replay(&ops);
    assert_eq!(text_of(&result.segments), "adef");
}

#[test]
fn a_remove_spanning_segments_works() {
    let ops = [
        DocumentOp::Insert {
            position: 0,
            segments: vec![text_segment("abc")],
        },
        DocumentOp::Insert {
            position: 3,
            segments: vec![text_segment("def")],
        },
        DocumentOp::Remove { start: 2, end: 4 },
    ];

    let result = replay(&ops);
    assert_eq!(text_of(&result.segments), "abef");
}

#[test]
fn an_insert_past_the_end_is_appended_and_reported() {
    let ops = [DocumentOp::Insert {
        position: 9,
        segments: vec![text_segment("late")],
    }];

    let result = replay(&ops);

    assert_eq!(text_of(&result.segments), "late");
    assert_eq!(result.skipped, 1);
    assert!(result
        .warnings
        .iter()
        .any(|w| w.code == "operation_position_out_of_range"));
}

#[test]
fn annotate_is_counted_as_skipped_not_applied() {
    let ops = [
        DocumentOp::Insert {
            position: 0,
            segments: vec![text_segment("abc")],
        },
        DocumentOp::Annotate {
            start: 0,
            end: 3,
            properties: Default::default(),
        },
    ];

    let result = replay(&ops);

    assert_eq!(text_of(&result.segments), "abc");
    assert_eq!(result.applied, 1);
    assert_eq!(result.skipped, 1);
}

#[test]
fn replaying_on_top_of_a_snapshot_uses_it_as_the_starting_positions() {
    // Observed: an operation-backed page has a snapshot of empty markers, and
    // its first insert lands at position 1, after the first marker.
    let base = vec![marker_segment(), marker_segment()];
    let ops = [DocumentOp::Insert {
        position: 1,
        segments: vec![text_segment("body")],
    }];

    let result = replay_onto(base, &ops);

    assert_eq!(text_of(&result.segments), "body");
    assert_eq!(result.segments.len(), 3);
    assert_eq!(result.skipped, 0, "seeding should make the position valid");
}

#[test]
fn positions_count_utf16_units_so_astral_characters_do_not_shift_text() {
    // An emoji is one char in Rust but two UTF-16 units, which is what the
    // JavaScript runtime that wrote the operations counted.
    let ops = [
        DocumentOp::Insert {
            position: 0,
            segments: vec![text_segment("a\u{1F600}b")],
        },
        DocumentOp::Insert {
            position: 3,
            segments: vec![text_segment("X")],
        },
    ];

    let result = replay(&ops);
    assert_eq!(text_of(&result.segments), "a\u{1F600}Xb");
}

#[test]
fn a_split_inside_a_surrogate_pair_is_refused_rather_than_corrupting_text() {
    let ops = [
        DocumentOp::Insert {
            position: 0,
            segments: vec![text_segment("\u{1F600}")],
        },
        DocumentOp::Insert {
            position: 1,
            segments: vec![text_segment("X")],
        },
    ];

    let result = replay(&ops);

    assert_eq!(result.skipped, 1);
    assert!(text_of(&result.segments).contains('\u{1F600}'));
}

#[test]
fn addresses_are_ranked_by_insert_count_deterministically() {
    let log = OperationLog::parse(&[
        delta("quiet", &insert(0, "a")),
        delta("busy", &insert(0, "b")),
        delta("busy", &insert(1, "c")),
    ]);

    let ranked = log.addresses_by_inserts();

    assert_eq!(ranked[0], ("busy/text".to_string(), 2));
    assert_eq!(ranked[1], ("quiet/text".to_string(), 1));
}

#[test]
fn only_the_chosen_address_is_replayed() {
    let log = OperationLog::parse(&[
        delta("a", &insert(0, "from-a")),
        delta("b", &insert(0, "from-b")),
    ]);

    let result = log.replay_address("a/text", Vec::new());

    assert_eq!(text_of(&result.segments), "from-a");
}

fn text_segment(text: &str) -> Segment {
    Segment::from_value(&serde_json::json!({ "text": text }))
}

fn marker_segment() -> Segment {
    Segment::from_value(&serde_json::json!({ "marker": { "refType": 1 } }))
}
