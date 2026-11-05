# OpenCLI

A cross-platform CLI that turns OpenAPI documents into commands at runtime. Register an API once, then call its operations. Nothing is regenerated or recompiled when the spec changes.

```sh
opencli api add crm --spec https://api.example.com/swagger/v1/swagger.json
opencli auth login crm
opencli crm users list --output json
opencli crm users get 123
```

> **Status:** early development (milestone 0: workspace foundation). The commands above are the target interface. See [docs/architecture.md](docs/architecture.md) and the [PRD](PRD.md).

## Getting started

Requires the Rust toolchain pinned in `rust-toolchain.toml` (installed automatically by rustup).

```sh
cargo build --workspace
cargo run -p opencli -- --help
```

## Build and test

All of these must pass with zero warnings before a change is merged. Azure Pipelines (`azure-pipelines.yml`) runs them on Linux, macOS and Windows.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                         # cargo install --locked cargo-deny
cargo llvm-cov --workspace --fail-under-lines 80   # cargo install --locked cargo-llvm-cov
```

## Contributing

- Branch off `main` and use Conventional Commits.
- Read `docs/architecture.md` and `docs/adr/` before structural changes. Write a new ADR for any decision that's hard to reverse.
