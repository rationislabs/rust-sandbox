//! The four checks over a scanned vault, and the report.

use crate::parse::{Fragment, anchors, links};
use crate::resolve::{Index, Resolved, under};
use crate::scan::{Entry, Kind};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub broken: Vec<Finding>,
    pub ambiguous: Vec<Finding>,
    pub orphans: Vec<Finding>,
    pub no_repo: Vec<Finding>,
    pub dangling: Vec<Finding>,
    #[serde(skip)]
    pub notes: usize,
    #[serde(skip)]
    pub links: usize,
    #[serde(skip)]
    pub unverified: usize,
    #[serde(skip)]
    pub unchecked_hosts: Vec<String>,
}

pub struct Options {
    pub vault: PathBuf,
    pub roots: Vec<(String, PathBuf)>,
    pub exclude: Vec<String>,
}

impl Options {
    fn excluded(&self, path: &str) -> bool {
        self.exclude
            .iter()
            .any(|p| path == p || path.starts_with(&format!("{p}/")))
    }
}

fn finding(path: &str, line: Option<usize>, message: String) -> Finding {
    Finding {
        path: path.to_owned(),
        line,
        message,
    }
}

pub fn check(entries: &[Entry], opts: &Options) -> Report {
    let mut report = Report::default();
    let (locked, readable): (Vec<&Entry>, Vec<&Entry>) =
        entries.iter().partition(|e| e.kind == Kind::Unreadable);
    let index = Index::new(readable.iter().map(|e| e.path.as_str()));
    let kinds: HashMap<&str, &Kind> = entries.iter().map(|e| (e.path.as_str(), &e.kind)).collect();
    let mut linked: HashSet<String> = HashSet::new();

    for entry in entries {
        let Kind::Note(Some(text)) = &entry.kind else {
            continue;
        };
        report.notes += 1;
        let skip = opts.excluded(&entry.path);
        for link in links(text) {
            report.links += 1;
            let target = match index.resolve(&entry.path, &link.target, link.markdown) {
                Resolved::None if skip => continue,
                Resolved::None => {
                    if locked
                        .iter()
                        .any(|d| under(&d.path, &entry.path, &link.target))
                    {
                        report.unverified += 1;
                    } else {
                        let msg = format!("{} matches no note", link.raw);
                        report
                            .broken
                            .push(finding(&entry.path, Some(link.line), msg));
                    }
                    continue;
                }
                Resolved::One(p) => p,
                Resolved::Ambiguous { picked, others } => {
                    if !skip {
                        let msg = format!(
                            "{} matches {} files, opens {picked} (also {})",
                            link.raw,
                            1 + others.len(),
                            others.join(", ")
                        );
                        report
                            .ambiguous
                            .push(finding(&entry.path, Some(link.line), msg));
                    }
                    picked
                }
            };
            if target != entry.path {
                linked.insert(target.clone());
            }
            let Some(frag) = &link.fragment else { continue };
            let Some(Kind::Note(Some(body))) = kinds.get(target.as_str()) else {
                if !skip {
                    report.unverified += 1;
                }
                continue;
            };
            let a = anchors(body);
            let msg = match frag {
                Fragment::Heading(h) if !a.has_heading(h) => {
                    format!("{} no heading \"{h}\" in {target}", link.raw)
                }
                Fragment::Block(id) if !a.has_block(id) => {
                    format!("{} no block \"^{id}\" in {target}", link.raw)
                }
                _ => continue,
            };
            if !skip {
                report
                    .broken
                    .push(finding(&entry.path, Some(link.line), msg));
            }
        }
    }

    for entry in entries {
        let Kind::Note(Some(_)) = entry.kind else {
            continue;
        };
        let p = &entry.path;
        if opts.excluded(p) || p.eq_ignore_ascii_case("Home.md") || p.starts_with("inbox/") {
            continue;
        }
        if !linked.contains(p) {
            report
                .orphans
                .push(finding(p, None, "has no incoming links".into()));
        }
    }

    no_repo(entries, opts, &mut report);
    dangling(entries, opts, &mut report);

    for list in [
        &mut report.broken,
        &mut report.ambiguous,
        &mut report.orphans,
        &mut report.no_repo,
        &mut report.dangling,
    ] {
        list.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    }
    report
}

fn no_repo(entries: &[Entry], opts: &Options, report: &mut Report) {
    let mut hosts: BTreeSet<&str> = ["github", "gitlab"].into();
    hosts.extend(opts.roots.iter().map(|(h, _)| h.as_str()));
    for host in hosts {
        let prefix = format!("{host}/");
        let repos: BTreeSet<&str> = entries
            .iter()
            .filter_map(|e| e.path.strip_prefix(&prefix))
            .filter_map(|rest| rest.split_once('/').map(|(repo, _)| repo))
            .collect();
        if repos.is_empty() || opts.excluded(host) {
            continue;
        }
        let Some((_, dir)) = opts.roots.iter().find(|(h, _)| h == host) else {
            report.unchecked_hosts.push(host.to_owned());
            continue;
        };
        for repo in repos {
            let folder = format!("{host}/{repo}");
            if !opts.excluded(&folder) && !dir.join(repo).exists() {
                let msg = format!("has no repo at {}", dir.join(repo).display());
                report.no_repo.push(finding(&folder, None, msg));
            }
        }
    }
}

fn dangling(entries: &[Entry], opts: &Options, report: &mut Report) {
    for entry in entries {
        if entry.kind != Kind::Symlink || opts.excluded(&entry.path) {
            continue;
        }
        let link = opts.vault.join(&entry.path);
        if fs::metadata(&link).is_err() {
            let to = fs::read_link(&link)
                .map(|t| t.display().to_string())
                .unwrap_or_default();
            report
                .dangling
                .push(finding(&entry.path, None, format!("-> {to}")));
        }
    }
}

impl Report {
    pub fn failed(&self) -> bool {
        !(self.broken.is_empty() && self.no_repo.is_empty() && self.dangling.is_empty())
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        for (title, list) in [
            ("broken links", &self.broken),
            ("dangling symlinks", &self.dangling),
            ("folders without a repo", &self.no_repo),
            ("ambiguous links", &self.ambiguous),
            ("orphans", &self.orphans),
        ] {
            if list.is_empty() {
                continue;
            }
            let note = if matches!(title, "ambiguous links" | "orphans") {
                ", not an error"
            } else {
                ""
            };
            let _ = writeln!(out, "{title} ({}{note})", list.len());
            for f in list {
                let _ = match f.line {
                    Some(line) => writeln!(out, "  {}:{line}: {}", f.path, f.message),
                    None => writeln!(out, "  {} {}", f.path, f.message),
                };
            }
        }
        let _ = writeln!(
            out,
            "{} notes, {} links, {} unverified",
            self.notes, self.links, self.unverified
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(path: &str, text: &str) -> Entry {
        Entry {
            path: path.into(),
            kind: Kind::Note(Some(text.into())),
        }
    }

    #[test]
    fn links_into_unreadable_folder_are_unverified() {
        let entries = [
            Entry {
                path: "locked".into(),
                kind: Kind::Unreadable,
            },
            note("Home.md", "[[locked/x]] [[locked]] [[gone]]"),
        ];
        let opts = Options {
            vault: PathBuf::new(),
            roots: vec![],
            exclude: vec![],
        };
        let report = check(&entries, &opts);
        let broken: Vec<_> = report.broken.iter().map(|f| f.message.as_str()).collect();
        assert_eq!(
            broken,
            ["[[locked]] matches no note", "[[gone]] matches no note"]
        );
        assert_eq!(report.unverified, 1);
    }
}
