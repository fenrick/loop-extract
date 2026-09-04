//! Regression tests over a corpus of real Loop and Fluid files.
//!
//! Test documents are not committed: a Loop file contains document text and its
//! authors' personal data. Point the tests at a local directory with:
//!
//! ```text
//! LOOP_EXTRACT_TEST_CORPUS=./sample-documents cargo test
//! ```
//!
//! Without that variable these tests report that they were skipped and pass, so
//! the unit tests never depend on private fixtures.
//!
//! Two layers of checking:
//!
//! * structural invariants, applied to every file in the corpus;
//! * optional per-file expectations from `tests/expectations/<filename>.json`,
//!   applied only when such a file exists. That is the hook for hand-reviewed
//!   fixtures, and for a known-good Loop/Markdown fixture pair when one becomes
//!   available. Hand-reviewed expectations are *our* reading of the document,
//!   not canonical Microsoft output.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use loop_extract::analysis::Analysis;
use loop_extract::document::reconstruct::reconstruct;
use loop_extract::gzip::Limits;
use loop_extract::input::SourceFile;
use loop_extract::render::markdown;

#[derive(Debug, serde::Deserialize)]
struct Expectations {
    /// Text of the tree we believe is the document body.
    body_contains: Option<Vec<String>>,
    body_total_chars: Option<usize>,
    body_segment_count: Option<usize>,
    /// Text of the tree we believe is the document title.
    title: Option<String>,
    merge_trees: Option<usize>,
    has_bold: Option<bool>,
    has_lists: Option<bool>,
    /// Rendered Markdown must start with exactly this line.
    markdown_first_line: Option<String>,
    /// Rendered Markdown must contain each of these lines exactly.
    markdown_lines: Option<Vec<String>>,
    /// Rendered Markdown must contain each of these substrings.
    markdown_contains: Option<Vec<String>>,
    /// Deepest list nesting the Markdown must reach, counted in levels.
    markdown_min_list_depth: Option<usize>,
    /// Which source the body must come from: `snapshot`, `operations` or
    /// `snapshot-plus-operations`.
    content_source: Option<String>,
    /// The tree role classification must come from the container framing.
    structurally_classified: Option<bool>,
    /// Text of the title tree the framing identifies.
    structural_title: Option<String>,
    /// Number of trees the framing places under embedded components.
    embedded_component_trees: Option<usize>,
}

fn corpus_files() -> Option<Vec<PathBuf>> {
    let dir = std::env::var("LOOP_EXTRACT_TEST_CORPUS").ok()?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("LOOP_EXTRACT_TEST_CORPUS={dir} cannot be read: {e}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("loop") | Some("fluid")
            )
        })
        .collect();
    files.sort();
    Some(files)
}

fn analyse(path: &Path) -> Analysis {
    let source = SourceFile::load(path).expect("corpus file loads");
    Analysis::run(&source, Limits::default())
}

fn tree_text(tree: &loop_extract::fluid::merge_tree::MergeTree) -> String {
    tree.segments.iter().filter_map(|s| s.text()).collect()
}

/// Locates the expectation file for a corpus file, if there is one.
///
/// `tests/expectations/private/` is checked first. Expectation files that name
/// or quote real documents live there and are not committed; the committed
/// directory holds the schema and a synthetic example.
fn expectations_for(name: &str) -> Option<PathBuf> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expectations");
    [
        base.join("private").join(format!("{name}.json")),
        base.join(format!("{name}.json")),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn skip_notice() {
    eprintln!(
        "skipped: set LOOP_EXTRACT_TEST_CORPUS to a directory of .loop/.fluid files to run \
         corpus regression tests"
    );
}

#[test]
fn every_corpus_file_is_recognised_and_yields_a_report() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };
    assert!(
        !files.is_empty(),
        "the corpus directory holds no .loop or .fluid files"
    );

    for path in files {
        let analysis = analyse(&path);
        let name = path.file_name().unwrap().to_string_lossy();

        assert!(
            !analysis.members.is_empty(),
            "{name}: no gzip member inflated, so the file was not read at all"
        );
        assert_ne!(
            analysis.assessment.confidence,
            loop_extract::container::Confidence::None,
            "{name}: not recognised as a Fluid container"
        );
        // Every signature is accounted for: inflated or explicitly rejected.
        assert_eq!(
            analysis.gzip_candidates,
            analysis.members.len() + analysis.failed_candidates.len(),
            "{name}: gzip candidates unaccounted for"
        );
    }
}

#[test]
fn extraction_is_deterministic() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        let first = analyse(&path);
        let second = analyse(&path);

        let hashes = |a: &Analysis| a.trees.iter().map(|t| t.content_hash).collect::<Vec<_>>();
        let codes = |a: &Analysis| a.warnings.iter().map(|w| w.code).collect::<Vec<_>>();
        let texts = |a: &Analysis| a.trees.iter().map(tree_text).collect::<Vec<_>>();

        assert_eq!(
            hashes(&first),
            hashes(&second),
            "{name}: tree hashes differ between runs"
        );
        assert_eq!(
            codes(&first),
            codes(&second),
            "{name}: warnings differ between runs"
        );
        assert_eq!(
            texts(&first),
            texts(&second),
            "{name}: extracted text differs between runs"
        );
    }
}

#[test]
fn no_tree_is_left_incomplete_without_a_warning() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        let analysis = analyse(&path);
        for tree in &analysis.trees {
            if !tree.complete {
                assert!(
                    analysis
                        .warnings
                        .iter()
                        .any(|w| w.code == "tree_incomplete"),
                    "{name}: an incomplete tree was not reported"
                );
            }
        }
    }
}

#[test]
fn byte_identical_trees_never_produce_duplicated_text() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        let analysis = analyse(&path);

        // Every retained tree has a distinct content hash: the whole point of
        // deduplication. Repeated copies show up as `copies`, not extra trees.
        let mut seen: BTreeMap<[u8; 32], usize> = BTreeMap::new();
        for tree in &analysis.trees {
            *seen.entry(tree.content_hash).or_default() += 1;
        }
        assert!(
            seen.values().all(|count| *count == 1),
            "{name}: the same tree was retained more than once"
        );
    }
}

#[test]
fn assembled_segment_counts_match_the_declared_totals() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        let analysis = analyse(&path);
        for tree in &analysis.trees {
            if let Some(total) = tree.key.total_segment_count {
                if tree.complete {
                    assert_eq!(
                        tree.segments.len(),
                        total,
                        "{name}: a tree reported complete has the wrong segment count"
                    );
                }
            }
        }
    }
}

#[test]
fn known_files_match_their_recorded_expectations() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };
    let mut checked = 0usize;

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Some(expectations_path) = expectations_for(&name) else {
            continue;
        };
        checked += 1;
        let expected: Expectations =
            serde_json::from_str(&std::fs::read_to_string(&expectations_path).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e}", expectations_path.display()));
        let analysis = analyse(&path);

        if let Some(count) = expected.merge_trees {
            assert_eq!(analysis.trees.len(), count, "{name}: merge tree count");
        }

        let body = analysis.body_candidate();
        if let Some(total) = expected.body_total_chars {
            let body = body.expect("a body candidate was expected");
            assert_eq!(
                body.key.total_length_chars,
                Some(total),
                "{name}: body character count"
            );
        }
        if let Some(count) = expected.body_segment_count {
            let body = body.expect("a body candidate was expected");
            assert_eq!(body.segments.len(), count, "{name}: body segment count");
        }
        if let Some(fragments) = &expected.body_contains {
            let body = body.expect("a body candidate was expected");
            let text = tree_text(body);
            for fragment in fragments {
                assert!(
                    text.contains(fragment),
                    "{name}: body is missing {fragment:?}"
                );
            }
        }
        if let Some(title) = &expected.title {
            let found = analysis
                .title_candidates()
                .map(tree_text)
                .any(|candidate| candidate.trim() == title);
            assert!(found, "{name}: no title candidate reads {title:?}");
        }
        if let Some(bold) = expected.has_bold {
            let body = body.expect("a body candidate was expected");
            assert_eq!(body.evidence.has_bold, bold, "{name}: bold formatting");
        }
        if let Some(lists) = expected.has_lists {
            let body = body.expect("a body candidate was expected");
            assert_eq!(
                body.evidence.has_list_reference, lists,
                "{name}: list structure"
            );
        }
        if let Some(structural) = expected.structurally_classified {
            assert_eq!(
                analysis.has_structural_classification(),
                structural,
                "{name}: classification source"
            );
        }
        if let Some(title) = &expected.structural_title {
            let found = analysis
                .trees
                .iter()
                .find(|tree| tree.role == loop_extract::fluid::merge_tree::TreeRole::Title)
                .map(tree_text);
            assert_eq!(
                found.as_deref().map(str::trim),
                Some(title.as_str()),
                "{name}: the framing should name exactly this title"
            );
        }
        if let Some(count) = expected.embedded_component_trees {
            let found = analysis
                .trees
                .iter()
                .filter(|tree| {
                    tree.role == loop_extract::fluid::merge_tree::TreeRole::EmbeddedComponent
                })
                .count();
            assert_eq!(found, count, "{name}: embedded component trees");
        }
    }

    eprintln!("checked {checked} file(s) against recorded expectations");
}

/// Deepest bullet nesting in rendered Markdown, counted in four-space levels.
fn deepest_list_level(rendered: &str) -> usize {
    rendered
        .lines()
        .filter(|line| line.trim_start().starts_with("- "))
        .map(|line| (line.len() - line.trim_start().len()) / 4)
        .max()
        .unwrap_or(0)
}

#[test]
fn rendered_markdown_matches_recorded_expectations() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Some(expectations_path) = expectations_for(&name) else {
            continue;
        };
        let expected: Expectations =
            serde_json::from_str(&std::fs::read_to_string(&expectations_path).unwrap()).unwrap();
        let analysis = analyse(&path);
        let (document, _) = reconstruct(&analysis);
        let rendered = markdown::render(&document);

        if let Some(first) = &expected.markdown_first_line {
            assert_eq!(
                rendered.lines().next().unwrap_or_default(),
                first,
                "{name}: first Markdown line"
            );
        }
        if let Some(lines) = &expected.markdown_lines {
            for line in lines {
                assert!(
                    rendered.lines().any(|rendered_line| rendered_line == line),
                    "{name}: no rendered line equals {line:?}"
                );
            }
        }
        if let Some(fragments) = &expected.markdown_contains {
            for fragment in fragments {
                assert!(
                    rendered.contains(fragment),
                    "{name}: Markdown is missing {fragment:?}"
                );
            }
        }
        if let Some(source) = &expected.content_source {
            let actual = serde_json::to_value(document.metadata.content_source).unwrap();
            assert_eq!(
                actual.as_str(),
                Some(source.as_str()),
                "{name}: content source"
            );
        }
        if let Some(depth) = expected.markdown_min_list_depth {
            assert!(
                deepest_list_level(&rendered) >= depth,
                "{name}: list nesting only reached level {}, expected at least {depth}",
                deepest_list_level(&rendered)
            );
        }
    }
}

#[test]
fn rendering_never_panics_on_any_corpus_file() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        let analysis = analyse(&path);
        let (document, _) = reconstruct(&analysis);

        let md = markdown::render(&document);
        let txt = loop_extract::render::text::render(&document);
        let json = loop_extract::render::json::render(
            &document,
            &analysis,
            &[],
            loop_extract::render::json::Options {
                include_attribution: false,
            },
        )
        .unwrap_or_else(|e| panic!("{name}: JSON rendering failed: {e}"));

        // Rust strings are UTF-8 by construction; this asserts the output is
        // never left half-formed.
        assert!(
            md.is_empty() || md.ends_with('\n'),
            "{name}: Markdown must end with a newline"
        );
        assert!(
            txt.is_empty() || txt.ends_with('\n'),
            "{name}: text must end with a newline"
        );
        assert!(json.starts_with('{'), "{name}: JSON output is malformed");
    }
}

#[test]
fn an_ordinary_export_never_carries_personal_data() {
    let Some(files) = corpus_files() else {
        return skip_notice();
    };

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        let analysis = analyse(&path);
        let (document, _) = reconstruct(&analysis);

        let json = loop_extract::render::json::render(
            &document,
            &analysis,
            &[],
            loop_extract::render::json::Options {
                include_attribution: false,
            },
        )
        .unwrap();

        // An attribution record always carries `oid` and `dataSource` alongside
        // the address; their absence shows the records were removed.
        assert!(
            !json.contains("\"oid\""),
            "{name}: default JSON leaked an object id"
        );
        assert!(
            !json.contains("\"dataSource\""),
            "{name}: default JSON leaked an attribution record"
        );
        assert!(
            !json.contains('@') || !json.contains(".com\""),
            "{name}: default JSON may have leaked an address"
        );
    }
}
