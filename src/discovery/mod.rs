//! Finding parseable payloads inside a decompressed gzip member.
//!
//! Observed in test fixtures:
//! A decompressed member is *not* JSON. It is a binary-framed container that
//! embeds JSON documents as blob payloads, with arbitrary binary bytes before
//! and after each one. Parsing a whole member with `serde_json` fails on the
//! first framing byte.
//!
//! Two discovery strategies are therefore possible, behind one interface:
//!
//! * [`fragment_scanner::FragmentScanner`] - locates JSON by anchor field names
//!   and brace matching. Tolerant, format-change resistant, but blind to
//!   anything it has no anchor for.
//! * [`envelope::EnvelopeDecoder`] - decodes the binary framing into named
//!   blobs. Not implemented; see that module for what is known so far.
//!
//! This is reverse-engineered behaviour and is not based on a published
//! Microsoft Prague/Fluid file-format specification.

pub mod envelope;
pub mod fragment_scanner;

use serde_json::Value;
use thiserror::Error;

/// A JSON document recovered from inside a member.
#[derive(Debug, Clone)]
pub struct PayloadCandidate {
    pub member_index: usize,
    /// Byte range within the *decompressed* member.
    pub start: usize,
    pub end: usize,
    /// Which anchor led to this payload. Diagnostic only.
    pub anchor: &'static str,
    pub value: Value,
}

impl PayloadCandidate {
    /// Size in bytes of the JSON slice this payload was parsed from.
    pub fn byte_len(&self) -> usize {
        self.end - self.start
    }
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("{0}")]
    Unsupported(String),
}

/// A strategy for locating parseable payloads inside one decompressed member.
pub trait PayloadDiscovery {
    /// Name used in diagnostics to record which strategy produced a payload.
    fn name(&self) -> &'static str;

    fn discover(
        &self,
        member_index: usize,
        member: &[u8],
    ) -> Result<Vec<PayloadCandidate>, DiscoveryError>;
}
