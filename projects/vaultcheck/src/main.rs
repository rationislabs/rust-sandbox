//! Read-only checker for an Obsidian vault: broken links, orphans, folders without a repo and
//! dangling symlinks.

mod check;
mod parse;
mod resolve;
mod scan;

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    version,
    about = "Find broken links, orphans, folders without a repo and dangling symlinks in an Obsidian vault"
)]
struct Cli {
    /// The vault (default: `$OBSIDIAN_VAULT`, else `~/Documents/vault`)
    vault: Option<PathBuf>,
    /// HOST=DIR: the folder holding the repos of vault/HOST/<repo>; repeatable
    #[arg(long = "root", value_name = "HOST=DIR", value_parser = parse_root)]
    roots: Vec<(String, PathBuf)>,
    /// Report nothing under this vault folder and stat none of its symlinks; repeatable
    #[arg(long, value_name = "PREFIX")]
    exclude: Vec<String>,
    /// Print the findings as JSON
    #[arg(long)]
    json: bool,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn expand(s: &str) -> PathBuf {
    match s.strip_prefix('~') {
        Some("") => home(),
        Some(rest) if rest.starts_with('/') => home().join(&rest[1..]),
        _ => PathBuf::from(s),
    }
}

fn parse_root(s: &str) -> Result<(String, PathBuf), String> {
    match s.split_once('=') {
        Some((host, dir)) if !host.is_empty() && !dir.is_empty() => {
            Ok((host.to_owned(), expand(dir)))
        }
        _ => Err("expected HOST=DIR".into()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let vault = match (&cli.vault, std::env::var_os("OBSIDIAN_VAULT")) {
        (Some(v), _) => expand(&v.to_string_lossy()),
        (None, Some(v)) => PathBuf::from(v),
        (None, None) => home().join("Documents/vault"),
    };
    let roots = if cli.roots.is_empty() {
        vec![
            ("github".to_owned(), home().join("Development/pro/github")),
            (
                "gitlab".to_owned(),
                home().join("Development/pro/gitlab/rationis_labs"),
            ),
        ]
    } else {
        cli.roots
    };
    let exclude = cli
        .exclude
        .iter()
        .map(|p| p.trim_end_matches('/').to_owned())
        .collect();
    let entries = match scan::scan(&vault) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: {}: {e}", vault.display());
            return ExitCode::from(2);
        }
    };
    let report = check::check(
        &entries,
        &check::Options {
            vault,
            roots,
            exclude,
        },
    );
    for host in &report.unchecked_hosts {
        eprintln!("note: no --root for {host}, its folders are not checked");
    }
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a report always serialises")
        );
    } else {
        print!("{}", report.text());
    }
    if report.failed() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
