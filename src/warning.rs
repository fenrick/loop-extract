//! Recoverable-condition reporting.
//!
//! The extractor never aborts because one embedded payload is unreadable. Every
//! recoverable condition becomes a `Warning` carried through to the report, so a
//! format change shows up as diagnostics rather than as silently missing text.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    /// Stable machine-readable code. Use these in tests rather than the message text.
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_index: Option<usize>,
    /// Byte offset into the source file, where one applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_offset: Option<usize>,
}

impl Warning {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            member_index: None,
            file_offset: None,
        }
    }

    pub fn at_member(mut self, index: usize, file_offset: usize) -> Self {
        self.member_index = Some(index);
        self.file_offset = Some(file_offset);
        self
    }
}

/// Accumulates warnings in discovery order.
#[derive(Debug, Default)]
pub struct Warnings(Vec<Warning>);

impl Warnings {
    pub fn push(&mut self, warning: Warning) {
        self.0.push(warning);
    }

    pub fn extend(&mut self, other: impl IntoIterator<Item = Warning>) {
        self.0.extend(other);
    }

    pub fn into_vec(self) -> Vec<Warning> {
        self.0
    }

    pub fn as_slice(&self) -> &[Warning] {
        &self.0
    }
}
