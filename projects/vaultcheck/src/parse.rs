//! One note's text: its links, headings and block ids.

use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fragment {
    Heading(String),
    Block(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub line: usize,
    pub raw: String,
    pub target: String,
    pub fragment: Option<Fragment>,
    pub markdown: bool,
}

static WIKI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(!?)\[\[([^\[\]\n]+?)\]\]").unwrap());
static MARKDOWN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!?\[[^\]\n]*\]\(\s*(<[^>\n]*>|(?:[^()\s]|\([^()\s]*\))+)(?:\s+"[^"\n]*")?\s*\)"#)
        .unwrap()
});
static SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:").unwrap());
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}#{1,6}(?:\s+(.*?))?(?:\s+#+)?\s*$").unwrap());
static SETEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}(?:=+|-+)[ \t]*$").unwrap());
static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[ \t]*(?:>[ \t]?)+").unwrap());
static BLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)\^([A-Za-z0-9-]+)\s*$").unwrap());

/// Lines with blockquote markers (callouts) removed and fenced code blanked (line count kept).
fn unfenced(text: &str) -> Vec<&str> {
    let mut open: Option<(char, usize)> = None;
    text.lines()
        .map(|line| {
            let line = QUOTE.find(line).map_or(line, |m| &line[m.end()..]);
            let trimmed = line.trim_start();
            if let Some((ch, n)) = open {
                let rest = trimmed.trim_end();
                if !rest.is_empty() && rest.chars().all(|c| c == ch) && rest.chars().count() >= n {
                    open = None;
                }
                return "";
            }
            for ch in ['`', '~'] {
                let n = trimmed.chars().take_while(|&c| c == ch).count();
                // a backtick in the rest of the line makes it inline code, not a fence
                if n >= 3 && !(ch == '`' && trimmed[n..].contains('`')) {
                    open = Some((ch, n));
                    return "";
                }
            }
            line
        })
        .collect()
}

/// Inline code spans blanked: a run of n backticks up to the next run of exactly n.
fn without_inline_code(line: &str) -> String {
    let bytes = line.as_bytes();
    let run_at = |pos: usize| bytes[pos..].iter().take_while(|&&c| c == b'`').count();
    let mut out = String::with_capacity(line.len());
    let mut last = 0;
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] != b'`' {
            pos += 1;
            continue;
        }
        let open = run_at(pos);
        let mut scan = pos + open;
        let mut close = None;
        while scan < bytes.len() {
            if bytes[scan] == b'`' {
                let len = run_at(scan);
                if len == open {
                    close = Some(scan + len);
                    break;
                }
                scan += len;
            } else {
                scan += 1;
            }
        }
        match close {
            Some(end) => {
                out.push_str(&line[last..pos]);
                out.extend(std::iter::repeat_n(' ', end - pos));
                last = end;
                pos = end;
            }
            None => pos += open,
        }
    }
    out.push_str(&line[last..]);
    out
}

fn fragment(s: &str) -> Option<Fragment> {
    if let Some(id) = s.strip_prefix('^') {
        return (!id.is_empty()).then(|| Fragment::Block(id.to_owned()));
    }
    s.split('#')
        .map(str::trim)
        .rfind(|p| !p.is_empty())
        .map(|p| Fragment::Heading(p.to_owned()))
}

/// `%XX` decoded; the input unchanged when the result is not UTF-8.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

pub fn links(text: &str) -> Vec<Link> {
    let mut out = Vec::new();
    for (n, line) in unfenced(text).into_iter().enumerate() {
        let line = without_inline_code(line);
        let mut found: Vec<(usize, Link)> = Vec::new();
        for c in WIKI.captures_iter(&line) {
            let whole = c.get(0).unwrap();
            let inner = c[2].split('|').next().unwrap_or_default();
            let inner = inner.strip_suffix('\\').unwrap_or(inner);
            let (name, frag) = inner.split_once('#').unwrap_or((inner, ""));
            found.push((
                whole.start(),
                Link {
                    line: n + 1,
                    raw: whole.as_str().to_owned(),
                    target: name.trim().to_owned(),
                    fragment: fragment(frag),
                    markdown: false,
                },
            ));
        }
        for c in MARKDOWN.captures_iter(&line) {
            let whole = c.get(0).unwrap();
            let t = &c[1];
            let t = t
                .strip_prefix('<')
                .and_then(|t| t.strip_suffix('>'))
                .unwrap_or(t);
            if t.is_empty() || t.starts_with('#') || t.starts_with("//") || SCHEME.is_match(t) {
                continue;
            }
            let (name, frag) = t.split_once('#').unwrap_or((t, ""));
            found.push((
                whole.start(),
                Link {
                    line: n + 1,
                    raw: whole.as_str().to_owned(),
                    target: percent_decode(name),
                    fragment: fragment(&percent_decode(frag)),
                    markdown: true,
                },
            ));
        }
        found.sort_by_key(|(start, _)| *start);
        out.extend(found.into_iter().map(|(_, l)| l));
    }
    out
}

pub struct Anchors {
    headings: HashSet<String>,
    blocks: HashSet<String>,
}

impl Anchors {
    pub fn has_heading(&self, h: &str) -> bool {
        self.headings.contains(&normalise_heading(h))
    }

    pub fn has_block(&self, id: &str) -> bool {
        self.blocks.contains(&id.to_lowercase())
    }
}

pub fn anchors(text: &str) -> Anchors {
    let mut headings = HashSet::new();
    let mut blocks = HashSet::new();
    let mut prev = "";
    for line in unfenced(text) {
        if let Some(c) = HEADING.captures(line) {
            headings.insert(normalise_heading(c.get(1).map_or("", |m| m.as_str())));
        } else if SETEXT.is_match(line) && !prev.trim().is_empty() && !HEADING.is_match(prev) {
            headings.insert(normalise_heading(prev));
        }
        if let Some(c) = BLOCK.captures(line) {
            blocks.insert(c[1].to_lowercase());
        }
        prev = line;
    }
    Anchors { headings, blocks }
}

/// Lowercase, words of letters and digits joined by one space.
pub fn normalise_heading(s: &str) -> String {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Fragment {
        Fragment::Heading(s.into())
    }

    #[test]
    fn wiki_forms() {
        let l = links("[[a/b]] [[c|alias]]\n![[d#Some Head]] [[e#^blk-1]] [[#Self]]");
        let t: Vec<_> = l.iter().map(|l| l.target.as_str()).collect();
        assert_eq!(t, ["a/b", "c", "d", "e", ""]);
        let f: Vec<_> = l.iter().map(|l| l.fragment.clone()).collect();
        assert_eq!(
            f,
            [
                None,
                None,
                Some(h("Some Head")),
                Some(Fragment::Block("blk-1".into())),
                Some(h("Self"))
            ]
        );
        let lines: Vec<_> = l.iter().map(|l| l.line).collect();
        assert_eq!(lines, [1, 1, 2, 2, 2]);
        assert_eq!(l[2].raw, "![[d#Some Head]]");
        assert!(l.iter().all(|l| !l.markdown));
    }

    #[test]
    fn table_pipe() {
        let l = links("| [[github/x/README\\|x]] |");
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].target, "github/x/README");
        assert_eq!(l[0].fragment, None);
    }

    #[test]
    fn nested_heading_chain() {
        assert_eq!(links("[[n#A#B]]")[0].fragment, Some(h("B")));
    }

    #[test]
    fn markdown_links() {
        let l = links(
            "[a](notes/x.md) [b](<my note.md>) [c](my%20note.md#Two%20Words \"title\") [d](https://x.org) [e](#top) [f](mailto:a@b) ![g](pics/p.jpg)",
        );
        let t: Vec<_> = l.iter().map(|l| l.target.as_str()).collect();
        assert_eq!(t, ["notes/x.md", "my note.md", "my note.md", "pics/p.jpg"]);
        assert_eq!(l[2].fragment, Some(h("Two Words")));
        assert!(l.iter().all(|l| l.markdown));
    }

    #[test]
    fn inline_code_ignored() {
        let t = |s: &str| links(s).into_iter().map(|l| l.target).collect::<Vec<_>>();
        assert_eq!(t(r"`[[a]]` and ``[[b]] ` x`` then [[c]]"), ["c"]);
    }

    #[test]
    fn nested_and_indented_fences() {
        let t = |s: &str| links(s).into_iter().map(|l| l.target).collect::<Vec<_>>();
        assert_eq!(
            t(
                "````md\n```\n[[a]]\n```\n[[b]]\n````\n[[c]]\n- item\n  ```py\n  df[[\"open\", \"high\"]]\n  ```\n[[d]]\n~~~\n[[e]]\n"
            ),
            ["c", "d"] // the ~~~ fence is never closed
        );
    }

    #[test]
    fn heading_normalisation() {
        let n = normalise_heading;
        assert_eq!(n("Phase 2: rollout"), n("Phase 2 rollout"));
        assert_eq!(n("Use `foo` **now**"), n("use foo now"));
        assert_eq!(n("Café — notes"), n("café notes"));
        assert_ne!(n("Phase 2"), n("Phase2"));
    }

    #[test]
    fn anchors_found() {
        let a = anchors(
            "# Top\n## Second part: details ##\n```\n# not a heading ^hidden\n```\nA para. ^blk1\n",
        );
        assert!(
            a.has_heading("second part details")
                && a.has_heading("Top")
                && !a.has_heading("not a heading")
        );
        assert!(a.has_block("BLK1") && !a.has_block("hidden"));
    }

    #[test]
    fn callouts_hold_code_and_headings() {
        let t = |s: &str| links(s).into_iter().map(|l| l.target).collect::<Vec<_>>();
        assert_eq!(
            t("> [!example]\n> ```python\n> df[[\"open\", \"high\"]]\n> ```\n> [[c]]\n"),
            ["c"]
        );
        assert!(anchors("> [!note]\n> ## In Callout\n").has_heading("In Callout"));
    }

    #[test]
    fn inline_triple_backticks_are_not_a_fence() {
        let t = |s: &str| links(s).into_iter().map(|l| l.target).collect::<Vec<_>>();
        assert_eq!(t("```cargo test``` runs it\n[[gone]]\n"), ["gone"]);
    }

    #[test]
    fn setext_headings() {
        let a = anchors("Setext Head\n===\n\nSub head\n---\n\n---\n");
        assert!(a.has_heading("Setext Head") && a.has_heading("Sub head"));
    }

    #[test]
    fn balanced_parens_in_markdown_link() {
        assert_eq!(links("[md](paren_(1).md)")[0].target, "paren_(1).md");
    }
}
