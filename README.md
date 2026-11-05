# OpenCLI

A cross-platform CLI that turns OpenAPI documents into commands at runtime. Register an API once, then call its operations. Nothing is regenerated or recompiled when the spec changes.

```sh
opencli api add petstore --spec https://petstore.example.com/openapi.yaml
opencli petstore --help                      # resources and commands, generated from the spec
opencli petstore pets get 123 --dry-run      # prints the HTTP request
opencli petstore pets create --name Rex --tag dog --dry-run
opencli petstore pets create --describe      # machine-readable contract for scripts and agents
opencli api list | show petstore | versions petstore | remove petstore --yes
```

> **Status:** milestone 2 of 8 done. Specs import, and their operations become commands with help, `--describe` and `--dry-run`. Sending requests arrives in M3. See [docs/architecture.md](docs/architecture.md) and the [PRD](PRD.md).

Supported today: OpenAPI 3.0 documents in JSON or YAML, from a local file or an `https://` URL (`http://` only for localhost). Data is stored in the platform data directory, or in `$OPENCLI_HOME` if that is set.

### Command names

Commands follow `opencli <api> <resource…> <verb>`, derived from each operation's path and method: GET `/pets` → `pets list`, POST `/pets` → `pets create`, GET `/pets/{id}` → `pets get <ID>`, and so on. Spec authors can override names with `x-cli-name` and `x-cli-resource`. The full rules are in [ADR 0002](docs/adr/0002-command-surface.md).

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
cargo llvm-cov --workspace --fail-under-lines 80   # cargo install --locked cargo-llvm-cov; needs the rustup toolchain (llvm-tools)
```

## Contributing

- Branch off `main` and use Conventional Commits.
- Read `docs/architecture.md` and `docs/adr/` before structural changes. Write a new ADR for any decision that's hard to reverse.
