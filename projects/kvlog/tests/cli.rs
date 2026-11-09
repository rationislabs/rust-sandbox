use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kvlog-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn kv(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kvlog"))
        .arg("--dir")
        .arg(dir)
        .args(args)
        .env_remove("KVLOG_DIR")
        .env_remove("KVLOG_CRASH_AT")
        .output()
        .unwrap()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = kv(dir, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn wal(dir: &Path) -> Vec<u8> {
    fs::read(dir.join("wal")).unwrap_or_default()
}

#[test]
fn set_get_rm_list() {
    let d = tmp("basic");
    ok(&d, &["set", "city", "Vienna"]);
    ok(&d, &["set", "country", "Austria"]);
    ok(&d, &["set", "zip", "1010"]);
    assert_eq!(ok(&d, &["get", "city"]), "Vienna\n");
    assert_eq!(
        ok(&d, &["list"]),
        "city\tVienna\ncountry\tAustria\nzip\t1010\n"
    );
    assert_eq!(
        ok(&d, &["list", "--prefix", "c"]),
        "city\tVienna\ncountry\tAustria\n"
    );
    ok(&d, &["rm", "city"]);
    let out = kv(&d, &["get", "city"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no such key: city"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn unknown_key_exit_1_nothing_written() {
    let d = tmp("unknown");
    ok(&d, &["set", "a", "1"]);
    let before = wal(&d);
    assert_eq!(kv(&d, &["rm", "nope"]).status.code(), Some(1));
    assert_eq!(kv(&d, &["get", "nope"]).status.code(), Some(1));
    assert_eq!(wal(&d), before);
}

#[test]
fn second_process_refused() {
    let d = tmp("locked");
    let _held = kvlog::Store::open(&d).unwrap();
    let out = kv(&d, &["get", "a"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("the store is in use"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn compact_keeps_every_key() {
    let d = tmp("compact");
    for i in 0..50 {
        ok(&d, &["set", &format!("k{i}"), &format!("v{i}")]);
    }
    ok(&d, &["rm", "k7"]);
    let before = ok(&d, &["list"]);
    ok(&d, &["compact"]);
    assert_eq!(ok(&d, &["list"]), before);
    assert_eq!(wal(&d).len(), 0);
    assert!(d.join("snapshot").exists());
}

#[test]
fn corrupt_wal_exit_1_names_offset() {
    let d = tmp("corrupt");
    for (k, v) in [("a", "1"), ("b", "2"), ("c", "3")] {
        ok(&d, &["set", k, v]);
    }
    let mut bytes = wal(&d);
    bytes[37] ^= 0xFF;
    fs::write(d.join("wal"), &bytes).unwrap();
    let out = kv(&d, &["list"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("wal") && err.contains("byte 19"), "{err}");
    assert_eq!(wal(&d), bytes);
}

#[test]
fn torn_tail_notice() {
    let d = tmp("torn");
    ok(&d, &["set", "a", "1"]);
    ok(&d, &["set", "b", "2"]);
    let mut bytes = wal(&d);
    bytes.extend(&bytes[..10].to_vec());
    fs::write(d.join("wal"), &bytes).unwrap();
    let out = kv(&d, &["list"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout.clone()).unwrap(),
        "a\t1\nb\t2\n"
    );
    assert!(
        stderr(&out).contains("dropped 10 bytes"),
        "{}",
        stderr(&out)
    );
    assert_eq!(wal(&d).len(), 38);
}

#[test]
fn unicode_round_trip() {
    let d = tmp("unicode");
    ok(&d, &["set", "Straße", "Wien ☕"]);
    assert_eq!(ok(&d, &["get", "Straße"]), "Wien ☕\n");
}

#[test]
fn stress_prints_acks() {
    let d = tmp("stress");
    assert_eq!(
        ok(&d, &["stress", "--start", "5", "--count", "3"]),
        "ack 5\nack 6\nack 7\n"
    );
    let s = kvlog::Store::open(&d).unwrap();
    let got: Vec<_> = s.iter().map(|(k, v)| (k.to_vec(), v.to_vec())).collect();
    let want: Vec<_> = (5..=7)
        .map(|i| (format!("key{i}").into_bytes(), kvlog::stress_value(i)))
        .collect();
    assert_eq!(got, want);
    let d = tmp("stress-compact");
    ok(
        &d,
        &[
            "stress",
            "--start",
            "0",
            "--count",
            "4",
            "--compact-every",
            "2",
        ],
    );
    assert!(d.join("snapshot").exists());
}

#[test]
fn failed_write_leaves_log_clean() {
    let d = tmp("efbig");
    ok(&d, &["set", "a", "1"]);
    assert_eq!(wal(&d).len(), 19);
    let big = "x".repeat(8192);
    let out = Command::new("sh")
        .arg("-c")
        .arg(r#"trap "" XFSZ; ulimit -f 4; exec "$0" --dir "$1" set big "$2""#)
        .arg(env!("CARGO_BIN_EXE_kvlog"))
        .arg(&d)
        .arg(&big)
        .env_remove("KVLOG_DIR")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(wal(&d).len(), 19);
    assert_eq!(ok(&d, &["get", "a"]), "1\n");
}

#[test]
fn list_escapes_tabs_and_newlines() {
    let d = tmp("escape");
    ok(&d, &["set", "k", "a\tb\nc\\d"]);
    assert_eq!(ok(&d, &["list"]), "k\ta\\tb\\nc\\\\d\n");
}

#[test]
fn oversized_key_rejected() {
    let d = tmp("bigkey");
    let out = kv(&d, &["set", &"k".repeat(65_537), "v"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("65536"), "{}", stderr(&out));
    assert_eq!(wal(&d), Vec::<u8>::new());
}

#[test]
fn creates_missing_dir() {
    let d = tmp("missing").join("new/deeper");
    ok(&d, &["set", "a", "1"]);
    assert!(d.join("wal").exists() && d.join("LOCK").exists());
}

#[test]
fn usage_error_exits_1() {
    assert_eq!(kv(&tmp("usage"), &["frobnicate"]).status.code(), Some(1));
}

#[test]
fn stress_compact_every_zero_is_a_usage_error() {
    let d = tmp("every0");
    let out = kv(
        &d,
        &[
            "stress",
            "--start",
            "0",
            "--count",
            "1",
            "--compact-every",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("invalid value '0' for '--compact-every"),
        "{}",
        stderr(&out)
    );
}
