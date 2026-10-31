//! Timeline out: plain text (with optional ANSI colors) and the self-contained HTML page.

use crate::{Level, Timeline, Zone};
use chrono::{DateTime, Utc};
use serde_json::json;
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

const TEMPLATE: &str = include_str!("template.html");
const TIME_FMT: &str = "%Y-%m-%d %H:%M:%S%.3f";

/// JSON that is safe inside a `<script>` element: `<`, `>` and `&` become `\u` escapes, so log
/// text can never close the element or open a comment.
pub fn safe_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v)
        .unwrap_or_else(|_| "null".to_string())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// One self-contained page: the template with the timeline embedded as JSON.
pub fn html(t: &Timeline, zone: &Zone) -> String {
    let entries: Vec<_> = t
        .entries
        .iter()
        .map(|e| {
            json!({
                "s": e.source,
                "t": when(e.start, zone, TIME_FMT),
                "e": when(e.end, zone, TIME_FMT),
                "ms": e.start.timestamp_millis(),
                "l": e.level,
                "m": e.message,
                "n": e.count,
                "x": e.samples,
            })
        })
        .collect();
    let incidents: Vec<_> = t
        .incidents
        .iter()
        .map(|i| {
            json!({
                "t": when(i.start, zone, TIME_FMT),
                "e": when(i.end, zone, TIME_FMT),
                "ms": i.start.timestamp_millis(),
                "ems": i.end.timestamp_millis(),
                "sources": i.sources,
                "lines": i.lines,
            })
        })
        .collect();
    let mut counts = serde_json::Map::new();
    for e in &t.entries {
        let n = counts.entry(format!("{:?}", e.level)).or_insert(json!(0));
        *n = json!(n.as_u64().unwrap_or(0) + e.count);
    }
    let span = match (t.entries.first(), t.entries.iter().map(|e| e.end).max()) {
        (Some(first), Some(last)) => format!(
            "{} – {}",
            when(first.start, zone, "%Y-%m-%d %H:%M:%S"),
            when(last, zone, "%Y-%m-%d %H:%M:%S")
        ),
        _ => "no entries".to_string(),
    };
    let truncated = t.truncated.as_ref().map(|c| {
        json!({
            "cut": when(c.cut, zone, "%Y-%m-%d %H:%M:%S"),
            "dropped": c.dropped,
            "incidents_after": c.incidents_after,
        })
    });
    let data = json!({
        "zone": zone_name(zone),
        "sources": t.sources,
        "entries": entries,
        "incidents": incidents,
        "summary": { "span": span, "counts": counts, "lines": t.lines_read, "truncated": truncated },
    });
    TEMPLATE.replacen("/*DATA*/", &safe_json(&data), 1)
}

fn zone_name(zone: &Zone) -> String {
    match zone {
        Zone::Fixed(offset) if offset.local_minus_utc() == 0 => "UTC".to_string(),
        Zone::Fixed(offset) => offset.to_string(),
        Zone::Named(tz) => tz.name().to_string(),
    }
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

    #[test]
    fn html_is_self_contained() {
        let page = html(&timeline(), &"UTC".parse().unwrap());
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("const DATA ="));
        for bad in ["http://", "https://", "src=", "@import", "url("] {
            assert!(!page.contains(bad), "page contains {bad:?}");
        }
    }

    #[test]
    fn escaping() {
        let mut t = timeline();
        t.sources[0] = "<b>x.log".into();
        t.entries[1].message = "</script><script>alert(1)</script> <!--".into();
        t.entries[1].samples = vec![t.entries[1].message.clone()];
        let page = html(&t, &"UTC".parse().unwrap());
        assert!(!page.contains("</script><script>alert"));
        assert!(!page.contains("<b>x.log"));
        assert!(!page.contains("<!--"));
        assert!(
            page.contains("</script>"),
            "the page's own script tag still closes"
        );
    }

    #[test]
    fn data_round_trips() {
        let t = timeline();
        let page = html(&t, &"UTC".parse().unwrap());
        let start = page.find("const DATA = ").unwrap() + "const DATA = ".len();
        let end = start + page[start..].find(";</script>").unwrap();
        let data: serde_json::Value = serde_json::from_str(&page[start..end]).unwrap();
        assert_eq!(data["entries"].as_array().unwrap().len(), t.entries.len());
        assert_eq!(data["entries"][1]["n"], 512);
        assert_eq!(data["incidents"].as_array().unwrap().len(), 1);
        assert_eq!(data["sources"][1], "postgres");
        assert_eq!(data["summary"]["truncated"]["dropped"], 7);
    }
}
