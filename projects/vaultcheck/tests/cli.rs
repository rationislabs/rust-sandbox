use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vaultcheck-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for item in fs::read_dir(from).unwrap() {
        let item = item.unwrap();
        let dest = to.join(item.file_name());
        if item.file_type().unwrap().is_dir() {
            copy_dir(&item.path(), &dest);
        } else {
            fs::copy(item.path(), dest).unwrap();
        }
    }
}

fn put(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// The committed fixture copied to `<tmp>/vault`, plus what git cannot keep portably: symlinks,
/// a non-UTF-8 note and the repo folders. Returns `(tmp, vault)`.
fn vault(name: &str) -> (PathBuf, PathBuf) {
    let tmp = tmp(name);
    let vault = tmp.join("vault");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vault"),
        &vault,
    );
    put(&tmp.join("outside/README.md"), b"# Outside\n## Setup\n");
    put(&tmp.join("outside/dir/bad.md"), b"[[nowhere]]\n");
    put(&vault.join("notes/latin1.md"), b"caf\xe9 [[ghost2]]\n");
    fs::create_dir_all(vault.join("sym")).unwrap();
    symlink(tmp.join("outside/README.md"), vault.join("sym/README.md")).unwrap();
    symlink(tmp.join("outside/missing.md"), vault.join("sym/gone.md")).unwrap();
    symlink(tmp.join("outside/dir"), vault.join("linked")).unwrap();
    symlink(
        tmp.join("outside/nothing"),
        vault.join("gitlab/kept/dead.md"),
    )
    .unwrap();
    fs::create_dir_all(tmp.join("repos/github/present")).unwrap();
    fs::create_dir_all(tmp.join("repos/gitlab")).unwrap();
    (tmp, vault)
}

fn command(envs: &[(&str, &Path)], home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_vaultcheck"));
    c.env_remove("OBSIDIAN_VAULT").env("HOME", home);
    for (k, v) in envs {
        c.env(k, v);
    }
    c
}

fn run(vault: &Path, tmp: &Path, args: &[&str]) -> Output {
    command(&[], &tmp.join("home"))
        .arg(vault)
        .arg("--root")
        .arg(format!("github={}", tmp.join("repos/github").display()))
        .arg("--root")
        .arg(format!("gitlab={}", tmp.join("repos/gitlab").display()))
        .args(args)
        .output()
        .unwrap()
}

fn run_raw(args: &[&str], envs: &[(&str, &Path)]) -> Output {
    command(envs, Path::new("/nonexistent-home"))
        .args(args)
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
fn full_run() {
    let (tmp, vault) = vault("full");
    let out = run(&vault, &tmp, &[]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    let t = tmp.display();
    let lines = text.lines().collect::<Vec<_>>();
    for want in [
        "broken links (5)".to_owned(),
        "  cases.md:6: [[beta#Missing]] no heading \"Missing\" in beta.md".to_owned(),
        "  cases.md:14: [[ghost]] matches no note".to_owned(),
        "  cases.md:16: [nope](notes/nope.md) matches no note".to_owned(),
        "  cases.md:27: [[beta#^nope]] no block \"^nope\" in beta.md".to_owned(),
        "  gitlab/kept/plan.md:1: [[ghost-in-gitlab]] matches no note".to_owned(),
        "dangling symlinks (2)".to_owned(),
        format!("  gitlab/kept/dead.md -> {t}/outside/nothing"),
        format!("  sym/gone.md -> {t}/outside/missing.md"),
        "folders without a repo (2)".to_owned(),
        format!("  github/missing has no repo at {t}/repos/github/missing"),
        format!("  gitlab/kept has no repo at {t}/repos/gitlab/kept"),
        "ambiguous links (2, not an error)".to_owned(),
        "  b/sub/x.md:1: [[dup]] matches 2 files, opens b/sub/dup.md (also a/dup.md)".to_owned(),
        "  cases.md:12: [[dup]] matches 2 files, opens a/dup.md (also b/sub/dup.md)".to_owned(),
        "orphans (1, not an error)".to_owned(),
        "  notes/lonely.md has no incoming links".to_owned(),
    ] {
        assert!(
            lines.contains(&want.as_str()),
            "missing {want:?} in\n{text}"
        );
    }
    let last = lines.last().unwrap();
    assert!(
        last.starts_with("14 notes, ") && last.ends_with(", 1 unverified"),
        "{last}"
    );
    for not in [
        "inline-ghost",
        "fenced-ghost",
        "obsidian-ghost",
        "nowhere",
        "ghost2",
        ".trash",
        "sym/README",
        "linked",
        "inbox/todo.md",
        "Home.md",
        "cases.md:31",
        "cases.md:28",
    ] {
        assert!(!text.contains(not), "{not:?} in\n{text}");
    }
}

#[test]
fn group_order() {
    let (tmp, vault) = vault("order");
    let text = stdout(&run(&vault, &tmp, &[]));
    let at = |h: &str| text.find(h).unwrap_or_else(|| panic!("no {h}"));
    let order = [
        at("broken links"),
        at("dangling symlinks"),
        at("folders without a repo"),
        at("ambiguous links"),
        at("orphans"),
    ];
    assert!(order.is_sorted(), "{text}");
}

#[test]
fn exclude_is_a_filter() {
    let (tmp, vault) = vault("exclude");
    let out = run(&vault, &tmp, &["--exclude", "gitlab/"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("broken links (4)"), "{text}");
    assert!(!text.lines().any(|l| l.contains("gitlab/")), "{text}");
    assert!(!text.contains("notes/only-from-gitlab.md"), "{text}");
    assert!(text.contains("dangling symlinks (1)"), "{text}");
    assert!(text.contains("folders without a repo (1)"), "{text}");
}

#[test]
fn unchecked_host_noted_once() {
    let (tmp, vault) = vault("unchecked");
    let v = vault.to_str().unwrap();
    let root = format!("github={}", tmp.join("repos/github").display());
    let out = run_raw(&[v, "--root", &root], &[]);
    let note = "note: no --root for gitlab, its folders are not checked";
    assert_eq!(stderr(&out).matches(note).count(), 1, "{}", stderr(&out));
    assert!(!stdout(&out).contains("gitlab/kept has no repo"));
    let out = run_raw(&[v, "--root", &root, "--exclude", "gitlab"], &[]);
    assert!(!stderr(&out).contains("note:"), "{}", stderr(&out));
}

#[test]
fn non_utf8_warns_once() {
    let (tmp, vault) = vault("latin1");
    let out = run(&vault, &tmp, &[]);
    let warn = "warning: notes/latin1.md: not UTF-8, skipped";
    assert_eq!(stderr(&out).matches(warn).count(), 1, "{}", stderr(&out));
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn clean_vault_exit_0() {
    let tmp = tmp("clean");
    let vault = tmp.join("vault");
    put(&vault.join("Home.md"), b"[[a]] [[dup]]\n");
    put(&vault.join("a.md"), b"# a\n");
    put(&vault.join("x/dup.md"), b"# dup\n");
    put(&vault.join("y/dup.md"), b"# dup\n");
    put(&vault.join("latin1.md"), b"\xff");
    let out = run(&vault, &tmp, &[]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("ambiguous links (1, not an error)"), "{text}");
    assert!(text.contains("orphans (1, not an error)"), "{text}");
    assert!(text.contains("  y/dup.md has no incoming links"), "{text}");
    assert!(!text.contains("broken links"), "{text}");
}

#[test]
fn json_shape() {
    let (tmp, vault) = vault("json");
    let out = run(&vault, &tmp, &["--json"]);
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["ambiguous", "broken", "dangling", "no_repo", "orphans"]
    );
    let broken = v["broken"].as_array().unwrap();
    assert_eq!(broken.len(), 5);
    assert!(broken.contains(&serde_json::json!(
        {"path": "cases.md", "line": 14, "message": "[[ghost]] matches no note"}
    )));
    for key in ["dangling", "no_repo"] {
        assert!(
            v[key]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f.get("line").is_none())
        );
    }
}

#[test]
fn defaults_from_env() {
    let (tmp, vault) = vault("defaults");
    let home = tmp.join("home");
    fs::create_dir_all(home.join("Development/pro/github/present")).unwrap();
    fs::create_dir_all(home.join("Development/pro/gitlab/rationis_labs")).unwrap();
    let out = run_raw(&[], &[("OBSIDIAN_VAULT", &vault), ("HOME", &home)]);
    let text = stdout(&out);
    let h = home.display();
    assert!(
        text.contains(&format!(
            "github/missing has no repo at {h}/Development/pro/github/missing"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "gitlab/kept has no repo at {h}/Development/pro/gitlab/rationis_labs/kept"
        )),
        "{text}"
    );
}

#[test]
fn tilde_in_root() {
    let (tmp, vault) = vault("tilde");
    let home = tmp.join("home");
    fs::create_dir_all(home.join("Development/pro/github/present")).unwrap();
    let v = vault.to_str().unwrap();
    let out = run_raw(
        &[
            v,
            "--root",
            "github=~/Development/pro/github",
            "--exclude",
            "gitlab",
        ],
        &[("HOME", &home)],
    );
    let want = format!(
        "github/missing has no repo at {}/Development/pro/github/missing",
        home.display()
    );
    assert!(stdout(&out).contains(&want), "{}", stdout(&out));
}

#[test]
fn usage_errors_exit_2() {
    let (tmp, vault) = vault("usage");
    let out = run_raw(&[vault.to_str().unwrap(), "--root", "github"], &[]);
    assert_eq!(out.status.code(), Some(2));
    let nope = tmp.join("nope");
    let out = run_raw(&[nope.to_str().unwrap(), "--root", "github=/x"], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("error: "), "{}", stderr(&out));
}

/// Kind, mtime, length, bytes or link text of every path under `dir`, never following links.
fn snapshot(dir: &Path, out: &mut Vec<String>) {
    let mut items: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|i| i.unwrap().path())
        .collect();
    items.sort();
    for p in items {
        let m = fs::symlink_metadata(&p).unwrap();
        let what = if m.is_symlink() {
            format!("link {}", fs::read_link(&p).unwrap().display())
        } else if m.is_dir() {
            "dir".to_owned()
        } else {
            format!("file {:?}", fs::read(&p).unwrap())
        };
        out.push(format!(
            "{} {:?} {} {what}",
            p.display(),
            m.modified().unwrap(),
            m.len()
        ));
        if m.is_dir() {
            snapshot(&p, out);
        }
    }
}

#[test]
fn read_only() {
    let (tmp, vault) = vault("readonly");
    let mut before = Vec::new();
    snapshot(&tmp, &mut before);
    run(&vault, &tmp, &[]);
    run(&vault, &tmp, &["--json"]);
    let mut after = Vec::new();
    snapshot(&tmp, &mut after);
    assert_eq!(before, after);
}
