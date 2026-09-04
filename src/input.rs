//! Source file loading.
//!
//! Loading is one of the few hard-error paths: an unreadable or empty input
//! cannot produce any output, so it fails immediately rather than warning.

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum InputError {
    #[error("cannot read {path}: {source}")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is empty")]
    Empty { path: PathBuf },
}

#[derive(Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

impl SourceFile {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, InputError> {
        let path = path.as_ref().to_path_buf();
        let bytes = std::fs::read(&path).map_err(|source| InputError::Unreadable {
            path: path.clone(),
            source,
        })?;
        if bytes.is_empty() {
            return Err(InputError::Empty { path });
        }
        Ok(Self { path, bytes })
    }

    /// Size of the file in bytes.
    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    /// File extension in lower case, without the dot. Used for reporting only:
    /// extension is never treated as evidence that a file is a Loop container.
    pub fn extension(&self) -> String {
        self.path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }
}
