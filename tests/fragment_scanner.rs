//! Finding JSON inside binary framing: brace matching and its edge cases.

use loop_extract::discovery::fragment_scanner::{match_object, FragmentScanner};
use loop_extract::discovery::PayloadDiscovery;

fn discover(member: &[u8]) -> Vec<String> {
    FragmentScanner::default()
        .discover(0, member)
        .unwrap()
        .into_iter()
        .map(|p| p.value.to_string())
        .collect()
}

#[test]
fn matches_a_simple_object() {
    let data = br#"{"a":1}"#;
    assert_eq!(match_object(data, 0, 1024), Some(data.len()));
}

#[test]
fn braces_inside_a_string_do_not_change_depth() {
    let data = br#"{"a":"} not the end {","b":2}"#;
    assert_eq!(match_object(data, 0, 1024), Some(data.len()));
}

#[test]
fn an_escaped_quote_does_not_end_the_string() {
    let data = br#"{"a":"he said \"}\" and stopped","b":2}"#;
    assert_eq!(match_object(data, 0, 1024), Some(data.len()));
}

#[test]
fn an_escaped_backslash_before_a_quote_still_ends_the_string() {
    let data = br#"{"a":"ends with a backslash \\","b":2}"#;
    assert_eq!(match_object(data, 0, 1024), Some(data.len()));
}

#[test]
fn nested_objects_close_at_the_outermost_brace() {
    let data = br#"{"a":{"b":{"c":1}},"d":2}"#;
    assert_eq!(match_object(data, 0, 1024), Some(data.len()));
}

#[test]
fn an_unclosed_object_does_not_match() {
    assert_eq!(match_object(br#"{"a":1"#, 0, 1024), None);
}

#[test]
fn a_raw_control_byte_inside_a_string_rejects_the_candidate() {
    // The usual sign that the opening brace was binary framing, not JSON.
    let data = b"{\"a\":\"bad\x01byte\"}";
    assert_eq!(match_object(data, 0, 1024), None);
}

#[test]
fn matching_stops_at_the_byte_budget() {
    let mut data = br#"{"a":""#.to_vec();
    data.extend(std::iter::repeat_n(b'x', 5_000));
    data.extend(br#""}"#);
    assert_eq!(match_object(&data, 0, 100), None);
}

#[test]
fn a_start_that_is_not_a_brace_does_not_match() {
    assert_eq!(match_object(br#" {"a":1}"#, 0, 1024), None);
}

#[test]
fn finds_a_chunk_between_binary_bytes() {
    let mut member = vec![0x33, 0x14, 0x01, 0x03, 0xb3, 0xff];
    member.extend(
        br#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"totalSegmentCount":1,"segmentTexts":["hi"]}"#,
    );
    member.extend([0x00, 0x42, 0xfe]);

    let found = discover(&member);

    assert_eq!(found.len(), 1);
    assert!(found[0].contains("\"segmentTexts\":[\"hi\"]"));
}

#[test]
fn prefers_the_smallest_enclosing_object() {
    // `segmentTexts` sits inside a chunk object, which itself sits inside a
    // larger structure. The chunk is the one wanted.
    let member =
        br#"{"outer":{"chunkStartSegmentIndex":0,"totalSegmentCount":1,"segmentTexts":["x"]},"other":1}"#;

    let found = discover(member);

    assert!(found
        .iter()
        .any(|f| f.starts_with(r#"{"chunkStartSegmentIndex""#)));
}

#[test]
fn several_anchors_in_one_object_yield_one_payload() {
    let member = br#"{"chunkStartSegmentIndex":0,"chunkSegmentCount":1,"headerMetadata":{"orderedChunkMetadata":[{"id":"header"}]},"segmentTexts":["x"]}"#;

    let found = discover(member);

    // The chunk object itself, plus the nested orderedChunkMetadata holder.
    assert!(found.iter().any(|f| f.contains("segmentTexts")));
    assert!(
        found.len() <= 2,
        "expected the chunk not to be reported repeatedly: {found:?}"
    );
}

#[test]
fn invalid_utf8_around_a_payload_is_tolerated() {
    let mut member = vec![0xff, 0xfe, 0xb3, 0x80];
    member.extend(br#"{"segmentTexts":["ok"]}"#);
    member.extend([0x80, 0xb3]);

    let found = discover(&member);

    assert_eq!(found.len(), 1);
}

#[test]
fn a_member_with_no_anchor_yields_nothing() {
    assert!(discover(b"no json here, just \x00\x01\x02 bytes").is_empty());
}

#[test]
fn malformed_json_at_an_anchor_is_skipped_without_error() {
    let member = br#"{"segmentTexts":[unquoted]}"#;
    assert!(discover(member).is_empty());
}

#[test]
fn discovery_is_deterministic() {
    let mut member = Vec::new();
    for index in 0..5 {
        member.extend([0xb3, 0x14, 0x00]);
        member.extend(
            format!(
                r#"{{"chunkStartSegmentIndex":{index},"chunkSegmentCount":1,"totalSegmentCount":5,"segmentTexts":["s{index}"]}}"#
            )
            .into_bytes(),
        );
    }

    let first = discover(&member);
    let second = discover(&member);

    assert_eq!(first, second);
    assert_eq!(first.len(), 5);
}
