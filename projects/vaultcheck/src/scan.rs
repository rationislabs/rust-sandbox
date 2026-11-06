//! Walking the vault: every file and symlink, notes read into memory, symlinks never followed.

use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A regular `.md` file; `None` when it is not UTF-8 or cannot be read.
    Note(Option<String>),
    File,
    Symlink,
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Vault-relative, `/`-separated.
    pub path: String,
    pub kind: Kind,
}

/// `Err` only when the vault folder itself cannot be read; anything below it that cannot be read
/// is a warning on stderr and is skipped.
pub fn scan(root: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    walk(root, "", fs::read_dir(root)?, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk(root: &Path, prefix: &str, dir: fs::ReadDir, out: &mut Vec<Entry>) {
    for item in dir {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{prefix}{name}");
        let Ok(ft) = item.file_type() else { continue };
        if ft.is_symlink() {
            out.push(Entry {
                path,
                kind: Kind::Symlink,
            });
        } else if ft.is_dir() {
            match fs::read_dir(root.join(&path)) {
                Ok(sub) => walk(root, &format!("{path}/"), sub, out),
                Err(e) => eprintln!("warning: {path}: {e}, skipped"),
            }
        } else if ft.is_file() {
            let kind = if Path::new(&name)
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("md"))
            {
                Kind::Note(read_note(&root.join(&path), &path))
            } else {
                Kind::File
            };
            out.push(Entry { path, kind });
        }
    }
}

fn read_note(file: &Path, path: &str) -> Option<String> {
    match fs::read(file) {
        Ok(bytes) => String::from_utf8(bytes)
            .inspect_err(|_| eprintln!("warning: {path}: not UTF-8, skipped"))
            .ok(),
        Err(e) => {
            eprintln!("warning: {path}: {e}, skipped");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vaultcheck-scan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn summary(entries: &[Entry]) -> Vec<(String, Kind)> {
        entries
            .iter()
            .map(|e| (e.path.clone(), e.kind.clone()))
            .collect()
    }

    #[test]
    fn kinds_and_order() {
        let root = tmp("kinds");
        put(&root, "b.md", b"# b");
        put(&root, "a/c.MD", b"c");
        put(&root, "pics/p.jpg", b"jpg");
        put(&root, ".obsidian/x.md", b"x");
        put(&root, ".trash/y.md", b"y");
        assert_eq!(
            summary(&scan(&root).unwrap()),
            [
                ("a/c.MD".to_owned(), Kind::Note(Some("c".into()))),
                ("b.md".to_owned(), Kind::Note(Some("# b".into()))),
                ("pics/p.jpg".to_owned(), Kind::File),
            ]
        );
    }

    #[test]
    fn symlinks_not_followed() {
        let base = tmp("symlinks");
        let root = base.join("vault");
        put(&base, "outside/dir/bad.md", b"[[x]]");
        fs::create_dir_all(&root).unwrap();
        symlink(base.join("outside/dir"), root.join("linked")).unwrap();
        symlink(base.join("outside/missing.md"), root.join("n.md")).unwrap();
        let got = summary(&scan(&root).unwrap());
        assert_eq!(
            got,
            [
                ("linked".to_owned(), Kind::Symlink),
                ("n.md".to_owned(), Kind::Symlink)
            ]
        );
    }

    #[test]
    fn non_utf8_note() {
        let root = tmp("latin1");
        put(&root, "latin1.md", b"caf\xe9");
        assert_eq!(
            summary(&scan(&root).unwrap()),
            [("latin1.md".to_owned(), Kind::Note(None))]
        );
    }

    #[test]
    fn unreadable_subfolder() {
        let root = tmp("locked");
        put(&root, "ok.md", b"ok");
        put(&root, "locked/x.md", b"x");
        let locked = root.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&locked).is_ok(); // root ignores the mode
        let got = scan(&root);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            let got = got.unwrap();
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].path, "ok.md");
        }
    }

    #[test]
    fn missing_root() {
        assert!(scan(&tmp("missing").join("nope")).is_err());
    }
}
