mod list;
mod model;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

/// A small command-line task list, kept in one JSON file.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The task file [default: `$TODO_FILE`, else `$XDG_DATA_HOME/todo/todo.json`, else `~/.local/share/todo/todo.json`]
    #[arg(long, global = true)]
    file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add a task
    Add {
        text: String,
        /// Due date, YYYY-MM-DD
        #[arg(long)]
        due: Option<String>,
        /// A tag; repeat for more
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// List open tasks: overdue first, then by due date
    List {
        /// Include done tasks
        #[arg(long)]
        all: bool,
        /// today, overdue, or a date YYYY-MM-DD (due on or before it)
        #[arg(long)]
        due: Option<String>,
        #[arg(long)]
        tag: Option<String>,
    },
    /// Mark a task done
    Done { id: u32 },
    /// Remove a task
    Rm { id: u32 },
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
    let path = resolve_path(
        cli.file,
        std::env::var_os("TODO_FILE"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )?;
    let mut file = model::load(&path)?;
    match cli.command {
        Command::Add { text, due, tags } => {
            let due = due.map(|d| parse_date(&d)).transpose()?;
            let id = file.add(text, due, tags);
            model::save(&path, &file)?;
            println!("added {id}");
        }
        Command::List { all, due, tag } => {
            let due = due.map(|d| list::parse_due(&d)).transpose()?;
            let today = today()?;
            let tasks = list::select(&file.tasks, all, due, tag.as_deref(), today);
            if tasks.is_empty() {
                println!("No tasks.");
            }
            for task in tasks {
                println!("{}", list::render(task, today));
            }
        }
        Command::Done { id } => {
            file.done(id)?;
            model::save(&path, &file)?;
        }
        Command::Rm { id } => {
            file.remove(id)?;
            model::save(&path, &file)?;
        }
    }
    Ok(())
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("--due takes a date YYYY-MM-DD, not {s:?}"))
}

/// `TODO_TODAY` pins the date for tests; it is not in `--help` on purpose.
fn today() -> Result<NaiveDate> {
    match std::env::var("TODO_TODAY") {
        Ok(s) => parse_date(&s).context("TODO_TODAY"),
        Err(_) => Ok(chrono::Local::now().date_naive()),
    }
}

/// `--file`, then `TODO_FILE`, then the XDG data folder, then `~/.local/share`. Empty values count as unset.
fn resolve_path(
    flag: Option<PathBuf>,
    todo_file: Option<OsString>,
    xdg: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf> {
    let set = |v: Option<OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(path) = flag.or_else(|| set(todo_file)) {
        return Ok(path);
    }
    if let Some(xdg) = set(xdg) {
        return Ok(xdg.join("todo").join("todo.json"));
    }
    if let Some(home) = set(home) {
        return Ok(home.join(".local/share/todo/todo.json"));
    }
    bail!("no place for the task file: pass --file <PATH>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn flag_wins() {
        let got = resolve_path(
            Some("f.json".into()),
            Some(OsString::from("t.json")),
            Some(OsString::from("/x")),
            Some(OsString::from("/h")),
        );
        assert_eq!(got.unwrap(), PathBuf::from("f.json"));
    }

    #[test]
    fn then_todo_file() {
        let got = resolve_path(
            None,
            Some(OsString::from("t.json")),
            Some(OsString::from("/x")),
            Some(OsString::from("/h")),
        );
        assert_eq!(got.unwrap(), PathBuf::from("t.json"));
    }

    #[test]
    fn then_xdg() {
        let got = resolve_path(
            None,
            None,
            Some(OsString::from("/x")),
            Some(OsString::from("/h")),
        );
        assert_eq!(got.unwrap(), PathBuf::from("/x/todo/todo.json"));
    }

    #[test]
    fn then_home() {
        let got = resolve_path(
            None,
            Some(OsString::from("")),
            Some(OsString::from("")),
            Some(OsString::from("/h")),
        );
        assert_eq!(
            got.unwrap(),
            PathBuf::from("/h/.local/share/todo/todo.json")
        );
    }

    #[test]
    fn nothing_set_asks_for_file() {
        let err = resolve_path(None, None, None, None).unwrap_err();
        assert!(err.to_string().contains("--file"));
    }
}
