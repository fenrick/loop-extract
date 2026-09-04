//! Command-line interface.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "loop-extract",
    version,
    about = "Extracts document content from Microsoft Loop (.loop) and Fluid (.fluid) files",
    long_about = "Reads a Loop or Fluid container locally and reconstructs its document \
                  content. Requires no Microsoft 365, OneDrive, SharePoint, Microsoft Graph \
                  or network access."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Reconstruct the document and write it out. Markdown by default.
    Extract(ExtractArgs),
    /// Report what a container holds, without reconstructing a document.
    Inspect(InspectArgs),
}

#[derive(Debug, Parser)]
pub struct ExtractArgs {
    /// The .loop or .fluid file to read.
    pub file: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub format: OutputFormat,

    /// Write to this file instead of standard output.
    #[arg(long, short, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Keep author names, email addresses, object ids and edit timestamps.
    ///
    /// Off by default: Loop records an author on nearly every segment, and an
    /// ordinary export should not carry a colleague's email on every paragraph.
    /// Only the JSON format can carry attribution at all.
    #[arg(long)]
    pub include_attribution: bool,

    /// Largest decompressed size accepted from a single gzip member, in bytes.
    #[arg(long, default_value_t = 64 * 1024 * 1024)]
    pub max_member_bytes: u64,

    /// Largest total decompressed size accepted from one file, in bytes.
    #[arg(long, default_value_t = 512 * 1024 * 1024)]
    pub max_total_bytes: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Markdown,
    Text,
    Json,
}

#[derive(Debug, Parser)]
pub struct InspectArgs {
    /// The .loop or .fluid file to inspect.
    pub file: PathBuf,

    /// Report format.
    ///
    /// `text` is written for a person to read. `json` carries the same
    /// information for a script, and is the form to keep when recording what a
    /// file looked like before a Loop format change.
    #[arg(long, value_enum, default_value_t = ReportFormat::Text)]
    pub format: ReportFormat,

    /// Write every inflated member and discovered JSON payload to this directory.
    ///
    /// This is the reverse-engineering tool. Names are deterministic, so a diff
    /// between two runs, or between two Loop versions, is meaningful:
    ///
    ///   member-000.raw                  one inflated gzip member, verbatim
    ///   member-000-fragment-000.json    a JSON payload found inside it
    ///
    /// The dump contains the document's full text and its authors' names and
    /// email addresses. Treat the directory as you would the source file.
    #[arg(long, value_name = "DIR")]
    pub dump_payloads: Option<PathBuf>,

    /// Include per-member detail and per-tree source locations.
    ///
    /// Adds a row per gzip member (offset, compressed and decompressed size,
    /// whether it is UTF-8, payloads found) and, for each merge tree, every
    /// place a copy of it was found.
    #[arg(long, short)]
    pub verbose: bool,

    /// Largest decompressed size accepted from a single gzip member, in bytes.
    ///
    /// A member that reaches the limit is truncated and reported, rather than
    /// being allowed to exhaust memory. The largest member in the reference
    /// corpus inflates to about 0.5 MB, so the default is generous.
    #[arg(long, default_value_t = 64 * 1024 * 1024)]
    pub max_member_bytes: u64,

    /// Largest total decompressed size accepted from one file, in bytes.
    ///
    /// Once reached, no further members are inflated and a warning says so.
    #[arg(long, default_value_t = 512 * 1024 * 1024)]
    pub max_total_bytes: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReportFormat {
    Text,
    Json,
}
