//! One line in, time + level + message out. Never fails: unknown lines are kept as text.

use crate::{Level, Zone, parse_timestamp};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Logfmt,
    Syslog3164,
    Syslog5424,
    Combined,
    Postgres,
    Plain,
    Unparsed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// `None`: a continuation line or one that matches no format.
    pub time: Option<DateTime<Utc>>,
    pub level: Level,
    pub message: String,
    pub format: Format,
}

pub struct ParseCtx {
    pub zone: Zone,
    /// RFC 3164 syslog lines carry no year.
    pub year: i32,
}

const TIME_KEYS: [&str; 4] = ["ts", "time", "timestamp", "@timestamp"];
const LEVEL_KEYS: [&str; 3] = ["level", "lvl", "severity"];
const MESSAGE_KEYS: [&str; 2] = ["msg", "message"];

static SYSLOG5424: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<(\d{1,3})>1 (\S+) \S+ \S+ \S+ \S+ (?:-|\[.*?\]) ?(.*)$").expect("valid regex")
});
static SYSLOG3164: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<(\d{1,3})>([A-Z][a-z]{2} [ \d]\d \d\d:\d\d:\d\d) \S+ (.*)$")
        .expect("valid regex")
});
static COMBINED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\S+ \S+ \S+ \[([^\]]+)\] "([^"]*)" (\d{3}) "#).expect("valid regex")
});
static POSTGRES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d(?:[.,]\d+)?)(?: ([A-Za-z]{1,6}))? \[\d+\] ([A-Z]+\d?):\s+(.*)$")
        .expect("valid regex")
});
static LOGFMT_PAIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"([\w.@-]+)=("(?:[^"\\]|\\.)*"|\S*)"#).expect("valid regex"));

pub fn parse_line(line: &str, ctx: &ParseCtx) -> Parsed {
    json(line, ctx)
        .or_else(|| syslog5424(line, ctx))
        .or_else(|| syslog3164(line, ctx))
        .or_else(|| combined(line))
        .or_else(|| postgres(line, ctx))
        .or_else(|| logfmt(line, ctx))
        .or_else(|| plain(line, ctx))
        .unwrap_or_else(|| Parsed {
            time: None,
            level: line
                .split_whitespace()
                .take(3)
                .find_map(level_word)
                .unwrap_or(Level::Info),
            message: line.to_string(),
            format: Format::Unparsed,
        })
}

fn level_word(token: &str) -> Option<Level> {
    Level::from_word(token.trim_matches(|c: char| !c.is_ascii_alphabetic()))
}

/// Syslog severity is `pri % 8`: 0–3 error, 4 warning, the rest info.
fn syslog_level(pri: &str) -> Level {
    match pri.parse::<u32>().map(|p| p % 8) {
        Ok(0..=3) => Level::Error,
        Ok(4) => Level::Warn,
        _ => Level::Info,
    }
}

fn json(line: &str, ctx: &ParseCtx) -> Option<Parsed> {
    if !line.starts_with('{') {
        return None;
    }
    let map: Map<String, Value> = serde_json::from_str(line).ok()?;
    let field = |keys: &[&str]| keys.iter().find_map(|k| map.get(*k));
    let time = field(&TIME_KEYS).and_then(|v| match v {
        Value::String(s) => parse_timestamp(s, &ctx.zone),
        Value::Number(n) if n.is_f64() => float_epoch(n.as_f64()?),
        Value::Number(n) => parse_timestamp(&n.to_string(), &ctx.zone),
        _ => None,
    });
    let level = field(&LEVEL_KEYS)
        .and_then(Value::as_str)
        .and_then(Level::from_word);
    let message = field(&MESSAGE_KEYS).and_then(Value::as_str).unwrap_or(line);
    Some(Parsed {
        time,
        level: level.unwrap_or(Level::Info),
        message: message.to_string(),
        format: Format::Json,
    })
}

/// zap and others write `"ts": 1793215816.123` (seconds with a fraction).
fn float_epoch(f: f64) -> Option<DateTime<Utc>> {
    let ms = if f >= 1e12 { f } else { f * 1000.0 };
    if !(1e12..1e14).contains(&ms) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)] // range checked above
    DateTime::from_timestamp_millis(ms.round() as i64)
}

fn syslog5424(line: &str, ctx: &ParseCtx) -> Option<Parsed> {
    let c = SYSLOG5424.captures(line)?;
    Some(Parsed {
        time: parse_timestamp(&c[2], &ctx.zone),
        level: syslog_level(&c[1]),
        message: c[3].to_string(),
        format: Format::Syslog5424,
    })
}

fn syslog3164(line: &str, ctx: &ParseCtx) -> Option<Parsed> {
    let c = SYSLOG3164.captures(line)?;
    let stamp = format!("{} {}", ctx.year, c[2].replace("  ", " "));
    let naive = NaiveDateTime::parse_from_str(&stamp, "%Y %b %e %H:%M:%S").ok()?;
    Some(Parsed {
        time: Some(ctx.zone.to_utc(naive)),
        level: syslog_level(&c[1]),
        message: c[3].to_string(),
        format: Format::Syslog3164,
    })
}

/// nginx/Apache combined: 5xx is an error, everything else (bot scans' 4xx too) is info.
fn combined(line: &str) -> Option<Parsed> {
    let c = COMBINED.captures(line)?;
    let time = DateTime::parse_from_str(&c[1], "%d/%b/%Y:%H:%M:%S %z").ok()?;
    let status = &c[3];
    Some(Parsed {
        time: Some(time.with_timezone(&Utc)),
        level: if status.starts_with('5') {
            Level::Error
        } else {
            Level::Info
        },
        message: format!("{} {status}", &c[2]),
        format: Format::Combined,
    })
}

fn postgres(line: &str, ctx: &ParseCtx) -> Option<Parsed> {
    let c = POSTGRES.captures(line)?;
    let time = match c.get(2).map(|m| m.as_str()) {
        Some("UTC") => parse_timestamp(&format!("{} UTC", &c[1]), &ctx.zone),
        _ => parse_timestamp(&c[1], &ctx.zone),
    };
    let level = match &c[3] {
        "ERROR" | "FATAL" | "PANIC" => Level::Error,
        "WARNING" => Level::Warn,
        _ => Level::Info,
    };
    Some(Parsed {
        time,
        level,
        message: c[4].to_string(),
        format: Format::Postgres,
    })
}

fn logfmt(line: &str, ctx: &ParseCtx) -> Option<Parsed> {
    let pairs: Vec<(&str, String)> = LOGFMT_PAIR
        .captures_iter(line)
        .map(|c| {
            let value = c.get(2).map_or("", |m| m.as_str());
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            (
                c.get(1).map_or("", |m| m.as_str()),
                value.replace("\\\"", "\""),
            )
        })
        .collect();
    let get = |keys: &[&str]| {
        pairs
            .iter()
            .find(|(k, _)| keys.contains(k))
            .map(|(_, v)| v.as_str())
    };
    let time = get(&TIME_KEYS)?;
    if pairs.len() < 2 {
        return None;
    }
    Some(Parsed {
        time: parse_timestamp(time, &ctx.zone),
        level: get(&LEVEL_KEYS)
            .and_then(Level::from_word)
            .unwrap_or(Level::Info),
        message: get(&MESSAGE_KEYS).unwrap_or(line).to_string(),
        format: Format::Logfmt,
    })
}

/// A line that starts with a timestamp in its first one or two tokens.
fn plain(line: &str, ctx: &ParseCtx) -> Option<Parsed> {
    let mut tokens = line.splitn(3, ' ');
    let first = tokens.next()?;
    // Cheap guard before trying date formats on every token.
    if !first.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (time, rest) = match tokens.next() {
        Some(second) if NaiveDate::parse_from_str(first, "%Y-%m-%d").is_ok() => {
            let rest = tokens.next().unwrap_or("");
            (
                parse_timestamp(&format!("{first} {second}"), &ctx.zone)?,
                rest.to_string(),
            )
        }
        _ => {
            let t = parse_timestamp(first, &ctx.zone)?;
            (t, line[first.len()..].trim_start().to_string())
        }
    };
    let words: Vec<&str> = rest.split_whitespace().take(3).collect();
    let level = words.iter().find_map(|w| level_word(w));
    // Drop a leading level token ("[WARN] cache miss" → "cache miss").
    let message = match words.first() {
        Some(w) if level_word(w).is_some() => rest[rest.find(w).unwrap_or(0) + w.len()..]
            .trim_start()
            .to_string(),
        _ => rest,
    };
    Some(Parsed {
        time: Some(time),
        level: level.unwrap_or(Level::Info),
        message,
        format: Format::Plain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ParseCtx {
        ParseCtx {
            zone: "UTC".parse().unwrap(),
            year: 2026,
        }
    }

    #[allow(clippy::unnecessary_wraps)] // compared against `Parsed::time`
    fn at(s: &str) -> Option<DateTime<Utc>> {
        Some(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    #[test]
    fn json() {
        let p = parse_line(
            r#"{"ts":"2026-10-28T10:15:00Z","level":"error","msg":"db pool exhausted"}"#,
            &ctx(),
        );
        assert_eq!(p.format, Format::Json);
        assert_eq!(p.level, Level::Error);
        assert_eq!(p.message, "db pool exhausted");
        assert_eq!(p.time, at("2026-10-28T10:15:00Z"));
    }

    #[test]
    fn json_without_message_keeps_line() {
        let line = r#"{"time":"2026-10-28T10:15:00Z","event":"login"}"#;
        let p = parse_line(line, &ctx());
        assert_eq!(
            (p.format, p.level, p.message.as_str()),
            (Format::Json, Level::Info, line)
        );
    }

    #[test]
    fn logfmt() {
        let p = parse_line(
            r#"time=2026-10-28T10:15:00Z level=warn msg="slow query" ms=812"#,
            &ctx(),
        );
        assert_eq!(p.format, Format::Logfmt);
        assert_eq!(p.level, Level::Warn);
        assert_eq!(p.message, "slow query");
        assert_eq!(p.time, at("2026-10-28T10:15:00Z"));
    }

    #[test]
    fn syslog5424() {
        let p = parse_line(
            "<11>1 2026-10-28T10:15:00Z host app 123 - - disk full",
            &ctx(),
        );
        assert_eq!(p.format, Format::Syslog5424);
        assert_eq!(p.level, Level::Error);
        assert_eq!(p.message, "disk full");
        assert_eq!(p.time, at("2026-10-28T10:15:00Z"));
    }

    #[test]
    fn syslog3164() {
        let p = parse_line("<36>Oct 28 10:15:00 host sshd[42]: bad login", &ctx());
        assert_eq!(p.format, Format::Syslog3164);
        assert_eq!(p.level, Level::Warn);
        assert_eq!(p.time, at("2026-10-28T10:15:00Z"));
        assert!(p.message.contains("bad login"));
    }

    #[test]
    fn combined_5xx_and_4xx() {
        let line =
            r#"1.2.3.4 - - [28/Oct/2026:10:15:00 +0000] "GET /api HTTP/1.1" 502 157 "-" "curl""#;
        let p = parse_line(line, &ctx());
        assert_eq!((p.format, p.level), (Format::Combined, Level::Error));
        assert_eq!(p.time, at("2026-10-28T10:15:00Z"));
        assert!(p.message.contains("GET /api") && p.message.contains("502"));
        let p = parse_line(&line.replace(" 502 ", " 404 "), &ctx());
        assert_eq!((p.format, p.level), (Format::Combined, Level::Info));
    }

    #[test]
    fn postgres() {
        let p = parse_line(
            "2026-10-28 10:15:00.123 UTC [77] FATAL:  sorry, too many clients already",
            &ctx(),
        );
        assert_eq!(p.format, Format::Postgres);
        assert_eq!(p.level, Level::Error);
        assert_eq!(p.message, "sorry, too many clients already");
        assert_eq!(p.time, at("2026-10-28T10:15:00.123Z"));
        let p = parse_line(
            "2026-10-28 10:15:00.123 UTC [77] WARNING:  checkpoints too frequent",
            &ctx(),
        );
        assert_eq!(p.level, Level::Warn);
        let p = parse_line(
            "2026-10-28 10:15:00.123 UTC [77] LOG:  checkpoint starting",
            &ctx(),
        );
        assert_eq!(p.level, Level::Info);
    }

    #[test]
    fn plain_with_level() {
        let p = parse_line("2026-10-28 10:15:00,001 [WARN] cache miss", &ctx());
        assert_eq!(p.format, Format::Plain);
        assert_eq!(p.level, Level::Warn);
        assert_eq!(p.message, "cache miss");
        assert_eq!(p.time, at("2026-10-28T10:15:00.001Z"));
    }

    #[test]
    fn plain_rfc3339_without_level() {
        let p = parse_line("2026-10-28T10:15:00Z server started", &ctx());
        assert_eq!(
            (p.format, p.level, p.message.as_str()),
            (Format::Plain, Level::Info, "server started")
        );
    }

    #[test]
    fn stack_trace_line() {
        let p = parse_line("    at handler (app.js:12)", &ctx());
        assert_eq!((p.time, p.format), (None, Format::Unparsed));
    }

    #[test]
    fn garbage() {
        let p = parse_line("%%% not a log %%%", &ctx());
        assert_eq!(
            (p.format, p.level, p.message.as_str()),
            (Format::Unparsed, Level::Info, "%%% not a log %%%")
        );
    }

    #[test]
    fn json_float_epoch() {
        let p = parse_line(
            r#"{"level":"error","ts":1793215816.123,"msg":"boom"}"#,
            &ctx(),
        );
        assert_eq!(p.time, at("2026-10-28T19:30:16.123Z"));
        let p = parse_line(r#"{"level":"info","ts":1793215816123,"msg":"ms"}"#, &ctx());
        assert_eq!(p.time, at("2026-10-28T19:30:16.123Z"));
    }
}
