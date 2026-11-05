# OpenCLI

Specification-driven CLI: imported OpenAPI documents become commands at runtime, with no code generation.

## Stack

- Rust (pinned in `rust-toolchain.toml`, edition 2024), clap, tokio, reqwest, sqlx (SQLite), keyring, miette, tracing

## Commands

- Build: `cargo build --workspace --all-targets`
- Test: `cargo test --workspace`
- Lint/format: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
- Audit: `cargo deny check`
- Coverage: `rustup run 1.99.1 cargo llvm-cov --workspace --fail-under-lines 80`
- Run locally: `OPENCLI_HOME=/tmp/opencli cargo run -p opencli -- <args>`

## Layout

- `crates/opencli` — binary: clap commands, spec sources (file/URL), output rendering, dependency wiring
- `crates/opencli-core` — domain model and `ApiName` rules; no I/O
- `crates/opencli-spec` — OpenAPI parsing (3.0 today) and semantic validation
- `crates/opencli-store` — SQLite metadata store (`migrations/` holds the schema)
- `fixtures/` — sample specs shared by every crate's tests
- Further crates (`-runtime`, `-auth`, `-plugin`) are added in the milestone that needs them; see `docs/architecture.md`.

## Architecture

- Read `docs/architecture.md` before structural changes.
- Check `docs/adr/` before proposing alternatives; write a new ADR for any hard-to-reverse decision.

## Project-specific rules

- `opencli-core` depends on no other workspace crate and on no I/O crates (no sqlx, reqwest, keyring).
- Parser crates (`openapiv3`, `oas3`) are imported only inside `opencli-spec`.
- Dependency versions live in `[workspace.dependencies]`; member crates use `.workspace = true`.
- Every command supports `--output json` and runs without prompts when stdin isn't a terminal.
- Store queries are literal SQL with bound parameters; every store method has an integration test against a real SQLite file.
- Integration-test files start with `#![allow(clippy::unwrap_used, clippy::expect_used)]`, because helpers outside `#[test]` panic to fail tests.
