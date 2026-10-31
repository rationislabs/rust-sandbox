use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use finance::{DEFAULT_RULES, YearMonth, format_report, generate, load, months, summarize, to_csv};
use std::path::PathBuf;
use std::process::ExitCode;

/// Personal finance tracker on demo data: budgets vs actual, categories, monthly trend.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Transactions CSV (date,payee,amount) instead of the demo data
    #[arg(long, global = true)]
    csv: Option<PathBuf>,
    /// Categories and budgets instead of the built-in ones
    #[arg(long, global = true)]
    rules: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Write the demo transactions.csv and rules.toml into a folder
    Demo {
        #[arg(long)]
        out: PathBuf,
    },
    /// Print one month's summary as plain text
    Report {
        /// The month, YYYY-MM
        #[arg(long)]
        month: YearMonth,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Command::Demo { out }) => {
            std::fs::create_dir_all(&out)
                .with_context(|| format!("cannot create {}", out.display()))?;
            std::fs::write(
                out.join("transactions.csv"),
                to_csv(&generate(finance::DEMO_SEED)),
            )?;
            std::fs::write(out.join("rules.toml"), DEFAULT_RULES)?;
            println!(
                "wrote {0}/transactions.csv and {0}/rules.toml",
                out.display()
            );
        }
        Some(Command::Report { month }) => {
            let (txns, rules) = load(cli.csv.as_deref(), cli.rules.as_deref())?;
            let covered = months(&txns);
            if !covered.contains(&month) {
                let (first, last) = (covered[0], covered[covered.len() - 1]);
                bail!("no transactions in {month} (data covers {first} to {last})");
            }
            print!("{}", format_report(&summarize(&txns, &rules, month)));
        }
        None => {
            load(cli.csv.as_deref(), cli.rules.as_deref())?;
            println!("app: not yet");
        }
    }
    Ok(())
}
