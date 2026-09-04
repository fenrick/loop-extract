//! Command-line entry point for loop-extract.

use std::io::Write;

use anyhow::{Context, Result};
use clap::Parser;

use anyhow::bail;
use loop_extract::analysis::Analysis;
use loop_extract::cli::{Cli, Command, ExtractArgs, InspectArgs, OutputFormat, ReportFormat};
use loop_extract::container::Confidence;
use loop_extract::document::model::Document;
use loop_extract::document::reconstruct::reconstruct;
use loop_extract::gzip;
use loop_extract::input::SourceFile;
use loop_extract::inspect::{dump_payloads, InspectReport};
use loop_extract::render::{json, markdown, text};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Extract(args) => run_extract(args),
        Command::Inspect(args) => run_inspect(args),
    }
}

fn run_extract(args: ExtractArgs) -> Result<()> {
    let source = SourceFile::load(&args.file)?;
    let limits = gzip::Limits {
        max_member_bytes: args.max_member_bytes,
        max_total_bytes: args.max_total_bytes,
    };
    let analysis = Analysis::run(&source, limits);

    // A file with no Fluid structure at all is a hard error, not an empty
    // document. Exiting 0 with empty output makes "this is not a Loop file"
    // indistinguishable from "this Loop page is empty" in a script.
    if analysis.assessment.confidence == Confidence::None {
        bail!(
            "{} is not a Microsoft Loop or Fluid container: {}. \
             Run `loop-extract inspect` on it for the detail.",
            args.file.display(),
            describe_absence(&analysis)
        );
    }

    let (document, warnings) = reconstruct(&analysis);

    if args.include_attribution && !matches!(args.format, OutputFormat::Json) {
        eprintln!(
            "note: --include-attribution affects the json format only; markdown and text never \
             carry attribution"
        );
    }

    let rendered = match args.format {
        OutputFormat::Markdown => markdown::render(&document),
        OutputFormat::Text => text::render(&document),
        OutputFormat::Json => json::render(
            &document,
            &analysis,
            &warnings,
            json::Options {
                include_attribution: args.include_attribution,
            },
        )?,
    };

    // Diagnostics go to stderr so stdout carries only the document.
    for warning in analysis.warnings.iter().chain(warnings.iter()) {
        eprintln!("warning [{}]: {}", warning.code, warning.message);
    }

    // Recognised, but nothing to show. That is a legitimate outcome for an
    // empty page, so it is reported rather than treated as a failure.
    if document.visible_blocks().next().is_none() && document.title.is_none() {
        eprintln!("note: {}", describe_empty(&analysis, &document));
    }

    match &args.output {
        Some(path) => {
            std::fs::write(path, rendered).with_context(|| format!("writing {}", path.display()))?
        }
        None => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            out.write_all(rendered.as_bytes())?;
        }
    }
    Ok(())
}

fn run_inspect(args: InspectArgs) -> Result<()> {
    let source = SourceFile::load(&args.file)?;
    let limits = gzip::Limits {
        max_member_bytes: args.max_member_bytes,
        max_total_bytes: args.max_total_bytes,
    };
    let analysis = Analysis::run(&source, limits);

    if let Some(dir) = &args.dump_payloads {
        let written = dump_payloads(&analysis, dir)?;
        // Diagnostics go to stderr so stdout carries only the report.
        eprintln!("wrote {written} file(s) to {}", dir.display());
    }

    let report = InspectReport::build(&analysis);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match args.format {
        ReportFormat::Json => writeln!(out, "{}", report.to_json()?)?,
        ReportFormat::Text => report.write_text(&mut out, args.verbose)?,
    }
    Ok(())
}

/// Why a file was not recognised, in the terms the scan actually established.
fn describe_absence(analysis: &Analysis) -> String {
    if analysis.members.is_empty() {
        return format!(
            "no gzip member could be inflated from {} byte(s) ({} signature(s) tried)",
            analysis.file_size, analysis.gzip_candidates
        );
    }
    format!(
        "{} gzip member(s) inflated but none contained an embedded JSON payload",
        analysis.members.len()
    )
}

/// Which kind of empty a recognised container is, so the two are not confused.
fn describe_empty(analysis: &Analysis, document: &Document) -> String {
    if analysis.trees.is_empty() {
        return "the container was read but holds no merge tree, so there is no document \
                structure to reconstruct"
            .to_string();
    }
    let text_trees = analysis
        .trees
        .iter()
        .filter(|t| t.evidence.text_segments > 0)
        .count();
    if text_trees == 0 && analysis.operations.counts.inserts == 0 {
        return format!(
            "the page is empty: {} merge tree(s) hold only structure, and the operation log \
             carries no text to replay",
            analysis.trees.len()
        );
    }
    if text_trees > 0 {
        return format!(
            "{} merge tree(s) carry text, but none of them is the page body; the text belongs \
             to embedded components, which are not yet rendered. Use --format json to see it",
            text_trees
        );
    }
    format!(
        "no document text was recovered from {} merge tree(s) or {} operation(s)",
        analysis.trees.len(),
        document.metadata.body_segment_count
    )
}
