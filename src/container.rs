//! Deciding whether a file is a Loop/Fluid container, and what produced it.
//!
//! The original brief proposed validating files by searching for identifiers
//! such as `Microsoft.Prague`. That test was checked against the whole corpus
//! and rejected: `Microsoft.Prague` occurs zero times in all 13 files, spanning
//! Oct 2023 to Sep 2026. Those identifiers may belong to a different Loop
//! generation, but they are not a usable validator.
//!
//! Identification is therefore structural, with component identity reported as
//! supporting evidence rather than required. A missing or unknown component
//! never fails the file. Nor does the filename extension ever count as evidence.

use serde::Serialize;

use crate::discovery::PayloadCandidate;

/// Component names observed across the corpus.
///
/// `@fluidx/loop-page-container` is written by Loop from late 2024 onwards.
/// `@ms/office-fluid-container` is written by the older `.fluid` files and by
/// an older `.loop` page from Oct 2023. Both identify a genuine Loop/Fluid page.
pub const LOOP_PAGE_CONTAINER: &str = "@fluidx/loop-page-container";
pub const OFFICE_FLUID_CONTAINER: &str = "@ms/office-fluid-container";

pub const KNOWN_COMPONENTS: &[&str] = &[LOOP_PAGE_CONTAINER, OFFICE_FLUID_CONTAINER];

#[derive(Debug, Clone, Serialize)]
pub struct ComponentIdentity {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    High,
    Moderate,
    Low,
    None,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatAssessment {
    /// Human-readable format label.
    pub format: String,
    pub components: Vec<ComponentIdentity>,
    pub confidence: Confidence,
}

impl FormatAssessment {
    /// True when the file carries a recognised Loop/Fluid component.
    pub fn is_known_loop_component(&self) -> bool {
        self.components
            .iter()
            .any(|c| KNOWN_COMPONENTS.contains(&c.name.as_str()))
    }
}

/// Extracts component identity from payloads discovered by the `"package"`
/// anchor.
///
/// Observed shape:
/// `{"package":{"name":"@fluidx/loop-page-container","version":"0.0.1", ...}}`
///
/// The version is a build identifier, not semantic: values `0.0.1` and
/// `20250129024` both occur in the corpus.
pub fn components(payloads: &[PayloadCandidate]) -> Vec<ComponentIdentity> {
    let mut found: Vec<ComponentIdentity> = Vec::new();
    for payload in payloads {
        let Some(package) = payload.value.get("package") else {
            continue;
        };
        let Some(name) = package.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let version = package
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if found.iter().any(|c| c.name == name && c.version == version) {
            continue;
        }
        found.push(ComponentIdentity {
            name: name.to_string(),
            version,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    found
}

/// Assesses format from structural evidence plus component identity.
///
/// * gzip members that inflate, and embedded JSON payloads, are structural
///   evidence of a Fluid snapshot container;
/// * merge-tree chunks raise that to strong structural evidence;
/// * a recognised component name raises confidence to high.
pub fn assess(
    inflated_members: usize,
    json_payloads: usize,
    merge_tree_chunks: usize,
    components: Vec<ComponentIdentity>,
) -> FormatAssessment {
    let structural = inflated_members > 0 && json_payloads > 0;
    let strong_structural = structural && merge_tree_chunks > 0;
    let known_component = components
        .iter()
        .any(|c| KNOWN_COMPONENTS.contains(&c.name.as_str()));

    let (format, confidence) = match (strong_structural, structural, known_component) {
        (true, _, true) => (
            "Microsoft Loop / Fluid snapshot (probable)",
            Confidence::High,
        ),
        (true, _, false) => ("Fluid-compatible snapshot", Confidence::Moderate),
        (false, true, true) => (
            "Microsoft Loop container, no document content found",
            Confidence::Moderate,
        ),
        (false, true, false) => (
            "Fluid-compatible container, no document content found",
            Confidence::Low,
        ),
        _ => (
            "Unrecognised: no Fluid snapshot structure found",
            Confidence::None,
        ),
    };

    FormatAssessment {
        format: format.to_string(),
        components,
        confidence,
    }
}
