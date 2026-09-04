//! Format identification and the hard-error paths.

use loop_extract::container::{
    assess, components, Confidence, LOOP_PAGE_CONTAINER, OFFICE_FLUID_CONTAINER,
};
use loop_extract::discovery::PayloadCandidate;
use loop_extract::input::{InputError, SourceFile};

fn package_payload(json: &str) -> PayloadCandidate {
    PayloadCandidate {
        member_index: 0,
        start: 0,
        end: json.len(),
        anchor: "\"package\"",
        value: serde_json::from_str(json).unwrap(),
    }
}

#[test]
fn component_name_and_version_are_read_from_a_package_payload() {
    let payloads = vec![package_payload(
        r#"{"package":{"name":"@fluidx/loop-page-container","version":"20250129024"}}"#,
    )];

    let found = components(&payloads);

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, LOOP_PAGE_CONTAINER);
    assert_eq!(found[0].version.as_deref(), Some("20250129024"));
}

#[test]
fn repeated_package_payloads_are_reported_once() {
    let json = r#"{"package":{"name":"@fluidx/loop-page-container","version":"0.0.1"}}"#;
    let payloads = vec![package_payload(json), package_payload(json)];
    assert_eq!(components(&payloads).len(), 1);
}

#[test]
fn the_older_office_component_is_also_recognised() {
    // Observed in a 2023 `.loop` page and in both `.fluid` fixtures.
    let payloads = vec![package_payload(
        r#"{"package":{"name":"@ms/office-fluid-container","version":"20231009008"}}"#,
    )];

    let assessment = assess(29, 6, 2, components(&payloads));

    assert_eq!(assessment.confidence, Confidence::High);
    assert!(assessment.is_known_loop_component());
    assert_eq!(assessment.components[0].name, OFFICE_FLUID_CONTAINER);
}

#[test]
fn structure_without_a_known_component_is_moderate_confidence() {
    let payloads = vec![package_payload(
        r#"{"package":{"name":"@other/thing","version":"1"}}"#,
    )];

    let assessment = assess(10, 4, 2, components(&payloads));

    assert_eq!(assessment.confidence, Confidence::Moderate);
    assert!(!assessment.is_known_loop_component());
    assert_eq!(assessment.format, "Fluid-compatible snapshot");
}

#[test]
fn a_file_with_no_fluid_structure_is_not_recognised() {
    let assessment = assess(0, 0, 0, Vec::new());
    assert_eq!(assessment.confidence, Confidence::None);
}

#[test]
fn a_container_with_no_document_content_is_still_identified() {
    // Members inflate and JSON is present, but no merge tree was found.
    let payloads = vec![package_payload(
        r#"{"package":{"name":"@fluidx/loop-page-container","version":"0.0.1"}}"#,
    )];

    let assessment = assess(16, 6, 0, components(&payloads));

    assert_eq!(assessment.confidence, Confidence::Moderate);
    assert!(assessment.format.contains("no document content"));
}

#[test]
fn an_extension_alone_never_identifies_a_file() {
    // Nothing in the assessment reads the path: a `.loop` file with no gzip
    // structure is unrecognised.
    let assessment = assess(0, 0, 0, Vec::new());
    assert!(!assessment.is_known_loop_component());
}

#[test]
fn an_empty_file_is_a_hard_error() {
    let dir = std::env::temp_dir().join("loop-extract-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("empty.loop");
    std::fs::write(&path, b"").unwrap();

    match SourceFile::load(&path) {
        Err(InputError::Empty { .. }) => {}
        other => panic!("expected an Empty error, got {other:?}"),
    }
}

#[test]
fn a_missing_file_is_a_hard_error() {
    match SourceFile::load("/nonexistent/definitely-not-here.loop") {
        Err(InputError::Unreadable { .. }) => {}
        other => panic!("expected an Unreadable error, got {other:?}"),
    }
}
