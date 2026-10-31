//! A fixed (seeded) morning of three made-up logs that tell one small story: the database
//! runs out of connections, the API retries hundreds of times, nginx returns 502s, recovery.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

pub const DEMO_SEED: u64 = 2026;
pub const FILES: [&str; 3] = ["api.log", "postgres.log", "nginx.log"];

const FIRST_DAY: NaiveDate = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
const PATHS: [&str; 5] = [
    "/api/orders",
    "/api/orders/42",
    "/api/cart",
    "/api/login",
    "/health",
];
const AGENTS: [&str; 3] = [
    "Mozilla/5.0 (X11; Linux x86_64)",
    "curl/8.10.1",
    "okhttp/4.12.0",
];

struct Day {
    rng: fastrand::Rng,
    api: String,
    pg: String,
    nginx: String,
}

impl Day {
    fn api(&mut self, t: DateTime<Utc>, level: &str, msg: &str) {
        let ts = t.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let _ = writeln!(
            self.api,
            r#"{{"ts":"{ts}","level":"{level}","msg":"{msg}"}}"#
        );
    }

    fn pg(&mut self, t: DateTime<Utc>, level: &str, msg: &str) {
        let pid = self.rng.u32(70..90);
        let _ = writeln!(
            self.pg,
            "{} UTC [{pid}] {level}:  {msg}",
            t.format("%Y-%m-%d %H:%M:%S%.3f")
        );
    }

    fn nginx(&mut self, t: DateTime<Utc>, path: &str, status: u16) {
        let ip = format!("203.0.113.{}", self.rng.u8(2..250));
        let bytes = self.rng.u32(120..9000);
        let agent = AGENTS[self.rng.usize(..AGENTS.len())];
        let _ = writeln!(
            self.nginx,
            r#"{ip} - - [{}] "GET {path} HTTP/1.1" {status} {bytes} "-" "{agent}""#,
            t.format("%d/%b/%Y:%H:%M:%S +0000")
        );
    }
}

fn at(day: NaiveDate, h: u32, m: u32, s: u32) -> DateTime<Utc> {
    day.and_hms_opt(h, m, s).expect("valid time").and_utc()
}

/// Writes `api.log` (JSON), `postgres.log` and `nginx.log` (combined) into `dir`: 08:00–12:00 UTC
/// on 2026-10-28, repeated on the following days when `scale` > 1.
pub fn write_demo(dir: &Path, seed: u64, scale: u32) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let first = FIRST_DAY;
    let mut files: Vec<fs::File> = FILES
        .iter()
        .map(|f| fs::File::create(dir.join(f)).with_context(|| format!("cannot write {f}")))
        .collect::<Result<_>>()?;
    let mut rng = fastrand::Rng::with_seed(seed);
    for d in 0..scale {
        let day = first + TimeDelta::days(i64::from(d));
        let mut out = Day {
            rng: fastrand::Rng::with_seed(rng.u64(..)),
            api: String::new(),
            pg: String::new(),
            nginx: String::new(),
        };
        morning(&mut out, day);
        for (file, text) in files.iter_mut().zip([&out.api, &out.pg, &out.nginx]) {
            std::io::Write::write_all(file, text.as_bytes())?;
        }
    }
    Ok(())
}

fn morning(out: &mut Day, day: NaiveDate) {
    let (start, end) = (at(day, 8, 0, 0), at(day, 12, 0, 0));
    let (outage, recovery) = (at(day, 10, 15, 0), at(day, 10, 22, 10));
    // Isolated warnings, far apart: never 5 warn/error lines within a minute outside the outage.
    let warnings = [at(day, 8, 37, 12), at(day, 9, 48, 40), at(day, 11, 31, 5)];
    out.pg(
        start,
        "LOG",
        "database system is ready to accept connections",
    );
    out.api(start + TimeDelta::seconds(2), "info", "listening on :8080");

    let mut t = start + TimeDelta::seconds(3);
    let mut next_checkpoint = start + TimeDelta::minutes(5);
    let mut warned = 0;
    let mut traced = false;
    let mut recovered = false;
    while t < end {
        let in_outage = t >= outage && t < recovery;
        if in_outage {
            if !traced {
                traced = true;
                out.api(t, "error", "unhandled rejection in GET /api/orders");
                out.api.push_str("Error: connect ECONNREFUSED 10.0.0.5:5432\n    at TCPConnectWrap.afterConnect (node:net:1607:16)\n    at Pool.connect (/app/db.js:41:11)\n");
            }
            // 1–3 retries a second (in time order), a 502 from nginx about once a second.
            let mut offsets: Vec<i64> = (0..out.rng.u32(1..=3))
                .map(|_| out.rng.i64(1..999))
                .collect();
            offsets.sort_unstable();
            for ms in offsets {
                out.api(
                    t + TimeDelta::milliseconds(ms),
                    "error",
                    "db pool exhausted, retrying",
                );
            }
            if out.rng.u8(0..20) == 0 {
                out.pg(t, "FATAL", "sorry, too many clients already");
            }
            out.nginx(t, "/api/orders", 502);
            t += TimeDelta::seconds(1);
            continue;
        }
        if t >= recovery && !recovered {
            recovered = true;
            out.api(t, "info", "db pool healthy again");
        }
        if t >= next_checkpoint {
            out.pg(t, "LOG", "checkpoint starting: time");
            let buffers = out.rng.u32(40..900);
            let done = t + TimeDelta::seconds(out.rng.i64(4..30));
            out.pg(
                done,
                "LOG",
                &format!("checkpoint complete: wrote {buffers} buffers"),
            );
            while next_checkpoint <= t {
                next_checkpoint += TimeDelta::minutes(5);
            }
        }
        if warned < warnings.len() && t >= warnings[warned] {
            if warned == 1 {
                out.pg(
                    t,
                    "WARNING",
                    "checkpoints are occurring too frequently (24 seconds apart)",
                );
            } else {
                let ms = out.rng.u32(1200..2600);
                out.api(
                    t,
                    "warn",
                    &format!("slow request GET /api/orders took {ms} ms"),
                );
            }
            warned += 1;
        }
        let path = PATHS[out.rng.usize(..PATHS.len())];
        let status = match out.rng.u8(0..40) {
            0 => 404,
            1 => 304,
            _ => 200,
        };
        out.nginx(t, path, status);
        if path != "/health" && status == 200 {
            let ms = out.rng.u32(4..180);
            out.api(t, "info", &format!("GET {path} 200 in {ms} ms"));
        }
        t += TimeDelta::milliseconds(out.rng.i64(800..6000));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LineIter, Options, ParseCtx, build, merge, open_inputs};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("logline-demo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn read_all(dir: &Path) -> Vec<Vec<u8>> {
        FILES
            .iter()
            .map(|f| std::fs::read(dir.join(f)).unwrap())
            .collect()
    }

    #[test]
    fn same_seed_same_files() {
        let (a, b, c) = (tmp("a"), tmp("b"), tmp("c"));
        write_demo(&a, DEMO_SEED, 1).unwrap();
        write_demo(&b, DEMO_SEED, 1).unwrap();
        write_demo(&c, 1, 1).unwrap();
        assert_eq!(read_all(&a), read_all(&b));
        assert_ne!(read_all(&a), read_all(&c));
    }

    #[test]
    fn demo_one_incident() {
        let dir = tmp("incident");
        write_demo(&dir, DEMO_SEED, 1).unwrap();
        let paths: Vec<PathBuf> = FILES.iter().map(|f| dir.join(f)).collect();
        let (inputs, _tmp) = open_inputs(&paths).unwrap();
        let ctx = Arc::new(ParseCtx {
            zone: "UTC".parse().unwrap(),
            year: 2026,
        });
        let names: Vec<String> = inputs.iter().map(|i| i.name.clone()).collect();
        let iters = inputs
            .into_iter()
            .enumerate()
            .map(|(i, input)| {
                LineIter::new(
                    i,
                    input.name,
                    input.reader,
                    ctx.clone(),
                    chrono::TimeDelta::zero(),
                    false,
                )
            })
            .collect();
        let t = build(names, merge(iters), &Options::default()).unwrap();
        assert_eq!(t.incidents.len(), 1, "{:?}", t.incidents);
        let inc = &t.incidents[0];
        let start = inc.start.format("%H:%M").to_string();
        assert!(("10:14".."10:17").contains(&start.as_str()), "{start}");
        assert_eq!(inc.sources, [0, 1, 2]);
        assert!(t.entries.iter().filter(|e| e.count > 1).count() >= 2);
    }
}
