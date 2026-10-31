use anyhow::{Context, Result, anyhow, bail};
use chrono::offset::LocalResult;
use chrono::{DateTime, FixedOffset, NaiveDateTime, Offset, TimeDelta, TimeZone, Utc};
use std::str::FromStr;

/// The zone for timestamps that carry none, and for display.
#[derive(Debug, Clone)]
pub enum Zone {
    Fixed(FixedOffset),
    Named(chrono_tz::Tz),
}

impl FromStr for Zone {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        if s.eq_ignore_ascii_case("utc") || s == "Z" {
            return Ok(Zone::Fixed(
                FixedOffset::east_opt(0).expect("zero is a valid offset"),
            ));
        }
        if let Ok(offset) = FixedOffset::from_str(s) {
            return Ok(Zone::Fixed(offset));
        }
        if let Some(offset) = compact_offset(s) {
            return Ok(Zone::Fixed(offset));
        }
        s.parse::<chrono_tz::Tz>().map(Zone::Named).map_err(|_| {
            anyhow!("unknown time zone {s:?} (use UTC, +02:00 or a name like Europe/Vienna)")
        })
    }
}

/// `-0500` / `+0200`.
fn compact_offset(s: &str) -> Option<FixedOffset> {
    let (sign, digits) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

impl Zone {
    /// Ambiguous times (the fall-back hour) take the earliest; times in the spring-forward gap
    /// use the offset in force just before the gap.
    pub fn to_utc(&self, naive: NaiveDateTime) -> DateTime<Utc> {
        match self {
            Zone::Fixed(offset) => resolve(offset, naive),
            Zone::Named(tz) => resolve(tz, naive),
        }
    }

    pub fn display(&self, t: DateTime<Utc>, fmt: &str) -> String {
        match self {
            Zone::Fixed(offset) => t.with_timezone(offset).format(fmt).to_string(),
            Zone::Named(tz) => t.with_timezone(tz).format(fmt).to_string(),
        }
    }
}

fn resolve<Z: TimeZone>(zone: &Z, naive: NaiveDateTime) -> DateTime<Utc> {
    match zone.from_local_datetime(&naive) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.with_timezone(&Utc),
        LocalResult::None => {
            let before = zone.offset_from_utc_datetime(&(naive - TimeDelta::hours(3)));
            let seconds = before.fix().local_minus_utc();
            (naive - TimeDelta::seconds(i64::from(seconds))).and_utc()
        }
    }
}

/// `+2s`, `-1500ms`, `90s`, `5m`, `1h`.
pub fn parse_duration(s: &str) -> Result<TimeDelta> {
    let digits_end = s
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || (*i == 0 && (*c == '+' || *c == '-'))))
        .map_or(s.len(), |(i, _)| i);
    let (number, unit) = s.split_at(digits_end);
    let n: i64 = number
        .parse()
        .with_context(|| format!("bad duration {s:?} (use e.g. 90s, -1500ms, 5m, 1h)"))?;
    match unit {
        "ms" => Ok(TimeDelta::milliseconds(n)),
        "s" => Ok(TimeDelta::seconds(n)),
        "m" => Ok(TimeDelta::minutes(n)),
        "h" => Ok(TimeDelta::hours(n)),
        _ => bail!("bad duration {s:?}: the unit must be ms, s, m or h"),
    }
}

/// `NAME=DURATION`, for `--shift db=+2s`.
pub fn parse_shift(s: &str) -> Result<(String, TimeDelta)> {
    let Some((name, duration)) = s.split_once('=') else {
        bail!("bad shift {s:?}: expected NAME=DURATION, e.g. db=+2s");
    };
    Ok((name.to_string(), parse_duration(duration)?))
}

/// The whole string must be one timestamp: RFC 3339, a naive date-time (in `zone`), the same
/// followed by ` UTC`, or epoch seconds (10 digits) / milliseconds (13 digits).
pub fn parse_timestamp(s: &str, zone: &Zone) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        let n: i64 = s.parse().ok()?;
        return match s.len() {
            10 => DateTime::from_timestamp(n, 0),
            13 => DateTime::from_timestamp_millis(n),
            _ => None,
        };
    }
    if let Some(rest) = s.strip_suffix(" UTC") {
        return parse_naive(rest).map(|n| n.and_utc());
    }
    parse_naive(s).map(|n| zone.to_utc(n))
}

fn parse_naive(s: &str) -> Option<NaiveDateTime> {
    let s = s.replacen(',', ".", 1);
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(&s, fmt).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn naive(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn vienna() -> Zone {
        "Europe/Vienna".parse().unwrap()
    }

    #[test]
    fn zone_parse() {
        assert!("Europe/Vienna".parse::<Zone>().is_ok());
        assert!("+02:00".parse::<Zone>().is_ok());
        assert!("-0500".parse::<Zone>().is_ok());
        assert!("Z".parse::<Zone>().is_ok());
        assert!("UTC".parse::<Zone>().is_ok());
        let err = "Mars/Base".parse::<Zone>().unwrap_err();
        assert!(err.to_string().contains("Mars/Base"));
    }

    #[test]
    fn naive_in_vienna() {
        assert_eq!(
            vienna().to_utc(naive("2026-07-01 12:00:00")),
            utc("2026-07-01T10:00:00Z")
        );
        let plus2: Zone = "+02:00".parse().unwrap();
        assert_eq!(
            plus2.to_utc(naive("2026-07-01 12:00:00")),
            utc("2026-07-01T10:00:00Z")
        );
    }

    #[test]
    fn dst_gap_and_fold() {
        assert_eq!(
            vienna().to_utc(naive("2026-03-29 02:30:00")),
            utc("2026-03-29T01:30:00Z")
        );
        assert_eq!(
            vienna().to_utc(naive("2026-10-25 02:30:00")),
            utc("2026-10-25T00:30:00Z")
        );
    }

    #[test]
    fn display_in_zone() {
        assert_eq!(
            vienna().display(utc("2026-10-28T19:30:16Z"), "%H:%M:%S"),
            "20:30:16"
        );
    }

    #[test]
    fn timestamps() {
        let z: Zone = "UTC".parse().unwrap();
        let want = utc("2026-10-28T19:30:16.123Z");
        assert_eq!(parse_timestamp("2026-10-28T19:30:16.123Z", &z), Some(want));
        assert_eq!(
            parse_timestamp("2026-10-28T21:30:16.123+02:00", &z),
            Some(want)
        );
        assert_eq!(parse_timestamp("2026-10-28 19:30:16,123", &z), Some(want));
        assert_eq!(parse_timestamp("2026-10-28T19:30:16.123", &z), Some(want));
        assert_eq!(
            parse_timestamp("2026-10-28 19:30:16.123 UTC", &z),
            Some(want)
        );
        assert_eq!(parse_timestamp("1793215816123", &z), Some(want));
        assert_eq!(
            parse_timestamp("1793215816", &z),
            Some(utc("2026-10-28T19:30:16Z"))
        );
        assert_eq!(
            parse_timestamp("2026-10-28 20:30:16", &vienna()),
            Some(utc("2026-10-28T19:30:16Z"))
        );
        assert_eq!(parse_timestamp("yesterday", &z), None);
        assert_eq!(parse_timestamp("42", &z), None);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("+2s").unwrap(), TimeDelta::seconds(2));
        assert_eq!(
            parse_duration("-1500ms").unwrap(),
            TimeDelta::milliseconds(-1500)
        );
        assert_eq!(parse_duration("1h").unwrap(), TimeDelta::hours(1));
        assert_eq!(parse_duration("90s").unwrap(), TimeDelta::seconds(90));
        assert_eq!(parse_duration("5m").unwrap(), TimeDelta::minutes(5));
        assert!(parse_duration("2x").is_err());
        assert_eq!(
            parse_shift("db=+2s").unwrap(),
            ("db".to_string(), TimeDelta::seconds(2))
        );
        assert!(parse_shift("db").is_err());
    }
}
