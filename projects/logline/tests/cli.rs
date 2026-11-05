use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("logline-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn logline(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logline"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

const FIXTURES: [&str; 2] = ["tests/fixtures/app.log", "tests/fixtures/db.log"];

#[test]
fn text_shows_continuation_lines() {
    let dir = tmp("trace");
    let log = dir.join("app.log");
    fs::write(
        &log,
        "2026-10-28 10:15:00,001 [ERROR] boom\n  at foo\n  at bar\n2026-10-28 10:15:01,000 [INFO] ok\n",
    )
    .unwrap();
    let out = logline(&[log.to_str().unwrap(), "--format", "text"]);
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[0].ends_with("boom"), "{stdout}");
    assert_eq!(lines[1..3], ["      at foo", "      at bar"], "{stdout}");
    assert!(lines[3].ends_with("ok"), "{stdout}");
}

#[test]
fn gzip_input_is_read() {
    let dir = tmp("gzip");
    let log = dir.join("app.log.gz");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(b"2026-10-28 10:15:00,001 [ERROR] boom\n")
        .unwrap();
    fs::write(&log, gz.finish().unwrap()).unwrap();
    let out = logline(&[log.to_str().unwrap(), "--format", "text"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).starts_with("2026-10-28 10:15:00.001 app.log ERROR boom"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn text_fixture() {
    let out = logline(&[FIXTURES[0], FIXTURES[1], "--format", "text"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let times: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("2026"))
        .map(|l| &l[..23])
        .collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{stdout}");
    assert!(
        stdout.lines().any(|l| l.contains("db.log")
            && !l.contains("FATAL")
            && l.contains("too many clients"))
    );
    assert!(stdout.contains("(×3"), "{stdout}");
    assert!(
        stdout
            .trim_end()
            .lines()
            .last()
            .unwrap()
            .contains("entries from"),
        "{stdout}"
    );
}

#[test]
fn tz_display() {
    let out = logline(&[
        FIXTURES[0],
        FIXTURES[1],
        "--format",
        "text",
        "--tz",
        "Europe/Vienna",
    ]);
    assert!(
        text(&out.stdout).starts_with("2026-01-15 10:00:00"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn empty_inputs() {
    let dir = tmp("empty");
    let (a, b) = (dir.join("a.log"), dir.join("b.log"));
    fs::write(&a, "").unwrap();
    fs::write(&b, "").unwrap();
    let out = logline(&[a.to_str().unwrap(), b.to_str().unwrap(), "--format", "text"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).trim_end().ends_with("no entries"));
}

#[test]
fn missing_file() {
    let dir = tmp("missing");
    let html = dir.join("x.html");
    let out = logline(&["nope.log", "-o", html.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).starts_with("error: nope.log"),
        "{}",
        text(&out.stderr)
    );
    assert!(!html.exists());
}

#[test]
fn no_files_is_an_error() {
    let out = logline(&[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("no input files"));
}

#[test]
fn stdin_docker() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_logline"))
        .args(["-", "--format", "text"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"api-1  | 2026-10-28T10:00:01Z {\"level\":\"error\",\"msg\":\"x\"}\ndb-1   | 2026-10-28T10:00:00Z LOG: ok\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("api-1") && stdout.contains("db-1"),
        "{stdout}"
    );
}

#[test]
fn shift_and_window() {
    let out = logline(&[
        FIXTURES[0],
        FIXTURES[1],
        "--format",
        "text",
        "--shift",
        "db=+1h",
        "--from",
        "2026-01-15 09:30:00",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("too many clients") && !stdout.contains("server started"),
        "{stdout}"
    );
}

#[test]
fn demo_command() {
    let dir = tmp("demo").join("d");
    let out = logline(&["demo", "--out", dir.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let mut files: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path().to_str().unwrap().to_string())
        .collect();
    files.sort();
    assert_eq!(files.len(), 3);
    let mut args: Vec<&str> = files.iter().map(String::as_str).collect();
    args.extend(["--format", "text"]);
    let out = logline(&args);
    assert!(
        text(&out.stdout).contains("--- incident 1:"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn html_file_written() {
    let page = tmp("html").join("t.html");
    let out = logline(&[FIXTURES[0], FIXTURES[1], "-o", page.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let html = fs::read_to_string(&page).unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("upstream timeout"));
}
