# rust-sandbox

Small Rust projects, one crate each under `projects/`, in one Cargo workspace. They share a lock
file, the target folder and the lint settings (`unsafe_code` forbidden, clippy pedantic).

| Project | What it does |
|---|---|
| [hello](projects/hello/) | The template: prints a greeting; copy it to start a project |

```sh
cargo run -p hello -- Rust   # Hello, Rust!
cargo test --workspace
```

Start a new one: `cargo new projects/<name>`, then in its `Cargo.toml` set `edition`,
`license` and `publish` to `.workspace = true` and add `[lints] workspace = true`. CI runs
`cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` for the whole workspace.
