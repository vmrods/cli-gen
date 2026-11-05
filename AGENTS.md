# OpenCLI

Specification-driven CLI: imported OpenAPI documents become commands at runtime, with no code generation.

## Stack

- Rust (pinned in `rust-toolchain.toml`, edition 2024), clap, tokio, reqwest, sqlx (SQLite), keyring, miette, tracing

## Commands

- Build: `cargo build --workspace --all-targets`
- Test: `cargo test --workspace`
- Lint/format: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
- Audit: `cargo deny check`
- Run locally: `cargo run -p opencli -- <args>`

## Layout

- `crates/opencli` — binary: clap root and dependency wiring
- Further crates (`opencli-core`, `-spec`, `-store`, `-runtime`, `-auth`, `-plugin`) are added in the milestone that needs them; see `docs/architecture.md`.

## Architecture

- Read `docs/architecture.md` before structural changes.
- Check `docs/adr/` before proposing alternatives; write a new ADR for any hard-to-reverse decision.

## Project-specific rules

- `opencli-core` depends on no other workspace crate and on no I/O crates (no sqlx, reqwest, keyring).
- Parser crates (`openapiv3`, `oas3`) are imported only inside `opencli-spec`.
- Dependency versions live in `[workspace.dependencies]`; member crates use `.workspace = true`.
- Every command supports `--output json` and runs without prompts when stdin isn't a terminal.
