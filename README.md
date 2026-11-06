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

> **Status:** feature-complete for the original plan, plus follow-ups. Import, refresh and versioning for OpenAPI 3.0/3.1/3.2 and Swagger 2.0, authenticated requests including file uploads, shell completion, an audit log, and plugins. See [docs/architecture.md](docs/architecture.md) and the [PRD](PRD.md).

Supported today: OpenAPI 3.0, 3.1 and 3.2, and Swagger 2.0 documents in JSON or YAML, from a local file or an `https://` URL (`http://` only for localhost). Request bodies can be JSON, form-encoded, multipart (file fields become `--<field> <FILE>` flags) or text. Data is stored in the platform data directory, or in `$OPENCLI_HOME` if that is set.

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

### Plugins

A plugin manifest adds aliases, table layouts and credential providers to one API:

```toml
# jira-helpers.toml
name = "jira-helpers"
version = "1.0.0"
api = "jira"

[aliases.mine]                       # opencli jira mine
command = "issues list"
args = ["--assignee", "currentUser()"]

[formats."issues list"]              # table columns, by JSON path
columns = ["key", { path = "fields.summary", header = "SUMMARY" }, "fields.status.name"]

[credentials.oauth]                  # prints {"value": "<token>"}; used after OPENCLI_JIRA_OAUTH
exec = "jira-token"
```

```sh
opencli plugin install ./jira-helpers.toml   # shows everything it adds and runs, then asks
opencli plugin list
opencli plugin remove jira-helpers
```

Any `opencli-<name>` executable on `PATH` also runs as `opencli <name> …`, as with git and kubectl. Registered APIs take precedence. See [ADR 0005](docs/adr/0005-plugin-model.md) for the full contract.

### Shell completion, logs and audit

```sh
opencli completion zsh > ~/.zfunc/_opencli   # bash, zsh, fish, powershell, elvish
opencli -v petstore pets list                # debug logs with per-phase timings (or RUST_LOG=debug)
tail -n 5 ~/Library/Application\ Support/opencli/audit.log   # or $OPENCLI_HOME/audit.log
```

Completion scripts include every API registered when they're generated, so regenerate them after `api add`, `refresh` or `remove`. The audit log records each call and each change to APIs or credentials as a JSON line, without secrets or query strings. Set `OPENCLI_AUDIT=off` to disable it.

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

To create a release folder for this computer:

```sh
./build.sh                 # detect this computer's OS and architecture
./build.sh --os mac        # macOS (Apple Silicon or Intel)
./build.sh --os linux      # GNU/Linux, run on Linux
./build.sh --os windows    # Windows, run in Git Bash with MSVC Rust
```

Each OS must be built on a matching host with native build tools installed
(Xcode Command Line Tools on macOS, a C compiler on Linux, or Visual Studio
C++ Build Tools on Windows). The script uses the Rust host architecture and
creates `dist/opencli-<os>-<arch>/` containing the release executable and this
README. For example, on Apple Silicon:

```sh
./dist/opencli-mac-arm64/opencli --help
```

Run the executable directly or put its folder on `PATH`; Rust is not needed
to run it. The folder can be copied to a compatible computer with the same OS,
architecture, and system libraries (Linux builds use the build host's glibc).
Registered APIs and credentials remain in the normal platform data directory;
they are not included in the package. Set `OPENCLI_HOME` to choose a different
data directory.

## Build and test

All of these must pass with zero warnings before a change is merged. CI (`azure-pipelines.yml`) doesn't build on every push. It runs as a PR check (a build-validation policy on `main`), on demand, and weekly on `main`. A default run is one Linux job; tick *full matrix* when queuing, or wait for the weekly run, to also cover macOS and Windows.

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
