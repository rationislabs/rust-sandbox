use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("todo-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn todo(file: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_todo"))
        .arg("--file")
        .arg(file)
        .args(args)
        .env("TODO_TODAY", "2026-10-28")
        .env_remove("TODO_FILE")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

#[test]
fn add_list_done_rm() {
    let f = tmp("flow").join("todo.json");
    assert!(
        todo(
            &f,
            &["add", "Buy milk", "--due", "2026-11-02", "--tag", "home"]
        )
        .status
        .success()
    );
    assert!(todo(&f, &["add", "Write spec"]).status.success());

    let list = stdout(&todo(&f, &["list"]));
    let milk = list.find("Buy milk").expect("Buy milk listed");
    let spec = list.find("Write spec").expect("Write spec listed");
    assert!(milk < spec, "dated task first:\n{list}");

    assert!(todo(&f, &["done", "1"]).status.success());
    assert!(!stdout(&todo(&f, &["list"])).contains("Buy milk"));
    let all = stdout(&todo(&f, &["list", "--all"]));
    assert!(
        all.lines()
            .any(|l| l.contains("Buy milk") && l.contains("[x]")),
        "{all}"
    );

    assert!(todo(&f, &["rm", "2"]).status.success());
    assert_eq!(stdout(&todo(&f, &["list"])).trim(), "No tasks.");
}

#[test]
fn unknown_id_exit_1_nothing_written() {
    let f = tmp("unknown").join("todo.json");
    todo(&f, &["add", "a"]);
    let before = fs::read(&f).unwrap();
    let out = todo(&f, &["done", "9"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no task with id 9"));
    assert_eq!(fs::read(&f).unwrap(), before);
}

#[test]
fn corrupt_file_untouched() {
    let f = tmp("corrupt").join("todo.json");
    fs::write(&f, "{not json").unwrap();
    assert_eq!(todo(&f, &["add", "x"]).status.code(), Some(1));
    assert_eq!(fs::read_to_string(&f).unwrap(), "{not json");
}

#[test]
fn ids_not_reused() {
    let f = tmp("ids").join("todo.json");
    todo(&f, &["add", "a"]);
    todo(&f, &["add", "b"]);
    todo(&f, &["rm", "2"]);
    assert_eq!(stdout(&todo(&f, &["add", "c"])).trim(), "added 3");
}

#[test]
fn bad_due_rejected() {
    let f = tmp("baddue").join("todo.json");
    assert!(
        !todo(&f, &["add", "x", "--due", "tomorrow"])
            .status
            .success()
    );
    assert!(!f.exists());
    todo(&f, &["add", "y"]);
    let before = fs::read(&f).unwrap();
    assert!(!todo(&f, &["list", "--due", "2026-13-01"]).status.success());
    assert_eq!(fs::read(&f).unwrap(), before);
}

#[test]
fn unicode_round_trip() {
    let f = tmp("unicode").join("todo.json");
    todo(&f, &["add", "Café ☕", "--tag", "küche"]);
    let list = stdout(&todo(&f, &["list", "--tag", "küche"]));
    assert!(
        list.contains("Café ☕") && list.contains("#küche"),
        "{list}"
    );
}

#[test]
fn repeated_tags() {
    let f = tmp("tags").join("todo.json");
    todo(&f, &["add", "x", "--tag", "a", "--tag", "b"]);
    assert!(stdout(&todo(&f, &["list", "--tag", "b"])).contains(" x"));
}

#[test]
fn creates_missing_folder() {
    let f = tmp("folder").join("new").join("dir").join("todo.json");
    assert!(todo(&f, &["add", "x"]).status.success());
    assert!(f.exists());
}

#[test]
fn exhausted_ids_error_not_panic() {
    let f = tmp("exhausted").join("todo.json");
    fs::write(&f, r#"{"version": 1, "next_id": 4294967295, "tasks": []}"#).unwrap();
    let before = fs::read(&f).unwrap();
    let out = todo(&f, &["add", "z"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(fs::read(&f).unwrap(), before);
}

#[test]
fn closed_pipe_is_not_a_panic() {
    let f = tmp("pipe").join("todo.json");
    todo(&f, &["add", "a"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_todo"))
        .arg("--file")
        .arg(&f)
        .arg("list")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // like `todo list | head -0`
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}
