//! loop-extract: a local extractor for Microsoft Loop `.loop` and Fluid
//! `.fluid` containers.
//!
//! The container format is undocumented. Everything this crate knows about it
//! was established by reading a corpus of real files. Every such conclusion is
//! marked in the source as either an observation ("Observed in test fixtures")
//! or an assumption ("ASSUMPTION, not confirmed"), and the `inspect` command
//! exists so that a future format change is visible rather than silent.
//!
//! Layering, deliberately kept separate so parsing never depends on rendering:
//!
//! ```text
//! binary file      input
//!     v
//! gzip members     gzip
//!     v
//! embedded payloads  discovery
//!     v
//! merge trees      fluid
//!     v
//! document model   document      (stage 2)
//!     v
//! markdown/text/json  render     (stage 3)
//! ```

pub mod analysis;
pub mod cli;
pub mod container;
pub mod discovery;
pub mod document;
pub mod fluid;
pub mod gzip;
pub mod input;
pub mod inspect;
pub mod operations;
pub mod render;
pub mod warning;
