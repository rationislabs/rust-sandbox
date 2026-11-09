# vaultcheck

Finds what is broken in an Obsidian vault. Links rot when notes are renamed, repos are renamed or
removed while their vault folder stays, and symlinked READMEs point at paths that moved.
vaultcheck reports all of it and changes nothing.

```sh
vaultcheck                              # the vault at $OBSIDIAN_VAULT, else ~/Documents/vault
vaultcheck ~/notes                      # another vault
vaultcheck --root github=~/Development/pro/github --root gitlab=~/Development/pro/gitlab/rationis_labs
vaultcheck --exclude gitlab             # report nothing under a vault folder, stat none of its symlinks
vaultcheck --json                       # machine-readable findings
```

## What it checks

1. **Broken links**: wiki links (`[[note]]`, `[[note|alias]]`, `[[note#Heading]]`,
   `[[note#^block]]`, embeds `![[...]]`) and local Markdown links (`[text](path.md)`). A
   heading or block must exist in the target note. Links in code blocks, inline code and
   comments (`%% %%`, `<!-- -->`) are ignored.
2. **Orphans**: notes no other note links to. `Home.md` and `inbox/` never count.
3. **Folders without a repo**: `vault/<host>/<repo>/` with no `<repo>` folder under the
   `--root` given for that host. Without `--root`, the roots are `~/Development/pro/github`
   and `~/Development/pro/gitlab/rationis_labs`.
4. **Dangling symlinks**: symlinks in the vault whose target is gone.

Links resolve the way Obsidian resolves them: case-insensitive, `.md` optional, `./` and `../`
relative to the note, partial paths (`[[deep/gamma]]` finds `notes/deep/gamma.md`), an exact
vault path wins over partial matches, and a name that matches several files opens the one in
the note's own folder, else the shortest path.

Broken links, folders without a repo and dangling symlinks exit 1. Ambiguous links, orphans and
heading links into notes that cannot be read (symlinks, non-UTF-8) are listed, never fail the
run. A link whose path points into a folder that cannot be read counts as unverified, not
broken. Usage errors and an unreadable vault exit 2.

`--exclude` is a report filter, not a blind spot: notes under it are still read, so links into
them resolve and their links still count against orphans.

It is read-only, and it never reads through a symlink: a symlink is only checked for whether
its target exists, and a symlinked folder is never entered.

## Sample run

On the test fixture (`tests/fixtures/vault`, with the symlinks the tests add):

```
$ vaultcheck /tmp/demo/vault --root github=/tmp/demo/repos/github --root gitlab=/tmp/demo/repos/gitlab
broken links (5)
  cases.md:6: [[beta#Missing]] no heading "Missing" in beta.md
  cases.md:14: [[ghost]] matches no note
  cases.md:16: [nope](notes/nope.md) matches no note
  cases.md:27: [[beta#^nope]] no block "^nope" in beta.md
  gitlab/kept/plan.md:1: [[ghost-in-gitlab]] matches no note
dangling symlinks (2)
  gitlab/kept/dead.md -> /tmp/demo/outside/nothing
  sym/gone.md -> /tmp/demo/outside/missing.md
folders without a repo (2)
  github/missing has no repo at /tmp/demo/repos/github/missing
  gitlab/kept has no repo at /tmp/demo/repos/gitlab/kept
ambiguous links (2, not an error)
  b/sub/x.md:1: [[dup]] matches 2 files, opens b/sub/dup.md (also a/dup.md)
  cases.md:12: [[dup]] matches 2 files, opens a/dup.md (also b/sub/dup.md)
orphans (1, not an error)
  notes/lonely.md has no incoming links
14 notes, 29 links, 1 unverified
```

On stderr: `warning: notes/latin1.md: not UTF-8, skipped`. Exit code 1. On a vault of 174 notes
it runs in under 0.1 s.

## Test

```sh
cargo test -p vaultcheck
```
