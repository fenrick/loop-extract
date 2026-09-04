//! The narrow container-framing reader: tag decoding, path recovery and payload
//! placement.

use loop_extract::discovery::envelope::{parse, read, EnvelopeError, Value};

/// Builds framing bytes for tests, mirroring the observed tag table.
#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn tag(mut self, tag: u8) -> Self {
        self.0.push(tag);
        self
    }
    fn open_map(self) -> Self {
        self.tag(0x33)
    }
    fn close_map(self) -> Self {
        self.tag(0x34)
    }
    fn open_array(self) -> Self {
        self.tag(0x31)
    }
    fn close_array(self) -> Self {
        self.tag(0x32)
    }
    /// Interns a string with a one-byte id and immediately references it.
    fn define(mut self, id: u8, text: &str) -> Self {
        self.0.extend([0x14, id, text.len() as u8]);
        self.0.extend(text.as_bytes());
        self.reference(id)
    }
    /// Interns a string with a four-byte id, without referencing it.
    fn define_wide(mut self, id: u32, text: &str) -> Self {
        self.0.push(0x15);
        self.0.extend(id.to_le_bytes());
        self.0.extend((text.len() as u32).to_le_bytes());
        self.0.extend(text.as_bytes());
        self
    }
    fn reference(mut self, id: u8) -> Self {
        self.0.extend([0x11, id]);
        self
    }
    fn reference_wide(mut self, id: u16) -> Self {
        self.0.push(0x12);
        self.0.extend(id.to_le_bytes());
        self
    }
    fn literal(mut self, text: &str) -> Self {
        self.0.extend([0x0e, text.len() as u8]);
        self.0.extend(text.as_bytes());
        self
    }
    fn int(mut self, value: u8) -> Self {
        self.0.extend([0x03, value]);
        self
    }
    fn blob(mut self, bytes: &[u8]) -> Self {
        self.0.extend([0x21, bytes.len() as u8]);
        self.0.extend(bytes);
        self
    }
    fn done(self) -> Vec<u8> {
        self.0
    }
}

#[test]
fn decodes_the_scalar_tags() {
    let bytes = Writer::default()
        .open_map()
        .literal("zero")
        .tag(0x01)
        .literal("small")
        .int(7)
        .literal("wide")
        .tag(0x05)
        .tag(0x2c)
        .tag(0x01) // 0x012c = 300
        .literal("yes")
        .tag(0x0b)
        .literal("no")
        .tag(0x0c)
        .literal("nothing")
        .tag(0x0d)
        .close_map()
        .done();

    let value = parse(&bytes).expect("decodes");

    assert_eq!(value.get("zero"), Some(&Value::Int(0)));
    assert_eq!(value.get("small"), Some(&Value::Int(7)));
    assert_eq!(value.get("wide"), Some(&Value::Int(300)));
    assert_eq!(value.get("yes"), Some(&Value::Bool(true)));
    assert_eq!(value.get("no"), Some(&Value::Bool(false)));
    assert_eq!(value.get("nothing"), Some(&Value::Null));
}

#[test]
fn an_interned_string_can_be_referenced_again() {
    let bytes = Writer::default()
        .open_map()
        .define(1, "name")
        .literal("first")
        .reference(1) // the same key, by reference only
        .literal("second")
        .close_map()
        .done();

    let value = parse(&bytes).expect("decodes");

    // Both entries use the key `name`; the map keeps insertion order.
    match value {
        Value::Map(entries) => {
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].0, Value::Str("name".into()));
            assert_eq!(entries[1].0, Value::Str("name".into()));
            assert_eq!(entries[1].1, Value::Str("second".into()));
        }
        other => panic!("expected a map, got {other:?}"),
    }
}

#[test]
fn wide_string_ids_work_past_the_single_byte_limit() {
    // Observed in a large page, whose string table passes 255 entries.
    let bytes = Writer::default()
        .open_map()
        .define_wide(256, "channel-id")
        .reference_wide(256)
        .literal("value")
        .close_map()
        .done();

    let value = parse(&bytes).expect("decodes");
    assert_eq!(value.get("channel-id"), Some(&Value::Str("value".into())));
}

#[test]
fn a_definition_between_the_last_entry_and_the_close_is_consumed() {
    let mut bytes = Writer::default().open_map().literal("a").int(1).done();
    bytes.extend([0x14, 9, 4]);
    bytes.extend(b"late");
    bytes.push(0x34);

    let value = parse(&bytes).expect("decodes");
    assert_eq!(value.get("a"), Some(&Value::Int(1)));
}

#[test]
fn blobs_are_recorded_as_ranges_not_copied() {
    let bytes = Writer::default()
        .open_map()
        .literal("data")
        .blob(b"{\"x\":1}")
        .close_map()
        .done();

    let value = parse(&bytes).expect("decodes");
    let (start, len) = value.get("data").unwrap().as_blob().expect("a blob");

    assert_eq!(&bytes[start..start + len], b"{\"x\":1}");
}

#[test]
fn arrays_and_maps_nest() {
    let bytes = Writer::default()
        .open_map()
        .literal("items")
        .open_array()
        .open_map()
        .literal("n")
        .int(1)
        .close_map()
        .open_map()
        .literal("n")
        .int(2)
        .close_map()
        .close_array()
        .close_map()
        .done();

    let value = parse(&bytes).expect("decodes");
    let items = value.get("items").unwrap().as_array().expect("an array");

    assert_eq!(items.len(), 2);
    assert_eq!(items[1].get("n"), Some(&Value::Int(2)));
}

#[test]
fn an_unknown_tag_stops_the_decode_rather_than_desyncing() {
    // Silently guessing a width would misread every following byte.
    let bytes = Writer::default()
        .open_map()
        .literal("a")
        .tag(0x7f)
        .close_map()
        .done();

    match parse(&bytes) {
        Err(EnvelopeError::UnknownTag { tag, .. }) => assert_eq!(tag, 0x7f),
        other => panic!("expected UnknownTag, got {other:?}"),
    }
}

#[test]
fn a_truncated_member_is_an_error_not_a_panic() {
    let bytes = Writer::default().open_map().literal("a").done();
    assert!(matches!(
        parse(&bytes),
        Err(EnvelopeError::Truncated { .. })
    ));
}

#[test]
fn a_string_claiming_more_bytes_than_remain_is_an_error() {
    let bytes = vec![0x0e, 0x40, b'a', b'b'];
    assert!(matches!(
        parse(&bytes),
        Err(EnvelopeError::Truncated { .. })
    ));
}

#[test]
fn deeply_nested_input_is_rejected_rather_than_overflowing_the_stack() {
    let bytes = vec![0x31; 5_000];
    assert!(matches!(parse(&bytes), Err(EnvelopeError::TooDeep { .. })));
}

#[test]
fn a_member_that_is_not_an_envelope_is_reported_as_such() {
    let bytes = Writer::default()
        .open_map()
        .literal("something")
        .int(1)
        .close_map()
        .done();
    assert!(matches!(read(&bytes), Err(EnvelopeError::NotAnEnvelope(_))));
}

/// Builds a minimal but realistic envelope: one channel holding a merge tree.
fn sample_envelope(package: &str, channel: &str) -> Vec<u8> {
    let component = format!(r#"{{"pkg":"[\"Root\",\"{package}\"]","summaryFormatVersion":2}}"#);
    let root_map = r#"{"CanvasComponentHandle":{"type":"Plain","value":{"handle":{"type":"__fluid_handle__","url":"/E"}}}}"#;

    Writer::default()
        .open_map()
        .literal("snapshot")
        .open_map()
        .literal("treeNodes")
        .open_array()
        .open_map()
        .literal("name")
        .literal(".app")
        .literal("children")
        .open_array()
        .open_map()
        .literal("name")
        .literal(".channels")
        .literal("children")
        .open_array()
        .open_map()
        .literal("name")
        .literal(channel)
        .literal("children")
        .open_array()
        .open_map()
        .literal("name")
        .literal(".component")
        .literal("value")
        .literal("blob-component")
        .close_map()
        .open_map()
        .literal("name")
        .literal("content")
        .literal("value")
        .literal("blob-content")
        .close_map()
        .open_map()
        .literal("name")
        .literal("root")
        .literal("value")
        .literal("blob-root")
        .close_map()
        .close_array()
        .close_map()
        .close_array()
        .close_map()
        .close_array()
        .close_map()
        .close_array()
        .close_map()
        .literal("blobs")
        .open_array()
        .open_map()
        .literal("id")
        .literal("blob-component")
        .literal("data")
        .blob(component.as_bytes())
        .close_map()
        .open_map()
        .literal("id")
        .literal("blob-content")
        .literal("data")
        .blob(br#"{"segmentTexts":["hello"]}"#)
        .close_map()
        .open_map()
        .literal("id")
        .literal("blob-root")
        .literal("data")
        .blob(root_map.as_bytes())
        .close_map()
        .close_array()
        .close_map()
        .done()
}

#[test]
fn every_blob_gets_a_path_from_the_summary_tree() {
    let bytes = sample_envelope("LoopCanvasComponentSingleton", "E");
    let index = read(&bytes).expect("an envelope");

    assert_eq!(
        index.blob_paths.get("blob-content").map(String::as_str),
        Some("/.app/.channels/E/content")
    );
    assert_eq!(index.blob_ranges.len(), 3);
}

#[test]
fn a_channel_is_matched_to_its_component_package() {
    let bytes = sample_envelope("LoopCanvasComponentSingleton", "E");
    let index = read(&bytes).expect("an envelope");

    assert_eq!(
        index.channel_packages.get("E").map(String::as_str),
        Some("LoopCanvasComponentSingleton")
    );
}

#[test]
fn fluid_handles_are_collected_from_the_root_map() {
    let bytes = sample_envelope("LoopCanvasComponentSingleton", "E");
    let index = read(&bytes).expect("an envelope");

    assert_eq!(
        index
            .handles
            .get("CanvasComponentHandle")
            .map(String::as_str),
        Some("/E")
    );
}

#[test]
fn a_payload_offset_resolves_to_its_channel_and_component() {
    let bytes = sample_envelope("LoopPageTitleSingleton", "title-channel");
    let index = read(&bytes).expect("an envelope");
    let (start, _) = index
        .blob_ranges
        .get("blob-content")
        .copied()
        .expect("the content blob");

    let location = index.locate(3, start).expect("a location");

    assert_eq!(location.member_index, 3);
    assert_eq!(location.channel_id.as_deref(), Some("title-channel"));
    assert_eq!(
        location.component_package.as_deref(),
        Some("LoopPageTitleSingleton")
    );
    assert_eq!(
        location.blob_path.as_deref(),
        Some("/.app/.channels/title-channel/content")
    );
}

#[test]
fn an_offset_outside_every_blob_has_no_location() {
    let bytes = sample_envelope("LoopCanvasComponentSingleton", "E");
    let index = read(&bytes).expect("an envelope");
    assert!(index.locate(0, 0).is_none());
}
