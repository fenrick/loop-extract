//! Gzip member discovery and bounded decompression.

use std::io::Write;

use flate2::write::GzEncoder;
use flate2::Compression;
use loop_extract::gzip::{scan, Limits};

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn finds_a_single_member() {
    let mut file = b"leading binary junk".to_vec();
    file.extend(gzip(b"hello loop"));
    file.extend(b"trailing junk");

    let result = scan(&file, Limits::default());

    assert_eq!(result.members.len(), 1);
    assert_eq!(result.members[0].data, b"hello loop");
    assert_eq!(result.members[0].file_offset, 19);
    assert!(result.failed.is_empty());
}

#[test]
fn finds_members_laid_end_to_end() {
    // Observed layout: a container holds many independent members in sequence.
    let mut file = Vec::new();
    file.extend(gzip(b"first payload"));
    file.extend(gzip(b"second payload"));
    file.extend(gzip(b"third payload"));

    let result = scan(&file, Limits::default());

    assert_eq!(result.members.len(), 3);
    assert_eq!(result.members[0].data, b"first payload");
    assert_eq!(result.members[1].data, b"second payload");
    assert_eq!(result.members[2].data, b"third payload");
}

#[test]
fn reports_compressed_length_per_member() {
    let first = gzip(b"first payload");
    let mut file = first.clone();
    file.extend(gzip(b"second payload"));

    let result = scan(&file, Limits::default());

    assert_eq!(result.members[0].compressed_len, Some(first.len()));
}

#[test]
fn a_false_positive_signature_does_not_stop_the_scan() {
    // A coincidental `1F 8B 08` must be rejected without losing the real member
    // that follows it.
    let mut file = vec![0x00, 0x1f, 0x8b, 0x08, 0x99, 0x99, 0x99, 0x99];
    file.extend(gzip(b"real content"));

    let result = scan(&file, Limits::default());

    assert_eq!(result.members.len(), 1);
    assert_eq!(result.members[0].data, b"real content");
    assert_eq!(result.failed.len(), 1);
    assert_eq!(result.failed[0].file_offset, 1);
    assert_eq!(result.candidate_offsets.len(), 2);
}

#[test]
fn a_file_of_only_false_positives_yields_no_members() {
    let file = vec![0x1f, 0x8b, 0x08, 0xff, 0x1f, 0x8b, 0x08, 0x00];

    let result = scan(&file, Limits::default());

    assert!(result.members.is_empty());
    assert_eq!(result.failed.len(), 2);
}

#[test]
fn a_member_is_truncated_at_the_per_member_limit() {
    let file = gzip(&vec![b'x'; 10_000]);

    let result = scan(
        &file,
        Limits {
            max_member_bytes: 1_000,
            max_total_bytes: 1 << 20,
        },
    );

    assert_eq!(result.members.len(), 1);
    assert_eq!(result.members[0].data.len(), 1_000);
    assert!(result.members[0].truncated);
    assert!(result.warnings.iter().any(|w| w.code == "member_truncated"));
}

#[test]
fn the_total_budget_stops_further_inflation() {
    let mut file = Vec::new();
    for _ in 0..5 {
        file.extend(gzip(&vec![b'y'; 4_000]));
    }

    let result = scan(
        &file,
        Limits {
            max_member_bytes: 1 << 20,
            max_total_bytes: 5_000,
        },
    );

    assert!(result.members.len() < 5);
    assert!(result
        .warnings
        .iter()
        .any(|w| w.code == "decompression_budget_exhausted"));
}

#[test]
fn a_member_cut_short_never_panics_and_never_stops_the_scan() {
    // Incompressible content, so the compressed stream is long enough to cut.
    let mut payload = Vec::with_capacity(50_000);
    let mut state: u32 = 0x1234_5678;
    for _ in 0..50_000 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        payload.push((state >> 16) as u8);
    }
    let full = gzip(&payload);
    assert!(
        full.len() > 400,
        "test needs a compressed stream longer than the cut"
    );
    let cut = &full[..full.len() - 200];

    let result = scan(cut, Limits::default());

    // Either recovered partially or rejected outright; never a panic, and never
    // a lost scan.
    assert!(result.members.len() + result.failed.len() >= 1);
    if let Some(member) = result.members.first() {
        assert!(member.partial || member.truncated || !member.data.is_empty());
    }
}
