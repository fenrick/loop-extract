//! The command-line surface: help, version, exit codes and the messages that
//! distinguish one failure from another.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_loop-extract")
}

fn run(args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .output()
        .expect("the binary runs")
}

fn scratch(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join("loop-extract-cli-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn version_reports_the_crate_version() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert!(
        stdout(&output).contains(env!("CARGO_PKG_VERSION")),
        "expected the version in {:?}",
        stdout(&output)
    );
}

#[test]
fn help_lists_both_commands_and_the_offline_promise() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("extract"));
    assert!(text.contains("inspect"));
    assert!(
        text.contains("no Microsoft 365"),
        "the offline promise should be stated: {text}"
    );
}

#[test]
fn every_option_is_documented() {
    for command in ["extract", "inspect"] {
        let text = stdout(&run(&[command, "--help"]));
        assert!(
            text.contains("--format"),
            "{command}: --format undocumented"
        );
        assert!(
            text.contains("--max-member-bytes"),
            "{command}: byte limits undocumented"
        );
    }
    // The reverse-engineering option explains its own file naming.
    let inspect = stdout(&run(&["inspect", "--help"]));
    assert!(
        inspect.contains("member-000.raw"),
        "--dump-payloads should show its naming"
    );
    // The privacy default explains itself.
    let extract = stdout(&run(&["extract", "--help"]));
    assert!(extract.contains("--include-attribution"));
    assert!(
        extract.contains("Off by default"),
        "the privacy default should say why"
    );
}

#[test]
fn a_file_that_is_not_a_container_fails_with_a_reason() {
    let path = scratch("plain.loop", b"this is not a Loop file at all");

    let output = run(&["extract", path.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.contains("is not a Microsoft Loop or Fluid container"),
        "unexpected message: {message}"
    );
    assert!(
        message.contains("no gzip member could be inflated"),
        "should say why: {message}"
    );
    assert!(
        message.contains("inspect"),
        "should point at inspect: {message}"
    );
    assert!(
        stdout(&output).is_empty(),
        "a failure must not write a document to stdout"
    );
}

#[test]
fn a_gzip_file_with_no_fluid_payload_fails_distinctly() {
    // Inflates, but holds no embedded JSON: a different failure from the above,
    // and the message has to say so.
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, b"just some gzipped text").unwrap();
    let path = scratch("gzipped.loop", &encoder.finish().unwrap());

    let output = run(&["extract", path.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("none contained an embedded JSON payload"),
        "unexpected message: {}",
        stderr(&output)
    );
}

#[test]
fn an_empty_file_fails_before_anything_else() {
    let path = scratch("empty.loop", b"");

    let output = run(&["extract", path.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("is empty"),
        "unexpected message: {}",
        stderr(&output)
    );
}

#[test]
fn a_missing_file_fails_with_the_path() {
    let output = run(&["extract", "/nonexistent/no-such-page.loop"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no-such-page.loop"));
}

#[test]
fn inspect_reports_an_unrecognised_file_rather_than_failing() {
    // `inspect` exists to describe whatever it is given, including something
    // that is not a Loop file.
    let path = scratch("plain-inspect.loop", b"not a Loop file");

    let output = run(&["inspect", path.to_str().unwrap()]);

    assert!(output.status.success(), "inspect should report, not fail");
    assert!(
        stdout(&output).contains("Unrecognised"),
        "got: {}",
        stdout(&output)
    );
}

#[test]
fn an_unknown_subcommand_is_rejected_with_usage() {
    let output = run(&["frobnicate"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("Usage"));
}

/// Every corpus file is a valid container, so extraction must succeed, and any
/// file that yields nothing must say which kind of nothing it is.
#[test]
fn corpus_files_extract_successfully_and_explain_any_emptiness() {
    let Ok(dir) = std::env::var("LOOP_EXTRACT_TEST_CORPUS") else {
        eprintln!("skipped: set LOOP_EXTRACT_TEST_CORPUS to run this");
        return;
    };
    let mut checked = 0usize;
    for entry in std::fs::read_dir(Path::new(&dir)).expect("the corpus directory reads") {
        let path = entry.unwrap().path();
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("loop") | Some("fluid")
        ) {
            continue;
        }
        checked += 1;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let output = run(&["extract", path.to_str().unwrap()]);

        assert!(
            output.status.success(),
            "{name}: extraction failed: {}",
            stderr(&output)
        );

        if stdout(&output).trim().is_empty() {
            assert!(
                stderr(&output).contains("note:"),
                "{name}: produced no output and did not say why"
            );
        }
    }
    assert!(
        checked > 0,
        "the corpus directory holds no .loop or .fluid files"
    );
}
