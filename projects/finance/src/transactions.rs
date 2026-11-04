use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub struct Transaction {
    pub date: NaiveDate,
    pub payee: String,
    /// Negative = spending, positive = income or a refund.
    pub amount: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    pub year: i32,
    pub month: u32,
}

impl YearMonth {
    pub fn of(date: NaiveDate) -> Self {
        Self {
            year: date.year(),
            month: date.month(),
        }
    }
}

impl FromStr for YearMonth {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let date = NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d")
            .map_err(|_| anyhow!("expected a month as YYYY-MM, not {s:?}"))?;
        Ok(Self::of(date))
    }
}

impl fmt::Display for YearMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

const HEADER: [&str; 3] = ["date", "payee", "amount"];
/// Amounts and budgets stay below this, so no realistic number of rows can overflow a sum.
pub const MAX_AMOUNT: Decimal = Decimal::from_parts(1_000_000_000, 0, 0, false, 0);

/// Reads `date,payee,amount` rows; stops at the first bad row with `line <n>:` in the error.
pub fn parse_csv(input: impl std::io::Read) -> Result<Vec<Transaction>> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(input);
    let header = reader.headers().context("line 1: cannot read the header")?;
    if header.iter().ne(HEADER) {
        bail!("line 1: the header must be exactly date,payee,amount");
    }
    let mut txns = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| {
            let line = e.position().map_or(0, csv::Position::line);
            anyhow!("line {line}: {e}")
        })?;
        let line = record.position().map_or(0, csv::Position::line);
        txns.push(parse_row(&record).with_context(|| format!("line {line}"))?);
    }
    Ok(txns)
}

fn parse_row(record: &csv::StringRecord) -> Result<Transaction> {
    let [date, payee, amount] = [0, 1, 2].map(|i| record.get(i));
    let (Some(date), Some(payee), Some(amount), 3) = (date, payee, amount, record.len()) else {
        bail!(
            "expected 3 fields (date,payee,amount), found {}",
            record.len()
        );
    };
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| anyhow!("bad date {date:?}, expected YYYY-MM-DD"))?;
    Ok(Transaction {
        date,
        payee: payee.to_string(),
        amount: parse_amount(amount)?,
    })
}

/// A plain decimal like `-12.50`: optional sign, digits, at most 2 decimals, below
/// [`MAX_AMOUNT`]. `Decimal::from_str` alone would also take `1e3` and `1_000`.
pub(crate) fn parse_amount(amount: &str) -> Result<Decimal> {
    let digits = amount.strip_prefix(['-', '+']).unwrap_or(amount);
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let plain = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let parsed = (!int.is_empty() && plain(int) && plain(frac))
        .then_some(amount)
        .and_then(|a| Decimal::from_str(a).ok())
        .ok_or_else(|| anyhow!("bad amount {amount:?}, expected a number like -12.50"))?;
    if parsed.scale() > 2 {
        bail!("amount {amount:?} has more than 2 decimals");
    }
    if parsed.abs() >= MAX_AMOUNT {
        bail!("amount {amount:?} is too large (limit {MAX_AMOUNT})");
    }
    Ok(parsed)
}

pub fn to_csv(txns: &[Transaction]) -> String {
    let mut writer = csv::Writer::from_writer(Vec::new());
    // ponytail: writing to a Vec cannot fail, so the results are ignored.
    let _ = writer.write_record(HEADER);
    for t in txns {
        let _ = writer.write_record([
            t.date.to_string(),
            t.payee.clone(),
            format!("{:.2}", t.amount),
        ]);
    }
    String::from_utf8(writer.into_inner().unwrap_or_default()).unwrap_or_default()
}

/// Distinct months, ascending.
pub fn months(txns: &[Transaction]) -> Vec<YearMonth> {
    let mut out: Vec<YearMonth> = txns.iter().map(|t| YearMonth::of(t.date)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn err(input: &str) -> String {
        format!("{:#}", parse_csv(input.as_bytes()).unwrap_err())
    }

    #[test]
    fn parses_good_file() {
        let input = "date,payee,amount\n2026-09-01,ACME Salary,2500.00\n2026-09-03,BILLA,-23.45\n2026-09-05,SPAR,-0.10\n";
        let txns = parse_csv(input.as_bytes()).unwrap();
        assert_eq!(txns.len(), 3);
        assert_eq!(txns[0].amount, dec("2500.00"));
        assert_eq!(txns[2].amount, dec("-0.10"));
        assert_eq!(txns[1].payee, "BILLA");
    }

    #[test]
    fn bad_date_reports_line() {
        let e = err("date,payee,amount\n2026-02-01,A,-1.00\n2026-02-30,B,-1.00\n");
        assert!(e.starts_with("line 3:"), "{e}");
    }

    #[test]
    fn bad_amount_reports_line() {
        let e = err("date,payee,amount\n2026-02-01,A,abc\n");
        assert!(e.starts_with("line 2:"), "{e}");
    }

    #[test]
    fn missing_column_reports_line() {
        let e = err("date,payee,amount\n2026-02-01,A,-1\n2026-02-02,B,-1\n2026-02-03,C\n");
        assert!(e.starts_with("line 4:"), "{e}");
    }

    #[test]
    fn wrong_header_rejected() {
        let e = err("payee,date,amount\nA,2026-02-01,-1\n");
        assert!(e.contains("date,payee,amount"), "{e}");
    }

    #[test]
    fn rejects_3_decimals_and_comma() {
        let e = err("date,payee,amount\n2026-02-01,A,-1.005\n");
        assert!(e.starts_with("line 2:"), "{e}");
        let e = err("date,payee,amount\n2026-02-01,A,\"-12,50\"\n");
        assert!(e.starts_with("line 2:"), "{e}");
    }

    #[test]
    fn csv_round_trip() {
        let input =
            "date,payee,amount\n2026-09-01,\"Hausverwaltung, Wien\",-850\n2026-09-03,BILLA,-23.4\n";
        let txns = parse_csv(input.as_bytes()).unwrap();
        assert_eq!(parse_csv(to_csv(&txns).as_bytes()).unwrap(), txns);
        assert!(to_csv(&txns).contains("-850.00"));
    }

    #[test]
    fn months_sorted_distinct() {
        let input = "date,payee,amount\n2026-10-02,A,-1\n2026-05-02,B,-1\n2026-10-09,C,-1\n";
        let txns = parse_csv(input.as_bytes()).unwrap();
        let got: Vec<String> = months(&txns).iter().map(ToString::to_string).collect();
        assert_eq!(got, ["2026-05", "2026-10"]);
    }

    #[test]
    fn year_month_parses() {
        let ym: YearMonth = "2026-09".parse().unwrap();
        assert_eq!(
            ym,
            YearMonth {
                year: 2026,
                month: 9
            }
        );
        assert!("2026-13".parse::<YearMonth>().is_err());
        assert!("Sept".parse::<YearMonth>().is_err());
    }

    #[test]
    fn rejects_exponent_and_underscores() {
        for amount in ["-1e3", "-1_000.00", "1E2", "+-1", ".5", " -1.00"] {
            let e = err(&format!("date,payee,amount\n2026-02-01,A,{amount}\n"));
            assert!(e.starts_with("line 2:"), "{amount}: {e}");
        }
        let input = "date,payee,amount\n2026-02-01,A,+12.5\n2026-02-01,B,7.\n";
        assert_eq!(parse_csv(input.as_bytes()).unwrap().len(), 2);
    }

    #[test]
    fn rejects_absurd_amounts() {
        let e = err(
            "date,payee,amount\n2026-02-01,A,-1.00\n2026-02-02,B,-79228162514264337593543950335\n",
        );
        assert!(e.starts_with("line 3:"), "{e}");
    }
}
