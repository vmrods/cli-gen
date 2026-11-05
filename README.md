# OpenCLI

A cross-platform CLI that turns OpenAPI documents into commands at runtime. Register an API once, then call its operations. Nothing is regenerated or recompiled when the spec changes.

```sh
opencli api add petstore --spec https://petstore.example.com/openapi.yaml
opencli petstore --help                      # resources and commands, generated from the spec
opencli petstore pets list --limit 10        # table output; -o json for raw JSON
opencli petstore pets get 123
opencli petstore pets create --name Rex --tag dog --yes
opencli petstore pets create --describe      # machine-readable contract for scripts and agents
opencli petstore pets delete 123 --dry-run   # print the request without sending it
opencli --server https://staging.example.com petstore pets list
```

> **Status:** milestone 5 of 8 done. Specs import, refresh and version, and their operations run as authenticated HTTP requests. OpenAPI 3.1 and Swagger 2.0 arrive in M6. See [docs/architecture.md](docs/architecture.md) and the [PRD](PRD.md).

Supported today: OpenAPI 3.0 documents in JSON or YAML, from a local file or an `https://` URL (`http://` only for localhost). Data is stored in the platform data directory, or in `$OPENCLI_HOME` if that is set.

### Keeping specs up to date

```sh
opencli api refresh petstore --dry-run   # what would change: added, removed, renamed, changed commands
opencli api refresh petstore             # re-import from the original file or URL
opencli api refresh petstore --spec ./petstore-v2.yaml   # import from a new source
opencli api versions petstore            # history; `*` marks the current version
```

Refreshing to content seen before reactivates that version. Credentials carry over, except for security schemes the new version drops.

### Authentication

```sh
opencli auth login petstore                         # asks which scheme, then for the secret (hidden)
printf %s "$KEY" | opencli auth login petstore --scheme apiKey --with-token
opencli auth login crm --scheme oauth --client-id my-app --with-token < secret.txt   # client credentials
opencli auth login crm --scheme oauth --flow authorization-code --client-id my-app   # browser sign-in (PKCE)
opencli auth status petstore                        # what's configured; never shows secrets
opencli auth logout petstore
```

Credentials go to the OS keyring (Keychain, Credential Manager or Secret Service). Without one, they go to a passphrase-encrypted file (`OPENCLI_PASSPHRASE`, or a prompt). Set `OPENCLI_CREDENTIAL_STORE=keyring|file` to choose. In CI, set `OPENCLI_<API>_<SCHEME>` instead of logging in: for example `OPENCLI_PETSTORE_APIKEY`, or `user:pass` for basic auth. OAuth tokens are renewed automatically, and credentials are never sent over plain HTTP except to localhost.

### Scripting

Commands never prompt when stdin isn't a terminal. Requests that change data need `--yes` instead. Exit codes are stable: `0` success, `1` error, `2` usage, `3` HTTP 4xx, `4` HTTP 5xx, `5` network or timeout. The response body still goes to stdout on HTTP errors, so problem details can be parsed.

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
