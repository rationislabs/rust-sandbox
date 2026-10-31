use crate::{Rules, Transaction, YearMonth, months};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq)]
pub struct CategoryLine {
    pub name: String,
    pub budget: Decimal,
    pub actual: Decimal,
}

impl CategoryLine {
    /// Negative = over budget.
    pub fn remaining(&self) -> Decimal {
        self.budget - self.actual
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonthSummary {
    pub month: YearMonth,
    pub income: Decimal,
    pub spending: Decimal,
    pub net: Decimal,
    /// Every budgeted category in rules order, zeros included.
    pub categories: Vec<CategoryLine>,
    pub uncategorized: Decimal,
    /// Up to 5 spending rows, largest first.
    pub top: Vec<Transaction>,
}

/// Spending is positive: minus the signed sum, so a refund lowers it.
fn spent(signed_sum: Decimal) -> Decimal {
    if signed_sum.is_zero() {
        Decimal::ZERO
    } else {
        -signed_sum
    }
}

pub fn summarize(txns: &[Transaction], rules: &Rules, month: YearMonth) -> MonthSummary {
    let rows: Vec<&Transaction> = txns
        .iter()
        .filter(|t| YearMonth::of(t.date) == month)
        .collect();
    let mut sums: HashMap<&str, Decimal> = HashMap::new();
    let mut income = Decimal::ZERO;
    let mut uncategorized = Decimal::ZERO;
    let mut spending_rows = Vec::new();
    for t in &rows {
        match rules.categorize(&t.payee) {
            Some(c) if c.income => income += t.amount,
            Some(c) => {
                *sums.entry(c.name.as_str()).or_default() += t.amount;
                spending_rows.push(*t);
            }
            // Money in that no rule claims is income, not negative spending.
            None if t.amount.is_sign_positive() => income += t.amount,
            None => {
                uncategorized += t.amount;
                spending_rows.push(*t);
            }
        }
    }
    let uncategorized = spent(uncategorized);
    let spending = sums.values().map(|s| spent(*s)).sum::<Decimal>() + uncategorized;
    let categories = rules
        .categories
        .iter()
        .filter_map(|c| {
            Some(CategoryLine {
                name: c.name.clone(),
                budget: c.budget?,
                actual: spent(sums.get(c.name.as_str()).copied().unwrap_or_default()),
            })
        })
        .collect();
    let mut top: Vec<Transaction> = spending_rows
        .into_iter()
        .filter(|t| t.amount.is_sign_negative() && !t.amount.is_zero())
        .cloned()
        .collect();
    top.sort_by_key(|t| t.amount);
    top.truncate(5);
    MonthSummary {
        month,
        income,
        spending,
        net: income - spending,
        categories,
        uncategorized,
        top,
    }
}

/// Spending per month across all months in the data, ascending.
pub fn monthly_spending(txns: &[Transaction], rules: &Rules) -> Vec<(YearMonth, Decimal)> {
    months(txns)
        .into_iter()
        .map(|m| (m, summarize(txns, rules, m).spending))
        .collect()
}

pub fn format_report(s: &MonthSummary) -> String {
    let mut out = format!(
        "{}\nIncome: {:.2}\nSpending: {:.2}\nNet: {:.2}\n",
        s.month, s.income, s.spending, s.net
    );
    for c in &s.categories {
        let left = c.remaining();
        let state = if left.is_sign_negative() && !left.is_zero() {
            format!("{:.2} over", -left)
        } else {
            format!("{left:.2} left")
        };
        let _ = writeln!(
            out,
            "{}: {:.2} of {:.2} ({state})",
            c.name, c.actual, c.budget
        );
    }
    if !s.uncategorized.is_zero() {
        let _ = writeln!(out, "Uncategorized: {:.2}", s.uncategorized);
    }
    out.push_str("Top expenses:\n");
    for t in &s.top {
        let _ = writeln!(out, "  {}  {}  {:.2}", t.date, t.payee, -t.amount);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{default_rules, parse_csv};
    use std::str::FromStr;

    const SMALL: &str = include_str!("../tests/fixtures/small.csv");
    const SEPT: YearMonth = YearMonth {
        year: 2026,
        month: 9,
    };

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn line<'a>(s: &'a MonthSummary, name: &str) -> &'a CategoryLine {
        s.categories.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn exact_totals() {
        let s = summarize(
            &parse_csv(SMALL.as_bytes()).unwrap(),
            &default_rules(),
            SEPT,
        );
        assert_eq!(s.income, dec("2500.00"));
        assert_eq!(s.spending, dec("79.74"));
        assert_eq!(s.net, dec("2420.26"));
        assert_eq!(line(&s, "Groceries").actual, dec("18.75"));
        assert_eq!(line(&s, "Groceries").remaining(), dec("331.25"));
        assert_eq!(line(&s, "Transport").remaining(), dec("9.00"));
        assert_eq!(s.uncategorized, dec("9.99"));
        assert_eq!(
            s.categories.len(),
            7,
            "every budgeted category, zeros included"
        );
    }

    #[test]
    fn top_expenses_largest_first() {
        let s = summarize(
            &parse_csv(SMALL.as_bytes()).unwrap(),
            &default_rules(),
            SEPT,
        );
        assert_eq!(s.top[0].payee, "Wiener Linien");
        assert_eq!(s.top[0].amount, dec("-51.00"));
        assert!(s.top.len() <= 5);
        assert!(s.top.iter().all(|t| t.amount < Decimal::ZERO));
    }

    #[test]
    fn over_budget_line() {
        let mut txns = parse_csv(SMALL.as_bytes()).unwrap();
        txns.extend(parse_csv("date,payee,amount\n2026-09-20,BILLA,-400.00\n".as_bytes()).unwrap());
        let report = format_report(&summarize(&txns, &default_rules(), SEPT));
        assert!(
            report
                .lines()
                .any(|l| l == "Groceries: 418.75 of 350.00 (68.75 over)"),
            "{report}"
        );
    }

    #[test]
    fn report_text() {
        let report = format_report(&summarize(
            &parse_csv(SMALL.as_bytes()).unwrap(),
            &default_rules(),
            SEPT,
        ));
        for want in [
            "2026-09",
            "Income: 2500.00",
            "Spending: 79.74",
            "Net: 2420.26",
            "Groceries: 18.75 of 350.00 (331.25 left)",
            "Rent: 0.00 of 850.00 (850.00 left)",
            "Uncategorized: 9.99",
            "Top expenses:",
        ] {
            assert!(
                report.lines().any(|l| l == want),
                "missing {want:?} in\n{report}"
            );
        }
    }

    #[test]
    fn monthly_spending_two_months() {
        let got = monthly_spending(&parse_csv(SMALL.as_bytes()).unwrap(), &default_rules());
        assert_eq!(
            got,
            [
                (SEPT, dec("79.74")),
                (
                    YearMonth {
                        year: 2026,
                        month: 10
                    },
                    dec("0.00")
                )
            ]
        );
    }

    #[test]
    fn positive_uncategorized_is_income() {
        let txns = parse_csv(
            "date,payee,amount\n2026-09-01,Gift from mom,200.00\n2026-09-02,BILLA,-10.00\n"
                .as_bytes(),
        )
        .unwrap();
        let s = summarize(&txns, &default_rules(), SEPT);
        assert_eq!(s.income, dec("200.00"));
        assert_eq!(s.spending, dec("10.00"));
        assert_eq!(s.uncategorized, dec("0"));
    }
}
