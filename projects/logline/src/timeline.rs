//! Merged lines in, a timeline out: incidents from raw lines, then collapsing, then the cap.

use crate::{Level, Line};
use anyhow::Result;
use chrono::{DateTime, TimeDelta, Utc};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeSet, VecDeque};
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub source: usize,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// The highest level in the group.
    pub level: Level,
    pub message: String,
    pub count: u64,
    /// Raw text of the first 5 and last 5 lines (all of them when 10 or fewer).
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Incident {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sources: Vec<usize>,
    pub lines: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Truncation {
    pub cut: DateTime<Utc>,
    pub dropped: u64,
    pub incidents_after: usize,
}

pub struct Options {
    pub collapse_gap: TimeDelta,
    pub incident_min: usize,
    pub incident_window: TimeDelta,
    pub cap: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            collapse_gap: TimeDelta::seconds(60),
            incident_min: 5,
            incident_window: TimeDelta::seconds(60),
            cap: 50_000,
        }
    }
}

pub struct Timeline {
    pub sources: Vec<String>,
    pub entries: Vec<Entry>,
    pub incidents: Vec<Incident>,
    pub truncated: Option<Truncation>,
    pub lines_read: u64,
}

static MASKS: LazyLock<[(Regex, &str); 4]> = LazyLock::new(|| {
    let re = |s: &str| Regex::new(s).expect("valid regex");
    [
        (
            re(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b"),
            "<id>",
        ),
        (re(r"\b\d{1,3}(?:\.\d{1,3}){3}(?::\d+)?\b"), "<ip>"),
        (re(r"(?i)\b(?:0x[0-9a-f]+|[0-9a-f]{8,})\b"), "<hex>"),
        (re(r"\d+(?:\.\d+)?"), "<n>"),
    ]
});

/// The message with ids, addresses, hex and numbers masked, so repeats compare equal.
pub fn template(message: &str) -> String {
    MASKS.iter().fold(message.to_string(), |out, (re, mask)| {
        re.replace_all(&out, *mask).into_owned()
    })
}

struct Group {
    template: String,
    entry: Entry,
    tail: VecDeque<String>,
}

impl Group {
    fn new(line: Line) -> Self {
        Group {
            template: template(&line.message),
            entry: Entry {
                source: line.source,
                start: line.time,
                end: line.time,
                level: line.level,
                message: line.message,
                count: 1,
                samples: vec![line.raw],
            },
            tail: VecDeque::new(),
        }
    }

    fn add(&mut self, line: Line) {
        let e = &mut self.entry;
        e.end = line.time;
        e.level = e.level.max(line.level);
        e.count += 1;
        if e.samples.len() < 5 {
            e.samples.push(line.raw);
        } else {
            if self.tail.len() == 5 {
                self.tail.pop_front();
            }
            self.tail.push_back(line.raw);
        }
    }

    fn finish(mut self) -> Entry {
        self.entry.samples.extend(self.tail);
        self.entry
    }
}

#[derive(Default)]
struct Incidents {
    recent: VecDeque<(DateTime<Utc>, usize)>,
    open: Option<(Incident, BTreeSet<usize>)>,
    done: Vec<Incident>,
}

impl Incidents {
    fn see(&mut self, line: &Line, opts: &Options) {
        if line.level < Level::Warn {
            return;
        }
        if let Some((inc, sources)) = &mut self.open {
            if line.time - inc.end <= opts.incident_window {
                inc.end = line.time;
                inc.lines += 1;
                sources.insert(line.source);
                return;
            }
            self.close();
        }
        self.recent.push_back((line.time, line.source));
        while self
            .recent
            .front()
            .is_some_and(|(t, _)| line.time - *t > opts.incident_window)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= opts.incident_min {
            let sources: BTreeSet<usize> = self.recent.iter().map(|(_, s)| *s).collect();
            let start = self.recent.front().map_or(line.time, |(t, _)| *t);
            let lines = self.recent.len() as u64;
            self.recent.clear();
            let inc = Incident {
                start,
                end: line.time,
                sources: Vec::new(),
                lines,
            };
            self.open = Some((inc, sources));
        }
    }

    fn close(&mut self) {
        if let Some((mut inc, sources)) = self.open.take() {
            inc.sources = sources.into_iter().collect();
            self.done.push(inc);
        }
    }
}

pub fn build(
    sources: Vec<String>,
    lines: impl Iterator<Item = Result<Line>>,
    opts: &Options,
) -> Result<Timeline> {
    let mut groups: Vec<Option<Group>> = (0..sources.len()).map(|_| None).collect();
    let mut incidents = Incidents::default();
    let mut entries = Vec::new();
    let mut dropped = 0u64;
    let mut cut: Option<DateTime<Utc>> = None;
    let mut lines_read = 0u64;

    let mut keep = |entry: Entry, entries: &mut Vec<Entry>| {
        if entries.len() < opts.cap {
            entries.push(entry);
        } else {
            dropped += 1;
            cut = Some(cut.map_or(entry.start, |c| c.min(entry.start)));
        }
    };

    for line in lines {
        let line = line?;
        lines_read += 1;
        incidents.see(&line, opts);
        let slot = &mut groups[line.source];
        match slot {
            Some(g)
                if g.template == template(&line.message)
                    && line.time - g.entry.end <= opts.collapse_gap =>
            {
                g.add(line);
            }
            _ => {
                if let Some(g) = slot.replace(Group::new(line)) {
                    keep(g.finish(), &mut entries);
                }
            }
        }
    }
    for g in groups.into_iter().flatten() {
        keep(g.finish(), &mut entries);
    }
    incidents.close();

    entries.sort_by_key(|e| (e.start, e.source));
    let truncated = cut.map(|cut| {
        // A clean time cut: nothing after `cut` stays, even what fit under the cap.
        let kept = entries.partition_point(|e| e.start < cut);
        dropped += (entries.len() - kept) as u64;
        entries.truncate(kept);
        Truncation {
            cut,
            dropped,
            incidents_after: incidents.done.iter().filter(|i| i.start >= cut).count(),
        }
    });
    Ok(Timeline {
        sources,
        entries,
        incidents: incidents.done,
        truncated,
        lines_read,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-28T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn line(source: usize, secs: i64, level: Level, msg: &str) -> Line {
        Line {
            source,
            seq: 0,
            time: base() + TimeDelta::seconds(secs),
            level,
            message: msg.to_string(),
            raw: msg.to_string(),
        }
    }

    fn run(lines: Vec<Line>, opts: &Options) -> Timeline {
        build(
            vec!["a".into(), "b".into(), "c".into()],
            lines.into_iter().map(Ok),
            opts,
        )
        .unwrap()
    }

    #[test]
    fn template_masks() {
        let a = template(
            "conn 10.0.0.5:5432 refused after 3 tries id=3f2b9c4e-8a1d-4f6b-9c2e-5d7a1b3c4e5f",
        );
        let b = template(
            "conn 10.0.0.9:6543 refused after 12 tries id=a1b2c3d4-e5f6-4789-abcd-ef0123456789",
        );
        assert_eq!(a, b);
        assert_eq!(
            template("addr 0xdeadbeef took 1.5 ms"),
            template("addr 0x1f took 22 ms")
        );
        assert_ne!(template("conn refused"), template("conn accepted"));
    }

    #[test]
    fn collapses_repeats() {
        let lines = (1..=512)
            .map(|i| line(0, i, Level::Info, &format!("connection refused {i}")))
            .collect();
        let t = run(lines, &Options::default());
        assert_eq!(t.entries.len(), 1);
        let e = &t.entries[0];
        assert_eq!(e.count, 512);
        assert_eq!(e.samples.len(), 10);
        assert_eq!(e.samples[0], "connection refused 1");
        assert_eq!(e.samples[9], "connection refused 512");
        assert_eq!(
            (e.start, e.end),
            (
                base() + TimeDelta::seconds(1),
                base() + TimeDelta::seconds(512)
            )
        );
        assert_eq!(t.lines_read, 512);
    }

    #[test]
    fn gap_breaks_group() {
        let t = run(
            vec![line(0, 0, Level::Info, "x"), line(0, 61, Level::Info, "x")],
            &Options::default(),
        );
        assert_eq!(t.entries.len(), 2);
        let t = run(
            vec![line(0, 0, Level::Info, "x"), line(0, 60, Level::Info, "x")],
            &Options::default(),
        );
        assert_eq!(t.entries.len(), 1);
    }

    #[test]
    fn different_messages_not_collapsed() {
        let lines = (0..4)
            .map(|i| line(0, i, Level::Info, if i % 2 == 0 { "a" } else { "b" }))
            .collect();
        assert_eq!(run(lines, &Options::default()).entries.len(), 4);
    }

    #[test]
    fn group_level_is_highest() {
        let t = run(
            vec![
                line(0, 0, Level::Info, "x 1"),
                line(0, 1, Level::Error, "x 2"),
            ],
            &Options::default(),
        );
        assert_eq!(t.entries[0].level, Level::Error);
    }

    #[test]
    fn incident_thresholds() {
        let burst = |start: i64, n: i64| {
            (0..n).map(move |i| line(0, start + i * 10, Level::Error, &format!("e{i}")))
        };
        assert_eq!(
            run(burst(0, 5).collect(), &Options::default())
                .incidents
                .len(),
            1
        );
        assert_eq!(
            run(burst(0, 4).collect(), &Options::default())
                .incidents
                .len(),
            0
        );
        let two: Vec<Line> = burst(0, 5).chain(burst(3600, 5)).collect();
        assert_eq!(run(two, &Options::default()).incidents.len(), 2);
    }

    #[test]
    fn incident_counts_raw_lines() {
        let lines = (0..500)
            .map(|i| line(0, i / 10, Level::Error, "db pool exhausted, retrying"))
            .collect();
        let t = run(lines, &Options::default());
        assert_eq!(t.entries.len(), 1);
        assert_eq!(t.incidents.len(), 1);
        assert_eq!(t.incidents[0].lines, 500);
    }

    #[test]
    fn incident_sources() {
        let lines = (0..6)
            .map(|i| {
                line(
                    if i % 2 == 0 { 0 } else { 2 },
                    i,
                    Level::Warn,
                    &format!("w{i}"),
                )
            })
            .collect();
        assert_eq!(run(lines, &Options::default()).incidents[0].sources, [0, 2]);
    }

    #[test]
    fn cap_cuts_by_time() {
        let opts = Options {
            cap: 4,
            incident_min: 3,
            ..Options::default()
        };
        let lines = (0..10)
            .map(|i| {
                line(
                    0,
                    i,
                    if i >= 7 { Level::Error } else { Level::Info },
                    &format!("distinct {}", "x".repeat(usize::try_from(i).unwrap())),
                )
            })
            .collect();
        let t = run(lines, &opts);
        assert_eq!(t.entries.len(), 4);
        let cut = t.truncated.expect("truncated");
        assert_eq!(cut.dropped, 6);
        assert_eq!(cut.cut, base() + TimeDelta::seconds(4));
        assert_eq!(cut.incidents_after, 1);
    }
}
