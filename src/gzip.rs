//! Embedded gzip member discovery and bounded decompression.
//!
//! Observed in test fixtures (13 files, Oct 2023 - Sep 2026):
//! A `.loop` / `.fluid` container holds many independent gzip members laid out
//! end to end. Members are found by scanning for the gzip magic `1F 8B 08`
//! rather than by following a container index, because the container framing is
//! not yet decoded.
//!
//! This is reverse-engineered behaviour and is not based on a published
//! Microsoft Prague/Fluid file-format specification.
//!
//! Assumption (not confirmed): a byte sequence `1F 8B 08` that fails to inflate
//! is a coincidental match inside compressed data, not a corrupt member. Either
//! way it is recorded as a failed candidate and skipped.

use std::io::Read;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::warning::Warning;

/// Bounds on decompression, to keep a malformed or hostile file from exhausting
/// memory. These are generous relative to the observed corpus, whose largest
/// single member decompresses to about 0.5 MB.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_member_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_member_bytes: 64 * 1024 * 1024,
            max_total_bytes: 512 * 1024 * 1024,
        }
    }
}

/// A gzip signature that was found and successfully inflated.
#[derive(Debug)]
pub struct GzipMember {
    pub index: usize,
    /// Byte offset of the `1F 8B 08` signature within the source file.
    pub file_offset: usize,
    /// Bytes consumed from the file by this member, when the decoder reported it.
    pub compressed_len: Option<usize>,
    pub data: Vec<u8>,
    /// True when the member hit `max_member_bytes` and `data` is incomplete.
    pub truncated: bool,
    /// True when inflation errored partway and `data` holds only what was recovered.
    pub partial: bool,
    pub sha256: [u8; 32],
}

impl GzipMember {
    pub fn is_utf8(&self) -> bool {
        std::str::from_utf8(&self.data).is_ok()
    }
}

/// A signature that did not inflate.
#[derive(Debug, Clone, Serialize)]
pub struct FailedCandidate {
    pub file_offset: usize,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub members: Vec<GzipMember>,
    pub failed: Vec<FailedCandidate>,
    /// Every `1F 8B 08` offset found, whether or not it inflated.
    pub candidate_offsets: Vec<usize>,
    pub warnings: Vec<Warning>,
}

const GZIP_MAGIC: [u8; 3] = [0x1f, 0x8b, 0x08];

/// Finds every gzip signature in `data` and attempts to inflate each one.
///
/// Candidates are independent: a failure never stops the scan.
pub fn scan(data: &[u8], limits: Limits) -> ScanResult {
    let mut result = ScanResult::default();
    let mut total_out: u64 = 0;
    let finder = memchr::memmem::Finder::new(&GZIP_MAGIC);

    for file_offset in finder.find_iter(data) {
        result.candidate_offsets.push(file_offset);

        if total_out >= limits.max_total_bytes {
            result.warnings.push(
                Warning::new(
                    "decompression_budget_exhausted",
                    format!(
                        "stopped inflating at offset {file_offset}: total decompressed output \
                         reached the {} byte limit",
                        limits.max_total_bytes
                    ),
                )
                .at_member(result.members.len(), file_offset),
            );
            break;
        }

        let index = result.members.len();
        match inflate_member(data, file_offset, limits.max_member_bytes) {
            Ok(inflated) => {
                if inflated.data.is_empty() {
                    result.failed.push(FailedCandidate {
                        file_offset,
                        error: "inflated to zero bytes".to_string(),
                    });
                    continue;
                }
                if inflated.truncated {
                    result.warnings.push(
                        Warning::new(
                            "member_truncated",
                            format!(
                                "member reached the {} byte per-member limit and was truncated",
                                limits.max_member_bytes
                            ),
                        )
                        .at_member(index, file_offset),
                    );
                }
                if inflated.partial {
                    result.warnings.push(
                        Warning::new(
                            "member_partially_inflated",
                            format!(
                                "member failed partway through inflation; recovered {} bytes: {}",
                                inflated.data.len(),
                                inflated.error.as_deref().unwrap_or("unknown error")
                            ),
                        )
                        .at_member(index, file_offset),
                    );
                }
                total_out += inflated.data.len() as u64;
                let sha256 = Sha256::digest(&inflated.data).into();
                result.members.push(GzipMember {
                    index,
                    file_offset,
                    compressed_len: inflated.compressed_len,
                    data: inflated.data,
                    truncated: inflated.truncated,
                    partial: inflated.partial,
                    sha256,
                });
            }
            Err(error) => {
                result.failed.push(FailedCandidate { file_offset, error });
            }
        }
    }

    result
}

struct Inflated {
    data: Vec<u8>,
    compressed_len: Option<usize>,
    truncated: bool,
    partial: bool,
    error: Option<String>,
}

/// Inflates one member starting at `file_offset`.
///
/// Returns `Err` only when nothing at all could be recovered, which is the
/// expected outcome for a coincidental signature match.
fn inflate_member(data: &[u8], file_offset: usize, max_bytes: u64) -> Result<Inflated, String> {
    let mut remaining: &[u8] = &data[file_offset..];
    let mut out = Vec::new();

    // `bufread::GzDecoder` consumes exactly the bytes of one member, which lets
    // us recover the compressed length from what is left of the slice.
    let mut decoder = flate2::bufread::GzDecoder::new(&mut remaining);
    let read_result = decoder.by_ref().take(max_bytes).read_to_end(&mut out);
    drop(decoder);

    let consumed = data.len() - file_offset - remaining.len();
    let compressed_len = (consumed > 0).then_some(consumed);

    match read_result {
        Ok(_) => {
            let truncated = out.len() as u64 >= max_bytes;
            Ok(Inflated {
                data: out,
                compressed_len,
                truncated,
                partial: false,
                error: None,
            })
        }
        Err(error) => {
            if out.is_empty() {
                Err(error.to_string())
            } else {
                Ok(Inflated {
                    data: out,
                    compressed_len,
                    truncated: false,
                    partial: true,
                    error: Some(error.to_string()),
                })
            }
        }
    }
}
