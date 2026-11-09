//! A key-value store on an append-only, checksummed write-ahead log.

// The library only serves this crate's binary: no `#[must_use]` / `# Errors` boilerplate.
#![allow(clippy::must_use_candidate, clippy::missing_errors_doc)]

pub mod record;
pub mod store;

pub use store::{Error, Store};

/// The value the hidden `stress` mode writes for key `i`: deterministic, 1 to 2048 bytes.
#[doc(hidden)]
pub fn stress_value(i: u64) -> Vec<u8> {
    let len = 1 + usize::try_from(i.wrapping_mul(2_654_435_761) % 2048).expect("below 2048");
    let unit = format!("{i}.");
    let mut v = unit.repeat(len / unit.len() + 1).into_bytes();
    v.truncate(len);
    v
}
