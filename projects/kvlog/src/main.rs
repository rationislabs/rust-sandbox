//! `kvlog`: set, get, rm, list and compact on a crash-safe key-value store.

use clap::{Parser, Subcommand};
use kvlog::{Store, stress_value};
use std::ffi::OsString;
use std::io::{self, ErrorKind, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// A key-value store on a write-ahead log that survives being killed.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The data folder [default: `$KVLOG_DIR`, else `$XDG_DATA_HOME/kvlog`, else `~/.local/share/kvlog`]
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Set a key to a value
    Set { key: String, value: String },
    /// Print a key's value
    Get { key: String },
    /// Remove a key
    Rm { key: String },
    /// Print every key and value, tab-separated, sorted by key
    List {
        /// Only keys starting with this
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Fold the log into a new snapshot
    Compact,
    /// Set key{i} for i in start..start+count, printing `ack {i}` after each fsync
    #[command(hide = true)]
    Stress {
        #[arg(long)]
        start: u64,
        #[arg(long)]
        count: u64,
        #[arg(long)]
        compact_every: Option<u64>,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kvlog: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let dir = resolve_dir(
        cli.dir,
        std::env::var_os("KVLOG_DIR"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )?;
    let mut store = Store::open(&dir)?;
    if store.torn_bytes() > 0 {
        eprintln!(
            "kvlog: dropped {} bytes of an unfinished write at the end of {}",
            store.torn_bytes(),
            dir.join("wal").display()
        );
    }
    let mut out = io::stdout().lock();
    let written = match cli.command {
        Cmd::Set { key, value } => {
            store.set(key.as_bytes(), value.as_bytes())?;
            Ok(())
        }
        Cmd::Get { key } => {
            let value = store
                .get(key.as_bytes())
                .ok_or(format!("no such key: {key}"))?;
            out.write_all(value).and_then(|()| out.write_all(b"\n"))
        }
        Cmd::Rm { key } => {
            if !store.remove(key.as_bytes())? {
                return Err(format!("no such key: {key}").into());
            }
            Ok(())
        }
        Cmd::List { prefix } => {
            let prefix = prefix.unwrap_or_default();
            store
                .iter()
                .filter(|(k, _)| k.starts_with(prefix.as_bytes()))
                .try_for_each(|(k, v)| writeln!(out, "{}\t{}", escape(k), escape(v)))
        }
        Cmd::Compact => {
            store.compact()?;
            Ok(())
        }
        Cmd::Stress {
            start,
            count,
            compact_every,
        } => {
            for i in start..start + count {
                store.set(format!("key{i}").as_bytes(), &stress_value(i))?;
                writeln!(out, "ack {i}").and_then(|()| out.flush())?;
                if compact_every.is_some_and(|k| (i - start + 1) % k == 0) {
                    store.compact()?;
                }
            }
            Ok(())
        }
    };
    match written.and_then(|()| out.flush()) {
        // `kvlog list | head` closing the pipe early is not an error
        Err(e) if e.kind() != ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

/// `--dir`, then `KVLOG_DIR`, then the XDG data folder, then `~/.local/share`. Empty values count
/// as unset.
fn resolve_dir(
    flag: Option<PathBuf>,
    kvlog_dir: Option<OsString>,
    xdg: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    let set = |v: Option<OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    flag.or_else(|| set(kvlog_dir))
        .or_else(|| set(xdg).map(|x| x.join("kvlog")))
        .or_else(|| set(home).map(|h| h.join(".local/share/kvlog")))
        .ok_or_else(|| "no place for the store: pass --dir <PATH>".to_owned())
}

/// One line per entry whatever the bytes hold: backslash, tab, LF and CR are escaped.
fn escape(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unnecessary_wraps)] // keeps the resolve_dir calls short
    fn os(s: &str) -> Option<OsString> {
        Some(s.into())
    }

    #[test]
    fn resolve_dir_order() {
        let p = PathBuf::from;
        assert_eq!(
            resolve_dir(Some(p("/f")), os("/k"), os("/x"), os("/h")),
            Ok(p("/f"))
        );
        assert_eq!(resolve_dir(None, os("/k"), os("/x"), os("/h")), Ok(p("/k")));
        assert_eq!(
            resolve_dir(None, os(""), os("/x"), os("/h")),
            Ok(p("/x/kvlog"))
        );
        assert_eq!(
            resolve_dir(None, None, os(""), os("/h")),
            Ok(p("/h/.local/share/kvlog"))
        );
        assert!(
            resolve_dir(None, None, None, os(""))
                .unwrap_err()
                .contains("--dir")
        );
    }

    #[test]
    fn escape_is_unambiguous() {
        assert_eq!(escape(b"a\tb\nc\\d\r"), "a\\tb\\nc\\\\d\\r");
        assert_eq!(escape("Wien ☕".as_bytes()), "Wien ☕");
    }
}
