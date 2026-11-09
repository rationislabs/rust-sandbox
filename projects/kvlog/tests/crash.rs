//! Kills `kvlog` at named crash points and at random moments, then checks that every
//! acknowledged write is still there.

use kvlog::{Store, stress_value};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kvlog-crash-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn kvlog(dir: &Path, args: &[&str], crash_at: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kvlog"));
    c.arg("--dir")
        .arg(dir)
        .args(args)
        .env_remove("KVLOG_DIR")
        .env_remove("KVLOG_CRASH_AT");
    if let Some(p) = crash_at {
        c.env("KVLOG_CRASH_AT", p);
    }
    c.output().unwrap()
}

fn stress(dir: &Path, start: u64, count: u64, crash_at: Option<&str>) -> Output {
    let (s, n) = (start.to_string(), count.to_string());
    kvlog(dir, &["stress", "--start", &s, "--count", &n], crash_at)
}

/// SIGABRT: a build without crash points exits 0 and fails here, so these tests cannot pass by
/// luck.
fn assert_aborted(out: &Output) {
    assert_eq!(
        out.status.signal(),
        Some(6),
        "{:?} {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every `key{i}` in `keys` holds `stress_value(i)`, and nothing else is stored.
fn assert_keys(dir: &Path, keys: impl IntoIterator<Item = u64>) {
    let store = Store::open(dir).unwrap();
    let mut want: Vec<_> = keys
        .into_iter()
        .map(|i| (format!("key{i}").into_bytes(), stress_value(i)))
        .collect();
    want.sort();
    let got: Vec<_> = store
        .iter()
        .map(|(k, v)| (k.to_vec(), v.to_vec()))
        .collect();
    assert_eq!(got, want);
}

fn wal_len(dir: &Path) -> u64 {
    fs::metadata(dir.join("wal")).unwrap().len()
}

#[test]
fn crash_half_record() {
    let d = tmp("half");
    assert!(stress(&d, 0, 10, None).status.success());
    let w = wal_len(&d);
    let out = stress(&d, 10, 10, Some("half-record"));
    assert_aborted(&out);
    assert_eq!(out.stdout, b"");
    let list = kvlog(&d, &["list"], None);
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stderr).contains("dropped"));
    assert_eq!(wal_len(&d), w);
    assert_keys(&d, 0..10);
}

#[test]
fn crash_after_write() {
    let d = tmp("after-write");
    assert!(stress(&d, 0, 10, None).status.success());
    let out = stress(&d, 10, 10, Some("after-write"));
    assert_aborted(&out);
    assert_eq!(out.stdout, b"");
    assert_keys(&d, 0..=10); // the full record replays
}

fn thirty_minus_key5(d: &Path) {
    assert!(stress(d, 0, 30, None).status.success());
    assert!(kvlog(d, &["rm", "key5"], None).status.success());
}

#[test]
fn crash_after_snapshot_sync() {
    let d = tmp("after-snapshot-sync");
    thirty_minus_key5(&d);
    assert_aborted(&kvlog(&d, &["compact"], Some("after-snapshot-sync")));
    assert!(d.join("snapshot.tmp").exists() && !d.join("snapshot").exists());
    assert_keys(&d, (0..30).filter(|&i| i != 5));
    assert!(kvlog(&d, &["compact"], None).status.success());
    assert_keys(&d, (0..30).filter(|&i| i != 5));
    assert_eq!(wal_len(&d), 0);
}

#[test]
fn crash_after_rename() {
    let d = tmp("after-rename");
    thirty_minus_key5(&d);
    assert_aborted(&kvlog(&d, &["compact"], Some("after-rename")));
    assert!(d.join("snapshot").exists() && !d.join("snapshot.tmp").exists());
    assert!(wal_len(&d) > 0);
    assert_keys(&d, (0..30).filter(|&i| i != 5));
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

/// One round: start stress at `next`, SIGKILL it after `kill_after` acks, return the highest ack.
fn kill_round(dir: &Path, next: u64, kill_after: u64, compact: bool, ctx: &str) -> u64 {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kvlog"));
    cmd.arg("--dir").arg(dir);
    cmd.args(["stress", "--start", &next.to_string(), "--count", "100000"]);
    if compact {
        cmd.args(["--compact-every", "25"]);
    }
    let mut child = cmd
        .env_remove("KVLOG_DIR")
        .env_remove("KVLOG_CRASH_AT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut acked = 0;
    let mut line = String::new();
    while acked < kill_after {
        line.clear();
        if stdout.read_line(&mut line).unwrap() == 0 {
            let mut err = String::new();
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut err)
                .unwrap();
            panic!("{ctx}: stress ended after {acked} acks: {err}");
        }
        assert_eq!(line, format!("ack {}\n", next + acked), "{ctx}");
        acked += 1;
    }
    child.kill().unwrap();
    child.wait().unwrap();
    // acks already in the pipe were sent before the kill landed: they count
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    for l in rest.split_inclusive('\n').filter(|l| l.ends_with('\n')) {
        assert_eq!(l, format!("ack {}\n", next + acked), "{ctx}");
        acked += 1;
    }
    next + acked - 1
}

#[test]
fn acked_writes_survive_sigkill() {
    let seed = env_u64("KVLOG_CRASH_SEED").unwrap_or_else(|| fastrand::u64(..));
    let rounds = env_u64("KVLOG_CRASH_ROUNDS").unwrap_or(200);
    println!("seed {seed}, {rounds} rounds (replay: KVLOG_CRASH_SEED={seed})");
    let mut rng = fastrand::Rng::with_seed(seed);
    let dir = tmp("loop");
    let mut next = 0;
    let mut expected: Vec<Vec<u8>> = Vec::new(); // stress_value(i), built once
    for round in 0..rounds {
        let compact = round % 2 == 1;
        // 25 = the kill lands as compaction starts
        let kill_after = if compact {
            rng.u64(24..=26)
        } else {
            rng.u64(1..=20)
        };
        let ctx = format!("round {round}, seed {seed}");
        let last = kill_round(&dir, next, kill_after, compact, &ctx);
        let store = Store::open(&dir).unwrap_or_else(|e| panic!("{ctx}: open failed: {e}"));
        while expected.len() as u64 <= last + 1 {
            expected.push(stress_value(expected.len() as u64));
        }
        for i in 0..=last {
            let v = store.get(format!("key{i}").as_bytes());
            assert!(
                v == Some(&expected[usize::try_from(i).unwrap()][..]),
                "{ctx}: key{i}"
            );
        }
        let extra = store.get(format!("key{}", last + 1).as_bytes());
        assert!(
            extra.is_none_or(|v| v == expected[usize::try_from(last + 1).unwrap()]),
            "{ctx}: key{}",
            last + 1
        );
        let count = store.iter().count() as u64;
        assert!(
            count == last + 1 || (count == last + 2 && extra.is_some()),
            "{ctx}: {count} keys, last ack {last}"
        );
        drop(store);
        next = last + 1;
    }
    fs::remove_dir_all(&dir).unwrap();
}
