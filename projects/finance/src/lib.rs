//! Personal finance on demo data: exact decimal money, CSV transactions, rules and budgets.

// The library only serves this crate's binary: no `#[must_use]` / `# Errors` boilerplate.
#![allow(clippy::must_use_candidate, clippy::missing_errors_doc)]

pub mod demo;
pub mod rules;
pub mod summary;
pub mod transactions;

pub use demo::{DEMO_SEED, generate};
pub use rules::{Category, DEFAULT_RULES, Rules, default_rules, parse_rules};
pub use summary::{CategoryLine, MonthSummary, format_report, monthly_spending, summarize};
pub use transactions::{Transaction, YearMonth, months, parse_csv, to_csv};

use anyhow::{Context, Result, bail};
use std::path::Path;

/// Transactions and rules from files, or the demo data and built-in rules when none are given.
pub fn load(csv: Option<&Path>, rules: Option<&Path>) -> Result<(Vec<Transaction>, Rules)> {
    let txns = match csv {
        Some(path) => {
            let file = std::fs::File::open(path)
                .with_context(|| format!("cannot open {}", path.display()))?;
            let txns = parse_csv(file).with_context(|| path.display().to_string())?;
            if txns.is_empty() {
                bail!("no transactions in {}", path.display());
            }
            txns
        }
        None => generate(DEMO_SEED),
    };
    let rules = match rules {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            parse_rules(&text).with_context(|| path.display().to_string())?
        }
        None => default_rules(),
    };
    Ok((txns, rules))
}
