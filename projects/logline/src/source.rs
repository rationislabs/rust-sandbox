//! Streams of lines per source: continuation lines attached, docker stdin split, k-way merge.

use crate::{Format, Level, ParseCtx, parse_line};
use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeDelta, Utc};
use regex::Regex;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub source: usize,
    pub seq: u64,
    pub time: DateTime<Utc>,
    pub level: Level,
    pub message: String,
    /// The original line plus attached continuation lines, joined with `\n`.
    pub raw: String,
}

pub struct Input {
    pub name: String,
    pub reader: Box<dyn BufRead>,
    /// Lines start with Docker's own timestamp (`docker compose logs -t`).
    pub docker: bool,
}

pub struct LineIter {
    source: usize,
    name: String,
    reader: Box<dyn BufRead>,
    ctx: Arc<ParseCtx>,
    shift: TimeDelta,
    docker: bool,
    seq: u64,
    buf: Vec<u8>,
    pending: Option<Line>,
    /// Lines before the first timestamp; they take that timestamp.
    leading: Vec<String>,
    seen_time: bool,
    done: bool,
    /// RFC 3164 syslog: years past `ctx.year`, and the month of the last such line.
    syslog_years: i32,
    syslog_month: u32,
}

impl LineIter {
    pub fn new(
        source: usize,
        name: String,
        reader: Box<dyn BufRead>,
        ctx: Arc<ParseCtx>,
        shift: TimeDelta,
        docker: bool,
    ) -> Self {
        Self {
            source,
            name,
            reader,
            ctx,
            shift,
            docker,
            seq: 0,
            buf: Vec::new(),
            pending: None,
            leading: Vec::new(),
            seen_time: false,
            done: false,
            syslog_years: 0,
            syslog_month: 0,
        }
    }

    fn read_line(&mut self) -> Result<Option<String>> {
        self.buf.clear();
        let n = self
            .reader
            .read_until(b'\n', &mut self.buf)
            .with_context(|| format!("{}: read error", self.name))?;
        if n == 0 {
            return Ok(None);
        }
        let mut line = String::from_utf8_lossy(&self.buf).into_owned();
        if line.ends_with('\n') {
            line.pop();
        }
        if line.ends_with('\r') {
            line.pop();
        }
        Ok(Some(line))
    }

    /// Time, level and message; `None` time = attach to the line before.
    fn parse(&mut self, line: &str) -> (Option<DateTime<Utc>>, Level, String) {
        if self.docker
            && let Some((stamp, rest)) = line.split_once(' ')
            && let Ok(time) = DateTime::parse_from_rfc3339(stamp)
        {
            let parsed = parse_line(rest, &self.ctx);
            // Docker stamps every line; an indented untimed rest is still a continuation.
            if parsed.time.is_none() && rest.starts_with(char::is_whitespace) {
                return (None, parsed.level, rest.to_string());
            }
            return (Some(time.with_timezone(&Utc)), parsed.level, parsed.message);
        }
        let parsed = parse_line(line, &self.ctx);
        let time = match parsed.time {
            Some(t) if parsed.format == Format::Syslog3164 => Some(self.syslog_year(t)),
            t => t,
        };
        (time, parsed.level, parsed.message)
    }

    /// RFC 3164 lines carry no year: a month well before the last one (December, then January)
    /// starts the next year.
    // ponytail: Feb 29 moved into a year without one keeps the parsed year.
    fn syslog_year(&mut self, time: DateTime<Utc>) -> DateTime<Utc> {
        if self.syslog_month > time.month() + 6 {
            self.syslog_years += 1;
        }
        self.syslog_month = time.month();
        time.with_year(time.year() + self.syslog_years)
            .unwrap_or(time)
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq - 1
    }
}

impl Iterator for LineIter {
    type Item = Result<Line>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            let line = match self.read_line() {
                Ok(Some(line)) => line,
                Ok(None) => {
                    self.done = true;
                    break;
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            };
            let (time, level, message) = self.parse(&line);
            match time {
                Some(time) => {
                    let mut raw = line;
                    if !self.seen_time && !self.leading.is_empty() {
                        raw = format!("{}\n{raw}", self.leading.join("\n"));
                        self.leading.clear();
                    }
                    self.seen_time = true;
                    let Some(time) = time.checked_add_signed(self.shift) else {
                        self.done = true;
                        return Some(Err(anyhow::anyhow!(
                            "--shift moves {} out of the time range",
                            self.name
                        )));
                    };
                    let new = Line {
                        source: self.source,
                        seq: self.next_seq(),
                        time,
                        level,
                        message,
                        raw,
                    };
                    if let Some(old) = self.pending.replace(new) {
                        return Some(Ok(old));
                    }
                }
                None => match &mut self.pending {
                    Some(p) => {
                        p.raw.push('\n');
                        p.raw.push_str(&line);
                    }
                    // ponytail: a file with no timestamp at all is buffered whole.
                    None => self.leading.push(line),
                },
            }
        }
        if let Some(p) = self.pending.take() {
            return Some(Ok(p));
        }
        if !self.leading.is_empty() {
            eprintln!("warning: no timestamps in {}", self.name);
            let raw = std::mem::take(&mut self.leading).join("\n");
            return Some(Ok(Line {
                source: self.source,
                seq: self.next_seq(),
                time: DateTime::UNIX_EPOCH,
                level: Level::Info,
                message: raw.lines().next().unwrap_or("").to_string(),
                raw,
            }));
        }
        None
    }
}

/// A folder under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static DOCKER_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([\w.-]+)\s*\|\s(\d{4}-\d\d-\d\dT\S+Z)\s?(.*)$").expect("valid regex")
});

/// A new, private (0700) folder with an unpredictable name; never an existing one.
fn private_temp_dir() -> Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let mut last = None;
    for _ in 0..8 {
        let dir = std::env::temp_dir().join(format!(
            "logline-{}-{:016x}",
            std::process::id(),
            fastrand::u64(..)
        ));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) => last = Some(e),
        }
    }
    Err(last.map_or_else(|| anyhow::anyhow!("no temp dir"), anyhow::Error::from))
        .context("cannot create a temp folder")
}

/// A gzip stream, known by its magic bytes whatever the file is called, is decompressed as it
/// is read.
fn gunzip(mut reader: Box<dyn BufRead>) -> Result<Box<dyn BufRead>> {
    if reader.fill_buf()?.starts_with(&[0x1f, 0x8b]) {
        let gz = flate2::bufread::MultiGzDecoder::new(reader);
        return Ok(Box::new(BufReader::new(gz)));
    }
    Ok(reader)
}

/// Files by path (gzipped or not); `-` is stdin.
pub fn open_inputs(paths: &[PathBuf]) -> Result<(Vec<Input>, Option<TempDir>)> {
    let mut inputs = Vec::new();
    let mut tmp = None;
    for path in paths {
        if path.as_os_str() == "-" {
            let (stdin_inputs, dir) = open_reader_inputs(Box::new(std::io::stdin().lock()))?;
            inputs.extend(stdin_inputs);
            tmp = dir;
            continue;
        }
        let file = File::open(path).with_context(|| path.display().to_string())?;
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        inputs.push(Input {
            name: name.strip_suffix(".gz").unwrap_or(&name).to_owned(),
            reader: gunzip(Box::new(BufReader::new(file)))
                .with_context(|| path.display().to_string())?,
            docker: false,
        });
    }
    Ok((inputs, tmp))
}

/// Stdin as one source, or split per service when one of the first 10 non-blank lines looks
/// like `docker compose logs -t` (compose can print a preamble such as "Attaching to …").
pub fn open_reader_inputs(reader: Box<dyn BufRead>) -> Result<(Vec<Input>, Option<TempDir>)> {
    let mut reader = gunzip(reader)?;
    let mut head = Vec::new();
    let mut line = Vec::new();
    let mut docker = false;
    let mut seen = 0;
    while seen < 10 && !docker {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        head.extend_from_slice(&line);
        let text = String::from_utf8_lossy(&line);
        if !text.trim().is_empty() {
            seen += 1;
            docker = DOCKER_LINE.is_match(text.trim_end());
        }
    }
    let rest: Box<dyn BufRead> = Box::new(Cursor::new(head).chain(reader));
    if !docker {
        let input = Input {
            name: "stdin".into(),
            reader: rest,
            docker: false,
        };
        return Ok((vec![input], None));
    }
    demux(rest)
}

fn demux(mut reader: Box<dyn BufRead>) -> Result<(Vec<Input>, Option<TempDir>)> {
    let dir = private_temp_dir()?;
    let tmp = TempDir(dir.clone());
    let mut names: Vec<String> = Vec::new();
    let mut writers: Vec<BufWriter<File>> = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&buf);
        let text = text.trim_end_matches(['\n', '\r']);
        let (name, out) = match DOCKER_LINE.captures(text) {
            Some(c) => (c[1].to_string(), format!("{} {}", &c[2], &c[3])),
            None => ("stdin".to_string(), text.to_string()),
        };
        let index = if let Some(i) = names.iter().position(|n| *n == name) {
            i
        } else {
            let file = File::create(dir.join(format!("{}.log", names.len())))?;
            names.push(name);
            writers.push(BufWriter::new(file));
            names.len() - 1
        };
        writeln!(writers[index], "{out}")?;
    }
    let mut inputs = Vec::new();
    for (i, (name, mut writer)) in names.into_iter().zip(writers).enumerate() {
        writer.flush()?;
        let file = File::open(dir.join(format!("{i}.log")))?;
        inputs.push(Input {
            docker: name != "stdin",
            name,
            reader: Box::new(BufReader::new(file)),
        });
    }
    Ok((inputs, Some(tmp)))
}

/// K-way merge by (time, source, seq): stable across equal times.
pub fn merge(iters: Vec<LineIter>) -> impl Iterator<Item = Result<Line>> {
    let mut iters = iters;
    let mut heap = BinaryHeap::new();
    let mut heads: Vec<Option<Line>> = Vec::with_capacity(iters.len());
    let mut first_error = None;
    for (i, it) in iters.iter_mut().enumerate() {
        match it.next() {
            Some(Ok(line)) => {
                heap.push(Reverse((line.time, line.source, line.seq, i)));
                heads.push(Some(line));
            }
            Some(Err(e)) => {
                first_error.get_or_insert(e);
                heads.push(None);
            }
            None => heads.push(None),
        }
    }
    let mut first_error = first_error;
    std::iter::from_fn(move || {
        if let Some(e) = first_error.take() {
            return Some(Err(e));
        }
        let Reverse((_, _, _, i)) = heap.pop()?;
        let line = heads[i].take()?;
        match iters[i].next() {
            Some(Ok(next)) => {
                heap.push(Reverse((next.time, next.source, next.seq, i)));
                heads[i] = Some(next);
            }
            Some(Err(e)) => first_error = Some(e),
            None => {}
        }
        Some(Ok(line))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Arc<ParseCtx> {
        Arc::new(ParseCtx {
            zone: "UTC".parse().unwrap(),
            year: 2026,
        })
    }

    fn lines_of(bytes: &[u8], shift: TimeDelta) -> Vec<Line> {
        let reader: Box<dyn BufRead> = Box::new(Cursor::new(bytes.to_vec()));
        LineIter::new(0, "t".into(), reader, ctx(), shift, false)
            .collect::<Result<Vec<_>>>()
            .unwrap()
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn syslog_crosses_new_year() {
        let got = lines_of(
            b"<13>Dec 31 23:59:58 host app: a\n<13>Jan  1 00:00:01 host app: b\n<13>Mar  3 10:00:00 host app: c\n",
            TimeDelta::zero(),
        );
        assert_eq!(got[0].time, at("2026-12-31T23:59:58Z"));
        assert_eq!(got[1].time, at("2027-01-01T00:00:01Z"));
        assert_eq!(got[2].time, at("2027-03-03T10:00:00Z"));
    }

    #[test]
    fn continuation_attaches() {
        let got = lines_of(
            b"2026-10-28 10:00:00 ERROR boom\n    at a (x.js:1)\n    at b (x.js:2)\n2026-10-28 10:00:01 INFO ok\n",
            TimeDelta::zero(),
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].raw.lines().count(), 3);
        assert_eq!(got[0].level, Level::Error);
        assert_eq!(got[1].message, "ok");
    }

    #[test]
    fn leading_untimed_lines_take_first_time() {
        let got = lines_of(b"boot\n2026-10-28 10:00:00 INFO up\n", TimeDelta::zero());
        assert_eq!(got[0].time, at("2026-10-28T10:00:00Z"));
        assert!(got[0].raw.contains("boot"));
        assert!(
            got.iter()
                .map(|l| l.raw.as_str())
                .collect::<String>()
                .contains("up")
        );
    }

    #[test]
    fn no_timestamps_kept_at_epoch() {
        let got = lines_of(b"a\nb\n", TimeDelta::zero());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].time, DateTime::UNIX_EPOCH);
        assert_eq!(got[0].raw, "a\nb");
    }

    #[test]
    fn lossy_and_crlf() {
        let got = lines_of(b"2026-10-28 10:00:00 INFO caf\xff\r\n", TimeDelta::zero());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "caf\u{fffd}");
        assert!(!got[0].raw.contains('\r'));
    }

    #[test]
    fn last_line_without_newline_is_kept() {
        let got = lines_of(
            b"2026-10-28 10:00:00 INFO a\n2026-10-28 10:00:01 INFO b",
            TimeDelta::zero(),
        );
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn shift_applies() {
        let got = lines_of(b"2026-10-28 10:00:00 INFO up\n", TimeDelta::seconds(2));
        assert_eq!(got[0].time, at("2026-10-28T10:00:02Z"));
    }

    #[test]
    fn merge_stable() {
        let a = b"2026-10-28 10:00:01 INFO a1\n2026-10-28 10:00:03 INFO a2\n";
        let b = b"2026-10-28 10:00:01 INFO b1\n2026-10-28 10:00:02 INFO b2\n";
        let c = b"2026-10-28 10:00:00 INFO c1\n2026-10-28 10:00:03 INFO c2\n";
        let iters = [&a[..], &b[..], &c[..]]
            .iter()
            .enumerate()
            .map(|(i, bytes)| {
                let reader: Box<dyn BufRead> = Box::new(Cursor::new(bytes.to_vec()));
                LineIter::new(i, format!("s{i}"), reader, ctx(), TimeDelta::zero(), false)
            })
            .collect();
        let order: Vec<(usize, u64)> = merge(iters)
            .map(|l| l.map(|l| (l.source, l.seq)))
            .collect::<Result<_>>()
            .unwrap();
        assert_eq!(order, [(2, 0), (0, 0), (1, 0), (1, 1), (0, 1), (2, 1)]);
    }

    #[test]
    fn docker_demux() {
        let text = "api-1  | 2026-10-28T10:00:01Z {\"level\":\"error\",\"msg\":\"x\"}\ndb-1   | 2026-10-28T10:00:00Z LOG: ok\n";
        let (inputs, _tmp) =
            open_reader_inputs(Box::new(Cursor::new(text.as_bytes().to_vec()))).unwrap();
        let names: Vec<&str> = inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["api-1", "db-1"]);
        let iters = inputs
            .into_iter()
            .enumerate()
            .map(|(i, input)| {
                LineIter::new(
                    i,
                    input.name,
                    input.reader,
                    ctx(),
                    TimeDelta::zero(),
                    input.docker,
                )
            })
            .collect();
        let lines: Vec<Line> = merge(iters).collect::<Result<_>>().unwrap();
        assert_eq!(lines[0].source, 1);
        assert_eq!(lines[1].level, Level::Error);
        assert_eq!(lines[1].message, "x");
        assert_eq!(lines[1].time, at("2026-10-28T10:00:01Z"));
    }

    #[test]
    fn docker_detected_past_a_preamble() {
        let text = "Attaching to api-1, db-1\napi-1  | 2026-10-28T10:00:01Z ok\ndb-1   | 2026-10-28T10:00:00Z LOG: ok\n";
        let (inputs, _tmp) =
            open_reader_inputs(Box::new(Cursor::new(text.as_bytes().to_vec()))).unwrap();
        let names: Vec<&str> = inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["stdin", "api-1", "db-1"]);
    }

    #[test]
    fn plain_stdin_is_one_source() {
        let (inputs, tmp) = open_reader_inputs(Box::new(Cursor::new(
            b"2026-10-28 10:00:00 INFO a\n".to_vec(),
        )))
        .unwrap();
        assert!(tmp.is_none());
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "stdin");
    }

    #[test]
    fn missing_file_names_path() {
        let err = open_inputs(&[PathBuf::from("does-not-exist.log")])
            .err()
            .unwrap();
        let text = format!("{err:#}"); // how main prints it
        assert!(text.starts_with("does-not-exist.log: "), "{text}");
    }

    #[test]
    fn demux_dir_is_private_and_unique() {
        use std::os::unix::fs::PermissionsExt;
        let text = "api-1  | 2026-10-28T10:00:01Z hi\n";
        let (_a, dir_a) =
            open_reader_inputs(Box::new(Cursor::new(text.as_bytes().to_vec()))).unwrap();
        let (_b, dir_b) =
            open_reader_inputs(Box::new(Cursor::new(text.as_bytes().to_vec()))).unwrap();
        let (a, b) = (dir_a.unwrap(), dir_b.unwrap());
        assert_ne!(a.0, b.0);
        assert_eq!(
            std::fs::metadata(&a.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
