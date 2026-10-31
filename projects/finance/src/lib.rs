//! Personal finance on demo data: exact decimal money, CSV transactions, rules and budgets.

// The library only serves this crate's binary: no `#[must_use]` / `# Errors` boilerplate.
#![allow(clippy::must_use_candidate, clippy::missing_errors_doc)]

pub mod rules;
pub mod transactions;

pub use rules::{Category, DEFAULT_RULES, Rules, default_rules, parse_rules};
pub use transactions::{Transaction, YearMonth, months, parse_csv, to_csv};
