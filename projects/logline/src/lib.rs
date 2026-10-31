//! Logs from several sources merged into one readable timeline.

// The library only serves this crate's binary: no `#[must_use]` / `# Errors` boilerplate.
#![allow(clippy::must_use_candidate, clippy::missing_errors_doc)]

pub mod time;

pub use time::{Zone, parse_duration, parse_shift, parse_timestamp};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// Case-insensitive level words as logs spell them.
    pub fn from_word(s: &str) -> Option<Level> {
        match s.to_ascii_lowercase().as_str() {
            "debug" | "trace" => Some(Level::Debug),
            "info" | "notice" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error" | "err" | "fatal" | "panic" | "crit" | "critical" => Some(Level::Error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_words() {
        assert_eq!(Level::from_word("WARNING"), Some(Level::Warn));
        assert_eq!(Level::from_word("Fatal"), Some(Level::Error));
        assert_eq!(Level::from_word("trace"), Some(Level::Debug));
        assert_eq!(Level::from_word("notice"), Some(Level::Info));
        assert_eq!(Level::from_word("hello"), None);
        assert!(
            Level::Error > Level::Warn && Level::Warn > Level::Info && Level::Info > Level::Debug
        );
    }
}
