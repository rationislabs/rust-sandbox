# rust-sandbox

Small Rust projects, one crate each under `projects/`, in one Cargo workspace. They share a lock
file, the target folder and the lint settings (`unsafe_code` forbidden, clippy pedantic).

| Project | What it does |
|---|---|
| [hello](projects/hello/) | The template: prints a greeting; copy it to start a project |
| [todo](projects/todo/) | Command-line task list: due dates, tags, overdue first |
| [finance](projects/finance/) | Terminal finance tracker: budgets vs actual, categories, trend (ratatui) |
| [logline](projects/logline/) | Logs from several sources into one HTML timeline: collapsed repeats, incident bands |
| [vaultcheck](projects/vaultcheck/) | Checks an Obsidian vault: broken links, orphans, folders without a repo, dangling symlinks |

```sh
cargo run -p hello -- Rust   # Hello, Rust!
cargo test --workspace
```

Start a new one: `cargo new projects/<name>`, then in its `Cargo.toml` set `edition`,
`license` and `publish` to `.workspace = true` and add `[lints] workspace = true`. CI runs
`cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` for the whole workspace.

Every project here was built with AI assistance.
