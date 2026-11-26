# OpenCLI

[![CI](https://github.com/vmrods/cli-gen/actions/workflows/ci.yml/badge.svg)](https://github.com/vmrods/cli-gen/actions/workflows/ci.yml) ![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)

A cross-platform CLI that turns OpenAPI documents into commands at runtime. Register an API once, then call its operations. Nothing is regenerated or recompiled when the spec changes.

Start with the **[documentation hub](docs/README.md)** for a guided setup, everyday workflows, authentication, the complete command/configuration reference, plugins, troubleshooting and a source-code walkthrough.

These examples use the current URL-shaped command model. Check `opencli --version` and your operation's `--help` if an older installed build still uses `list`/`get`/`create` commands. See the [upgrade notes](docs/troubleshooting.md#commands-changed-after-an-upgrade).

```sh
opencli api add petstore --spec https://petstore.example.com/openapi.yaml   # asks for the base URL
opencli --server https://crm.internal.example.com api add crm --spec ./crm.yaml   # or give it up front
opencli api set-server crm https://crm-staging.internal.example.com              # change it later
opencli petstore --help                      # resources and commands, generated from the spec
opencli petstore pets --limit 10             # GET /pets; table output, -o json for raw JSON
opencli petstore pets --pet-id 123           # GET /pets/{petId}
opencli petstore pets post --name Rex --tag dog --yes
opencli petstore pets post --describe        # machine-readable contract for scripts and agents
opencli petstore pets delete --pet-id 123 --dry-run   # print the request without sending it
opencli --server https://staging.example.com petstore pets
```

> **Status:** feature-complete for the original plan, plus follow-ups. Import, refresh and versioning for OpenAPI 3.0/3.1/3.2 and Swagger 2.0, authenticated requests including file uploads, shell completion, an audit log, and plugins. See [docs/architecture.md](docs/architecture.md) and the [PRD](PRD.md).

Supported today: OpenAPI 3.0, 3.1 and 3.2, and Swagger 2.0 documents in JSON or YAML, from a local file or an `https://` URL (`http://` only for localhost). Request bodies can be JSON, form-encoded, multipart (file fields become `--<field> <FILE>` flags) or text. Data is stored in the platform data directory, or in `$OPENCLI_HOME` if that is set.

### Arguments and request bodies

```sh
opencli petstore pets --pet-id 123                  # every input is a flag named after the spec, path parameters too
opencli petstore pets --limit 10 --tag dog          # query, header and cookie parameters
opencli petstore pets post --name Rex               # simple top-level body fields are flags too
opencli petstore pets post --body @pet.json         # the whole body from a file (or inline JSON, or - for stdin)
opencli petstore pets post --body @pet.json --name Rex   # flags override fields of --body
opencli petstore pets post -i                       # ask for every flag and body field in turn
```

- In a terminal, required values you leave out are asked for instead of failing. `-i`/`--interactive` asks for everything, optional values included, and walks nested objects and lists. Scripts and agents (no terminal) are never asked: missing values fail with a usage error, and `--interactive` is refused.
- Secret body fields (`format: password`, or string fields named like a password, secret, API key or token) never become flags, so they stay out of shell history. They are asked for hidden in a terminal, or come from `--body @file` or `--body -`.
- JSON bodies are checked against the spec before sending (and in `--dry-run`): missing required fields, wrong types, values outside an enum, at any depth. Every problem is listed with its path; `--no-validate` sends the body anyway.

### Keeping specs up to date

```sh
opencli api refresh petstore --dry-run   # what would change: added, removed, renamed, changed commands
opencli api refresh petstore             # re-import from the original file or URL
opencli api refresh petstore --spec ./petstore-v2.yaml   # import from a new source
opencli api versions petstore            # history; `*` marks the current version
```

Refreshing to content seen before reactivates that version. Credentials carry over, except for security schemes the new version drops.

### Authentication

See the [authentication guide](docs/authentication.md) for scheme selection, sign-in, OAuth, storage and credential precedence.

```sh
opencli auth login petstore                         # asks which scheme, then for the secret (hidden)
printf %s "$KEY" | opencli auth login petstore --scheme apiKey --with-token
opencli auth login crm --scheme oauth --client-id my-app --with-token < secret.txt   # client credentials
opencli auth login crm --scheme oauth --flow authorization-code --client-id my-app   # browser sign-in (PKCE)
opencli auth status petstore                        # what's configured; never shows secrets
opencli auth logout petstore
```

**APIs with their own sign-in.** When an API signs in through one of its operations (a POST ending in `login`, `signin` or `token`) and returns a token, OpenCLI saves that token as the API's credential, with `Bearer ` added where the spec sends it as an `Authorization` API key. `opencli auth login secops` offers that sign-in and asks for its fields (the password hidden), or run the operation itself: `opencli secops v1 account login post -i`. A line on stderr confirms the save; `--no-save` skips it. Details in [ADR 0010](docs/adr/0010-save-sign-in-tokens.md).

**APIs that share a token.** When one API accepts another's tokens (a trading API that takes the identity service's JWTs), link them: `opencli api set-credentials trading --from secops`. trading then reads secops's stored token at call time, converted to its own format, so one `opencli auth login secops` serves both and logging out of secops logs out of both. `--clear` unlinks. See [ADR 0011](docs/adr/0011-linked-credentials.md).

Credentials go to the OS keyring (Keychain, Credential Manager or Secret Service). Without one, they go to a passphrase-encrypted file (`OPENCLI_PASSPHRASE`, or a prompt). Set `OPENCLI_CREDENTIAL_STORE=keyring|file` to choose. In CI, set `OPENCLI_<API>_<SCHEME>` instead of logging in: for example `OPENCLI_PETSTORE_APIKEY`, or `user:pass` for basic auth. OAuth tokens are renewed automatically, and credentials are never sent over plain HTTP except to localhost.

For APIs accepting another registered API's token, link their credentials once:

```sh
opencli api set-credentials trading --from identity   # both APIs must already be registered
opencli auth login identity
```

The trading API reads the source token at call time. See [shared sign-in](docs/authentication.md#share-one-sign-in-between-two-apis).

### Claude Code and Codex integration

OpenCLI includes a local MCP server and a reusable usage skill. Expose only the
APIs you want the agent to access; execution defaults to GET operations:

```sh
claude mcp add --scope user --transport stdio opencli -- "$HOME/.local/bin/opencli" mcp serve --api trading
codex mcp add opencli -- "$HOME/.local/bin/opencli" mcp serve --api trading
```

The five tools discover APIs, search operations, describe inputs, plan requests
and execute through the existing credential/runtime path. Writes require an
explicit server option and per-call confirmation. See the [agent integration
guide](docs/agents.md) for setup, skill installation, output limits and ChatGPT
connection requirements.

### Plugins

A plugin manifest adds aliases, table layouts and credential providers to one API:

```toml
# jira-helpers.toml
name = "jira-helpers"
version = "1.0.0"
api = "jira"

[aliases.mine]                       # opencli jira mine
command = "issues"
args = ["--assignee", "currentUser()"]

[formats."issues"]                   # table columns, by JSON path
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
opencli -v petstore pets                     # debug logs with per-phase timings (or RUST_LOG=debug)
tail -n 5 ~/Library/Application\ Support/opencli/audit.log   # or $OPENCLI_HOME/audit.log
```

Completion scripts include every API registered when they're generated, so regenerate them after `api add`, `refresh` or `remove`. The audit log records each call and each change to APIs or credentials as a JSON line, without secrets or query strings. Set `OPENCLI_AUDIT=off` to disable it.

### Scripting

Commands never prompt when stdin isn't a terminal. Requests that change data need `--yes` instead. Exit codes are stable: `0` success, `1` error, `2` usage, `3` HTTP 4xx, `4` HTTP 5xx, `5` network or timeout. The response body still goes to stdout on HTTP errors, so problem details can be parsed.

### Selecting response data

Use `--query` to select a field before rendering an API response. For an envelope such as `{"data": [{"id": 1}], "message": "Items found"}`:

```sh
opencli trading v1 quotes --query data -o table
opencli trading v1 quotes --query data --output json
opencli trading v1 quotes --query data.0 --output json
```

Paths use dots for nested fields and numeric array indices (`data.items`, `data.0.id`). This is field selection, not a full JMESPath expression language; filters, projections and field names containing dots are unsupported. Missing paths and non-JSON responses produce an error. HTTP error responses retain their full body and status exit code. Selection applies before plugin table columns, and requires an API operation without `--dry-run` or `--describe`.

### Command names

The URL is the command: `opencli <api> <path words…> [method]`. Path words are the path's fixed segments (a leading `api` or `rest` is dropped; `v1` stays). GET has no method word, so a bare command only ever reads; other methods are named. Path parameters are flags like any other input.

| Spec | Command |
|---|---|
| `GET /api/v1/countries` | `countries` (prefixed `opencli <api> v1`) |
| `GET /api/v1/countries/{id}` | `countries --id 5` |
| `POST /api/v1/countries` | `countries post --body @c.json` |
| `DELETE /api/v1/countries/{id}` | `countries delete --id 5` |
| `GET /api/v1/countries/{id}/cities` | `countries cities --id 5` |
| `POST /api/v1/account/login` | `account login post -i` |

Operations that differ only by path parameters (`/countries` and `/countries/{id}`) are variants of one command, and the path flags you give pick the URL. `--help` lists every variant. A write without the path flags another variant takes acts on the whole collection, and the confirmation says `(all items)`. To find a command in a large API, search names, paths, summaries and operationIds: `opencli api commands secops login` lists every match with its endpoint (`-o json` for scripts). Spec authors can override names with `x-cli-resource` (the words) and `x-cli-name` (the method word). The full rules are in [ADR 0009](docs/adr/0009-path-shaped-commands.md).

## Getting started

Requires the Rust toolchain pinned in `rust-toolchain.toml` (installed automatically by rustup).

```sh
cargo build --workspace
cargo run -p opencli -- --help
```

To package a ready-to-use folder, see [Packaging](#packaging).

## Install locally

On macOS or Linux, install the current packaged version into `~/.local/bin`:

```sh
./install.sh                       # uses the current workspace version in dist/
opencli --version
opencli api list
```

If a package has not been built, run `./build.sh` first. The installer checks its
SHA-256 checksum, verifies that the binary runs, then replaces the installed
executable. Re-run it after building to upgrade. No sudo is needed.

```sh
./install.sh --package ./dist/opencli-0.1.2-mac-arm64
./install.sh --bin-dir /some/directory/on/your/PATH
```

If the destination is absent from `PATH`, the installer prints the exact `export`
command to run and add to your shell startup file (`~/.zshrc` for zsh or
`~/.bashrc` for bash). It does not edit those files. Windows users can add the
package's `bin` directory to their user PATH.

Packages also include `install.sh`, which can be run from the unpacked folder
without this repository or Rust. Uninstall with `rm ~/.local/bin/opencli`.
Registered APIs and credentials stay in the platform data directory across
installation, upgrades and uninstallation.

## Packaging

`./build.sh` increments the workspace patch version on each build (for example, `0.1.0` → `0.1.1`), updates `Cargo.toml` and `Cargo.lock`, and builds a ready-to-use folder at `dist/opencli-<version>-<os>-<arch>/`, containing `bin/opencli` (`opencli.exe` on Windows), this README, `install.sh`, `INSTALL.txt`, `SHA256SUMS`, documentation in `docs/` and the usage skill in `skills/`:

```sh
./build.sh                          # this computer's OS and architecture
./build.sh --os mac --arch x86_64   # Intel Macs (needs a Mac)
./build.sh --os linux               # built in Docker (Docker Desktop running)
./build.sh --os windows             # x86_64, cross-built in Docker with MinGW
./build.sh --out /tmp/release       # somewhere other than dist/
```

- Failed builds restore the previous version and lockfile and remove the partial package. `--help` and invalid options do not change the version. Run builds one at a time; each successful invocation creates a new version, including builds for another platform.
- All builds require rustup and the pinned toolchain on the host to update the workspace version and lockfile, including Docker builds.
- Native builds use the toolchain pinned in `rust-toolchain.toml` via rustup, plus the platform's C build tools: Xcode Command Line Tools on macOS, a C compiler on Linux, Visual Studio C++ Build Tools on Windows (run the script from Git Bash; it builds with MSVC).
- Builds for another OS run in Docker, so Linux and Windows packages can be made from a Mac. macOS packages need a Mac.
- Rust isn't needed to run the result: put `bin/` on `PATH` or run the binary directly. Linux builds link against the build host's glibc. The Docker builds use Debian 12's glibc 2.36, so they need Debian 12, Ubuntu 23.04 or newer; for older systems (Ubuntu 22.04, RHEL 9), run `./build.sh` on that system.
- Registered APIs and credentials stay in the platform data directory (or `$OPENCLI_HOME`); they aren't part of the package.

## Build and test

All of these must pass with zero warnings before a change is merged. CI (`azure-pipelines.yml`) doesn't build on every push. It runs as a PR check (a build-validation policy on `main`), on demand, and weekly on `main`. A default run is one Linux job; tick *full matrix* when queuing, or wait for the weekly run, to also cover macOS and Windows.

```sh
shellcheck build.sh install.sh tests/build_test.sh tests/install_test.sh
bash tests/build_test.sh
bash tests/install_test.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                         # cargo install --locked cargo-deny
cargo llvm-cov --workspace --fail-under-lines 80   # cargo install --locked cargo-llvm-cov; needs the rustup toolchain (llvm-tools)
```

## Contributing

- Branch off `main` and use Conventional Commits.
- Read `docs/architecture.md` and `docs/adr/` before structural changes. Write a new ADR for any decision that's hard to reverse.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
