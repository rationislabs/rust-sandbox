# finance

A personal finance tracker in the terminal, running on six months of made-up demo data
(May–October 2026, one person in Vienna). `cargo run -p finance` opens it straight away: income,
spending and budgets for the month, every transaction, and the spending trend, all by keyboard.
Money is `rust_decimal::Decimal` throughout, never a float, so cent sums are exact.

```text
 Overview │ Transactions │ Trend                                     ◀ 2026-10 ▶
Income      2847.63
Spending    1470.69
Net         1376.94
┌ Budget vs actual ────────────────────────────────────────────────────────────┐
│Rent          ██████████████████████████████████████████    850.00 / 850.00   │
│Groceries     ██████████████████████████████████████████    350.22 / 350.00   │
│Utilities     █████████████████████████████████████░░░░░    106.85 / 120.00   │
│Transport     █████████████████████░░░░░░░░░░░░░░░░░░░░░     30.42 / 60.00    │
│Eating out    ██████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░     50.52 / 150.00   │
│Subscriptions ██████████████████████████░░░░░░░░░░░░░░░░     24.98 / 40.00    │
│Shopping      ████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░     38.54 / 100.00   │
└──────────────────────────────────────────────────────────────────────────────┘
┌ Largest expenses ────────────────────────────────────────────────────────────┐
│2026-10-01  Hausverwaltung Gumpendorf          850.00                         │
│2026-10-05  Wien Energie                        76.86                         │
│2026-10-27  Lieferando.at                       40.81                         │
│2026-10-23  Amazon EU                           38.54                         │
│2026-10-14  HOFER                               37.44                         │
└──────────────────────────────────────────────────────────────────────────────┘
Tab view · ←/→ month · ↑/↓ scroll · q quit
```

(The Overview at 80×24, rendered with ratatui's `TestBackend`. In a terminal, over-budget
categories are red: Groceries here, by 0.22.)

## Keys

`Tab` switches between Overview, Transactions and Trend, `←`/`→` change the month, `↑`/`↓`
scroll the transactions, and `q` or `Esc` quits. Below 70×20 the app asks for a bigger window.

## Commands

```sh
finance                                   # demo data, opens the app
finance --csv my.csv [--rules rules.toml] # own files instead of the demo
finance demo --out demo/                  # writes demo transactions.csv + rules.toml
finance report --month 2026-09 [--csv …]  # plain-text month summary, no full-screen app
```

`report` prints the same numbers as the Overview:

```console
$ finance report --month 2026-08
2026-08
Income: 2847.63
Spending: 1715.60
Net: 1132.03
Rent: 850.00 of 850.00 (0.00 left)
Groceries: 310.01 of 350.00 (39.99 left)
...
Shopping: 341.18 of 100.00 (241.18 over)
Uncategorized: 31.21
Top expenses:
  2026-08-01  Hausverwaltung Gumpendorf  850.00
  2026-08-22  IKEA Wien Vösendorf  289.90
  ...
```

## File formats

`transactions.csv`: header exactly `date,payee,amount`; dates `YYYY-MM-DD`; amounts in EUR with
at most two decimals, negative for spending, positive for income or a refund. A bad row stops
the import with its line number (`line 3: bad amount "lots"`).

`rules.toml`: categories in order, and the first one with a keyword inside the payee wins
(case-insensitive). Budgets are monthly and written as strings, so they never pass through a
float. A refund at a grocery shop lowers grocery spending.

```toml
[[category]]
name = "Income"
income = true
keywords = ["salary"]

[[category]]
name = "Groceries"
budget = "350.00"
keywords = ["billa", "spar", "hofer", "lidl", "penny"]
```

The built-in rules are in [`src/default_rules.toml`](src/default_rules.toml); `finance demo`
writes a copy to edit.

## Test

```sh
cargo test -p finance
```
