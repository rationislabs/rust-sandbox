use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("finance-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn finance(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_finance"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn report_fixture() {
    let out = finance(&[
        "--csv",
        "tests/fixtures/small.csv",
        "report",
        "--month",
        "2026-09",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("Spending: 79.74") && stdout.contains("Net: 2420.26"),
        "{stdout}"
    );
}

#[test]
fn report_demo_default() {
    let out = finance(&["report", "--month", "2026-09"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with("2026-09"));
}

#[test]
fn report_month_outside_data() {
    let out = finance(&["report", "--month", "2026-03"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("2026-03") && stderr.contains("2026-05 to 2026-10"),
        "{stderr}"
    );
}

#[test]
fn header_only_csv() {
    let f = tmp("header").join("t.csv");
    fs::write(&f, "date,payee,amount\n").unwrap();
    let out = finance(&["--csv", f.to_str().unwrap(), "report", "--month", "2026-09"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("no transactions"));
}

#[test]
fn bad_csv_line() {
    let f = tmp("bad").join("t.csv");
    fs::write(
        &f,
        "date,payee,amount\n2026-09-01,A,-1.00\n2026-09-02,B,lots\n",
    )
    .unwrap();
    let out = finance(&["--csv", f.to_str().unwrap(), "report", "--month", "2026-09"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("line 3:"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn demo_writes_files() {
    let dir = tmp("demo").join("d");
    let out = finance(&["demo", "--out", dir.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let csv = dir.join("transactions.csv");
    let rules = dir.join("rules.toml");
    assert!(csv.exists() && rules.exists());
    let from_files = finance(&[
        "--csv",
        csv.to_str().unwrap(),
        "--rules",
        rules.to_str().unwrap(),
        "report",
        "--month",
        "2026-10",
    ]);
    assert!(from_files.status.success(), "{}", text(&from_files.stderr));
    assert_eq!(
        from_files.stdout,
        finance(&["report", "--month", "2026-10"]).stdout
    );
}
