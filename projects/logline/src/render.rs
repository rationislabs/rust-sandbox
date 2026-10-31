//! Timeline out: plain text (with optional ANSI colors) and the self-contained HTML page.

use crate::{Level, Timeline, Zone};
use chrono::{DateTime, Utc};
use std::fmt::Write;

fn when(t: DateTime<Utc>, zone: &Zone, fmt: &str) -> String {
    if t == DateTime::UNIX_EPOCH {
        "no time".to_string()
    } else {
        zone.display(t, fmt)
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Debug => "DEBUG",
        Level::Info => "INFO",
        Level::Warn => "WARN",
        Level::Error => "ERROR",
    }
}

/// One line per entry, incident markers before the first entry inside each incident, a summary.
pub fn text(t: &Timeline, zone: &Zone, color: bool) -> String {
    let width = t
        .sources
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    let mut next_incident = 0;
    for e in &t.entries {
        while let Some(inc) = t
            .incidents
            .get(next_incident)
            .filter(|i| e.start >= i.start)
        {
            next_incident += 1;
            let sources: Vec<&str> = inc.sources.iter().map(|&s| t.sources[s].as_str()).collect();
            let _ = writeln!(
                out,
                "--- incident {next_incident}: {}–{}, {} lines, sources: {} ---",
                when(inc.start, zone, "%H:%M:%S"),
                when(inc.end, zone, "%H:%M:%S"),
                inc.lines,
                sources.join(", ")
            );
        }
        let (on, off) = match (color, e.level) {
            (true, Level::Error) => ("\x1b[31m", "\x1b[0m"),
            (true, Level::Warn) => ("\x1b[33m", "\x1b[0m"),
            _ => ("", ""),
        };
        let _ = write!(
            out,
            "{} {:<width$} {on}{:<5}{off} {}",
            when(e.start, zone, "%Y-%m-%d %H:%M:%S%.3f"),
            t.sources[e.source],
            level_name(e.level),
            e.message
        );
        if e.count > 1 {
            let _ = write!(
                out,
                " (×{}, until {})",
                e.count,
                when(e.end, zone, "%H:%M:%S")
            );
        }
        out.push('\n');
    }
    if t.entries.is_empty() {
        out.push_str("no entries\n");
    } else {
        let _ = writeln!(
            out,
            "{} entries from {} lines, {} incidents",
            t.entries.len(),
            t.lines_read,
            t.incidents.len()
        );
    }
    if let Some(cut) = &t.truncated {
        let _ = writeln!(
            out,
            "cut at {}: {} more entries, {} incidents after it; narrow with --from",
            when(cut.cut, zone, "%Y-%m-%d %H:%M:%S"),
            cut.dropped,
            cut.incidents_after
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entry, Incident, Truncation};

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn timeline() -> Timeline {
        let entry = |source, start: &str, level, message: &str, count| Entry {
            source,
            start: at(start),
            end: at(start) + chrono::TimeDelta::seconds(30),
            level,
            message: message.to_string(),
            count,
            samples: vec![message.to_string()],
        };
        Timeline {
            sources: vec!["api".into(), "postgres".into()],
            entries: vec![
                entry(1, "2026-10-28T10:14:00Z", Level::Info, "checkpoint", 1),
                entry(
                    0,
                    "2026-10-28T10:15:00Z",
                    Level::Error,
                    "db pool exhausted",
                    512,
                ),
            ],
            incidents: vec![Incident {
                start: at("2026-10-28T10:15:00Z"),
                end: at("2026-10-28T10:22:00Z"),
                sources: vec![0, 1],
                lines: 530,
            }],
            truncated: Some(Truncation {
                cut: at("2026-10-28T11:00:00Z"),
                dropped: 7,
                incidents_after: 0,
            }),
            lines_read: 600,
        }
    }

    #[test]
    fn text_layout() {
        let out = text(&timeline(), &"UTC".parse().unwrap(), false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[0],
            "2026-10-28 10:14:00.000 postgres INFO  checkpoint"
        );
        assert_eq!(
            lines[1],
            "--- incident 1: 10:15:00–10:22:00, 530 lines, sources: api, postgres ---"
        );
        assert_eq!(
            lines[2],
            "2026-10-28 10:15:00.000 api      ERROR db pool exhausted (×512, until 10:15:30)"
        );
        assert_eq!(lines[3], "2 entries from 600 lines, 1 incidents");
        assert!(lines[4].starts_with("cut at 2026-10-28 11:00:00: 7 more entries"));
    }

    #[test]
    fn color_only_on_warn_and_error() {
        let out = text(&timeline(), &"UTC".parse().unwrap(), true);
        assert!(out.contains("\x1b[31mERROR\x1b[0m"));
        assert!(!out.lines().next().unwrap().contains('\x1b'));
    }
}
