//! Link resolution the way Obsidian does it.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    None,
    One(String),
    Ambiguous { picked: String, others: Vec<String> },
}

/// Every path with its lowercase keys (with and without `.md`).
pub struct Index {
    paths: Vec<(String, Vec<String>)>,
}

/// `dir` + `rel` with `.` and `..` applied; `None` when `..` climbs above the vault root.
fn join(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

fn folder(path: &str) -> String {
    path.rsplit_once('/').map_or("", |(d, _)| d).to_lowercase()
}

impl Index {
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        let paths = paths
            .into_iter()
            .map(|p| {
                let lower = p.to_lowercase();
                let mut keys = vec![lower.clone()];
                if let Some(stem) = lower.strip_suffix(".md") {
                    keys.push(stem.to_owned());
                }
                (p.to_owned(), keys)
            })
            .collect();
        Index { paths }
    }

    fn with_key(&self, key: &str) -> Option<&str> {
        self.paths
            .iter()
            .find(|(_, keys)| keys.iter().any(|k| k == key))
            .map(|(p, _)| p.as_str())
    }

    // ponytail: linear scan over all paths per link; fine for a few thousand notes, index by last
    // segment if it ever is not.
    pub fn resolve(&self, from: &str, target: &str, markdown: bool) -> Resolved {
        if target.is_empty() {
            return Resolved::One(from.to_owned());
        }
        let t = target.to_lowercase();
        let dir = folder(from);
        let here = join(&dir, &t);
        let found = |key: Option<&str>| match key.and_then(|k| self.with_key(k)) {
            Some(p) => Resolved::One(p.to_owned()),
            None => Resolved::None,
        };
        if t.starts_with("./") || t.starts_with("../") {
            return found(here.as_deref());
        }
        if let Some(abs) = t.strip_prefix('/') {
            return found(Some(abs));
        }
        if markdown && let Some(p) = here.as_deref().and_then(|k| self.with_key(k)) {
            return Resolved::One(p.to_owned());
        }
        let suffix = format!("/{t}");
        let mut cands: Vec<(&str, &Vec<String>)> = self
            .paths
            .iter()
            .filter(|(_, keys)| {
                keys.iter().any(|k| {
                    if t.contains('/') {
                        *k == t || k.ends_with(&suffix)
                    } else {
                        k.rsplit('/').next() == Some(t.as_str())
                    }
                })
            })
            .map(|(p, keys)| (p.as_str(), keys))
            .collect();
        cands.sort_by_key(|(p, keys)| {
            let near = here.as_ref().is_some_and(|h| keys.contains(h));
            (!near, p.matches('/').count(), p.len(), p.to_lowercase())
        });
        let mut names = cands.into_iter().map(|(p, _)| p.to_owned());
        match (names.next(), names.collect::<Vec<_>>()) {
            (None, _) => Resolved::None,
            (Some(p), others) if others.is_empty() => Resolved::One(p),
            (Some(picked), others) => Resolved::Ambiguous { picked, others },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATHS: [&str; 13] = [
        "Home.md",
        "beta.md",
        "notes/Alpha.md",
        "notes/deep/gamma.md",
        "a/dup.md",
        "b/sub/dup.md",
        "b/sub/x.md",
        "pics/photo.jpg",
        "specs/x.md",
        "g/specs/x.md",
        "claude-memory/MEMORY.md",
        "claude-memory/pdf-ocr.md",
        "my note.md",
    ];

    fn r(from: &str, target: &str, markdown: bool) -> Resolved {
        Index::new(PATHS).resolve(from, target, markdown)
    }

    fn one(p: &str) -> Resolved {
        Resolved::One(p.into())
    }

    #[test]
    fn exact_and_case() {
        assert_eq!(r("Home.md", "notes/alpha", false), one("notes/Alpha.md"));
        assert_eq!(r("Home.md", "NOTES/ALPHA.md", false), one("notes/Alpha.md"));
        assert_eq!(r("Home.md", "ALPHA", false), one("notes/Alpha.md"));
    }

    #[test]
    fn partial_path() {
        assert_eq!(
            r("Home.md", "deep/gamma", false),
            one("notes/deep/gamma.md")
        );
        assert_eq!(r("Home.md", "eep/gamma", false), Resolved::None);
    }

    #[test]
    fn attachment() {
        assert_eq!(r("Home.md", "pics/photo.jpg", false), one("pics/photo.jpg"));
        assert_eq!(r("Home.md", "photo.jpg", false), one("pics/photo.jpg"));
        assert_eq!(r("Home.md", "photo", false), Resolved::None);
    }

    #[test]
    fn relative() {
        let from = "notes/deep/gamma.md";
        assert_eq!(r(from, "../alpha", false), one("notes/Alpha.md"));
        assert_eq!(r(from, "../../../x", false), Resolved::None);
        assert_eq!(r(from, "./gamma", false), one("notes/deep/gamma.md"));
    }

    #[test]
    fn ambiguous_shortest() {
        assert_eq!(
            r("Home.md", "dup", false),
            Resolved::Ambiguous {
                picked: "a/dup.md".into(),
                others: vec!["b/sub/dup.md".into()]
            }
        );
    }

    #[test]
    fn ambiguous_same_folder() {
        assert_eq!(
            r("b/sub/x.md", "dup", false),
            Resolved::Ambiguous {
                picked: "b/sub/dup.md".into(),
                others: vec!["a/dup.md".into()]
            }
        );
    }

    #[test]
    fn ambiguous_path() {
        assert_eq!(
            r("Home.md", "specs/x", false),
            Resolved::Ambiguous {
                picked: "specs/x.md".into(),
                others: vec!["g/specs/x.md".into()]
            }
        );
    }

    #[test]
    fn markdown_relative_first() {
        assert_eq!(
            r("claude-memory/MEMORY.md", "pdf-ocr.md", true),
            one("claude-memory/pdf-ocr.md")
        );
        assert_eq!(r("Home.md", "my note.md", true), one("my note.md"));
        assert_eq!(r("Home.md", "notes/nope.md", true), Resolved::None);
    }

    #[test]
    fn self_link() {
        assert_eq!(r("beta.md", "", false), one("beta.md"));
    }
}
