//! Six made-up months for one person in Vienna, identical for the same seed.

use crate::Transaction;
use chrono::{Datelike, NaiveDate, Weekday};
use rust_decimal::Decimal;

pub const DEMO_SEED: u64 = 2026;
const FIRST: NaiveDate = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
const LAST: NaiveDate = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();

const GROCERS: [&str; 5] = [
    "BILLA Filiale 1070",
    "SPAR Gumpendorfer Str.",
    "HOFER",
    "Lidl Wien Margareten",
    "Penny Markt",
];
const EATING_OUT: [&str; 5] = [
    "Café Sperl",
    "Café Hawelka",
    "Restaurant Figlmüller",
    "mjam*order",
    "Lieferando.at",
];

struct Gen {
    rng: fastrand::Rng,
    out: Vec<Transaction>,
}

impl Gen {
    fn push(&mut self, date: NaiveDate, payee: &str, cents: i64) {
        self.out.push(Transaction {
            date,
            payee: payee.to_string(),
            amount: Decimal::new(cents, 2),
        });
    }

    /// A spending amount between `lo` and `hi` euros, with cents.
    fn spend(&mut self, date: NaiveDate, payee: &str, lo: i64, hi: i64) {
        let cents = self.rng.i64(lo * 100..hi * 100);
        // Whole-euro amounts look generated; nudge them off the round number.
        let cents = if cents % 100 == 0 { cents + 37 } else { cents };
        self.push(date, payee, -cents);
    }

    fn chance(&mut self, p: f64) -> bool {
        self.rng.f64() < p
    }
}

/// Demo transactions for 2026-05-01 to 2026-10-31, sorted by date.
pub fn generate(seed: u64) -> Vec<Transaction> {
    let mut g = Gen {
        rng: fastrand::Rng::with_seed(seed),
        out: Vec::new(),
    };
    let mut shop_days: Vec<u32> = Vec::new();
    for date in FIRST.iter_days().take_while(|d| *d <= LAST) {
        let month = date.month();
        match date.day() {
            1 => {
                g.push(date, "ACME GmbH Salary", 284_763);
                g.push(date, "Hausverwaltung Gumpendorf", -85_000);
                g.push(date, "Wiener Linien Jahreskarte", -3_042);
            }
            5 => g.spend(date, "Wien Energie", 62, 84),
            8 => g.push(date, "Netflix", -1_399),
            12 => g.push(date, "Magenta Telekom", -2_999),
            15 => g.push(date, "Spotify", -1_099),
            // Drugstore: no default rule matches it.
            20 => g.spend(date, "BIPA Mariahilfer Str.", 9, 34),
            _ => {}
        }
        // Groceries 3–4 times a week (Mon–Sat), days picked fresh each week.
        if date.weekday() == Weekday::Mon || date == FIRST {
            let mut days = [0u32, 1, 2, 3, 4, 5];
            g.rng.shuffle(&mut days);
            let k = g.rng.usize(3..=4);
            shop_days = days[..k].to_vec();
        }
        if shop_days.contains(&date.weekday().num_days_from_monday()) {
            let shop = GROCERS[g.rng.usize(..GROCERS.len())];
            g.spend(date, shop, 5, 38);
        }
        if g.chance(0.17) {
            let place = EATING_OUT[g.rng.usize(..EATING_OUT.len())];
            g.spend(date, place, 9, 42);
        }
        if g.chance(0.05) {
            g.spend(date, "Amazon EU", 11, 48);
        }
        // One-offs, fixed so the story is the same every run.
        match (month, date.day()) {
            (6, 14) => g.spend(date, "MediaMarkt Wien Mitte", 79, 80),
            (7, 3) => g.push(date, "Amazon EU Refund", 2_499),
            (7, 18) | (9, 26) => g.spend(date, "ÖBB Ticket Wien-Salzburg", 19, 31),
            // The over-budget month: a new shelf in August.
            (8, 22) => g.push(date, "IKEA Wien Vösendorf", -28_990),
            (9, 9) => g.spend(date, "Thalia Buchhandlung", 14, 38),
            _ => {}
        }
    }
    g.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{YearMonth, default_rules, months, summarize};
    use std::collections::HashSet;

    #[test]
    fn same_seed_same_data() {
        assert_eq!(generate(DEMO_SEED), generate(DEMO_SEED));
        assert_ne!(generate(1), generate(2));
    }

    #[test]
    fn six_months_in_window() {
        let got: Vec<String> = months(&generate(DEMO_SEED))
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            got,
            [
                "2026-05", "2026-06", "2026-07", "2026-08", "2026-09", "2026-10"
            ]
        );
        let txns = generate(DEMO_SEED);
        assert!(
            txns.windows(2).all(|w| w[0].date <= w[1].date),
            "sorted by date"
        );
    }

    #[test]
    fn some_month_over_budget() {
        let txns = generate(DEMO_SEED);
        let rules = default_rules();
        let over = months(&txns).into_iter().any(|m| {
            summarize(&txns, &rules, m)
                .categories
                .iter()
                .any(|c| c.remaining().is_sign_negative())
        });
        assert!(over);
    }

    #[test]
    fn has_uncategorized() {
        let rules = default_rules();
        let payees: HashSet<String> = generate(DEMO_SEED)
            .into_iter()
            .filter(|t| rules.categorize(&t.payee).is_none())
            .map(|t| t.payee)
            .collect();
        assert!(payees.len() >= 2, "{payees:?}");
    }

    #[test]
    fn looks_real() {
        let txns = generate(DEMO_SEED);
        let rules = default_rules();
        for m in months(&txns) {
            let groceries = txns
                .iter()
                .filter(|t| YearMonth::of(t.date) == m)
                .filter(|t| {
                    rules
                        .categorize(&t.payee)
                        .is_some_and(|c| c.name == "Groceries")
                })
                .count();
            assert!(groceries >= 8, "{m}: {groceries} grocery rows");
        }
        let spending: Vec<_> = txns
            .iter()
            .filter(|t| t.amount.is_sign_negative())
            .collect();
        let whole = spending
            .iter()
            .filter(|t| t.amount.fract().is_zero())
            .count();
        assert!(
            whole * 5 < spending.len(),
            "{whole} of {} whole euros",
            spending.len()
        );
    }
}
