use anyhow::{Context, Result, bail};
use chrono::{Datelike, TimeDelta, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use logline::{
    DEMO_SEED, LineIter, Options, ParseCtx, Zone, build, merge, open_inputs, parse_duration,
    parse_shift, parse_timestamp, render, write_demo,
};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

/// Merge logs from several sources into one readable timeline: repeats collapsed, error bursts
/// marked as incidents, written as one self-contained HTML file (or text).
#[derive(Parser)]
#[command(version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Log files; `-` reads stdin (`docker compose logs -t` is split per service)
    files: Vec<PathBuf>,
    /// Output file [default: stdout]
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Html)]
    format: OutputFormat,
    /// Zone for timestamps without one, and for display: UTC, +02:00 or e.g. Europe/Vienna
    #[arg(long, default_value = "UTC")]
    tz: String,
    /// Move one source's clock: NAME=DURATION, e.g. db=+2s (repeatable)
    #[arg(long)]
    shift: Vec<String>,
    /// Skip lines before this time
    #[arg(long)]
    from: Option<String>,
    /// Skip lines after this time
    #[arg(long)]
    to: Option<String>,
    /// Year for syslog lines that carry none [default: this year]
    #[arg(long)]
    year: Option<i32>,
    /// Repeats further apart than this start a new entry
    #[arg(long, default_value = "60s")]
    collapse_gap: String,
    /// Error/warn lines that make an incident...
    #[arg(long, default_value_t = 5)]
    incident_min: usize,
    /// ...within this window
    #[arg(long, default_value = "60s")]
    incident_window: String,
    /// Most entries in the output; later ones are cut by time
    #[arg(long, default_value_t = 50_000)]
    max_entries: usize,
    /// Print lines read, lines per second and the collapse ratio to stderr
    #[arg(long)]
    stats: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Html,
    Text,
}

#[derive(Subcommand)]
enum Command {
    /// Write demo logs of a made-up incident: api.log, postgres.log, nginx.log
    Demo {
        #[arg(long)]
        out: PathBuf,
        /// Repeat the demo morning on this many days (for a large file)
        #[arg(long, default_value_t = 1)]
        scale: u32,
    },
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    if let Some(Command::Demo { out, scale }) = &cli.command {
        write_demo(out, DEMO_SEED, *scale)?;
        println!(
            "wrote api.log, postgres.log and nginx.log to {}",
            out.display()
        );
        return Ok(());
    }
    if cli.files.is_empty() {
        bail!("no input files (use - for stdin)");
    }
    let zone: Zone = cli.tz.parse()?;
    let time_arg = |s: &Option<String>, flag: &str| {
        s.as_deref()
            .map(|s| {
                parse_timestamp(s, &zone)
                    .with_context(|| format!("{flag}: cannot read {s:?} as a time"))
            })
            .transpose()
    };
    let (from, to) = (time_arg(&cli.from, "--from")?, time_arg(&cli.to, "--to")?);
    let shifts: Vec<(String, TimeDelta)> = cli
        .shift
        .iter()
        .map(|s| parse_shift(s))
        .collect::<Result<_>>()?;
    let opts = Options {
        collapse_gap: parse_duration(&cli.collapse_gap).context("--collapse-gap")?,
        incident_min: cli.incident_min,
        incident_window: parse_duration(&cli.incident_window).context("--incident-window")?,
        cap: cli.max_entries,
    };
    let ctx = Arc::new(ParseCtx {
        zone: zone.clone(),
        year: cli.year.unwrap_or_else(|| Utc::now().year()),
    });

    let started = Instant::now();
    let (inputs, _tmp) = open_inputs(&cli.files)?;
    let names: Vec<String> = inputs.iter().map(|i| i.name.clone()).collect();
    for (name, _) in &shifts {
        if !names.iter().any(|n| matches_source(n, name)) {
            eprintln!("warning: --shift {name}: no source with that name");
        }
    }
    let iters = inputs
        .into_iter()
        .enumerate()
        .map(|(i, input)| {
            let shift = shifts
                .iter()
                .find(|(name, _)| matches_source(&input.name, name))
                .map_or_else(TimeDelta::zero, |(_, d)| *d);
            LineIter::new(
                i,
                input.name,
                input.reader,
                ctx.clone(),
                shift,
                input.docker,
            )
        })
        .collect();
    let lines = merge(iters).filter(|line| match line {
        Ok(l) => from.is_none_or(|f| l.time >= f) && to.is_none_or(|t| l.time <= t),
        Err(_) => true,
    });
    let timeline = build(names, lines, &opts)?;

    if cli.stats {
        let seconds = started.elapsed().as_secs_f64();
        let lines = timeline.lines_read;
        let entries = timeline.entries.len().max(1);
        #[allow(clippy::cast_precision_loss)] // statistics only
        let (rate, ratio) = (
            lines as f64 / seconds.max(1e-9),
            lines as f64 / entries as f64,
        );
        eprintln!(
            "lines: {lines}, lines/s: {rate:.0}, entries: {}, collapse ratio: {ratio:.1}",
            timeline.entries.len()
        );
    }

    let page = match cli.format {
        OutputFormat::Html => render::html(&timeline, &zone),
        OutputFormat::Text => {
            let color = cli.output.is_none() && std::io::stdout().is_terminal();
            render::text(&timeline, &zone, color)
        }
    };
    if let Some(path) = &cli.output {
        return write_file(path, &page);
    }
    print!("{page}");
    Ok(())
}

/// A source matches its full name or its name without the extension (`db` for `db.log`).
fn matches_source(source: &str, name: &str) -> bool {
    source == name
        || Path::new(source)
            .file_stem()
            .is_some_and(|stem| stem == name)
}

fn write_file(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
}
