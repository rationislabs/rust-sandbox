//! A key-value store on an append-only, checksummed write-ahead log.

// The library only serves this crate's binary: no `#[must_use]` / `# Errors` boilerplate.
#![allow(clippy::must_use_candidate, clippy::missing_errors_doc)]

pub mod record;
