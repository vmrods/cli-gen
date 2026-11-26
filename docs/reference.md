# Reference

[Documentation home](README.md) · [User guide](user-guide.md)

This reference describes the current source. The installed executable's help
is authoritative for its build. Names in `<angle brackets>` are placeholders.

## Global options

| Option | Meaning | Default |
|---|---|---|
| `-o, --output table\|json` | Render structured output; does not convert text/binary HTTP bodies | `table` |
| `--query <path>` | Select a successful JSON API response field before formatting | Whole response |
| `--server <url>` | Override the server for one call; on import, save this base URL | Saved URL, then spec |
| `--timeout <seconds>` | Request timeout; integer at least 1 | 30 |
| `-v, --verbose` | Debug logging to stderr; `RUST_LOG` takes precedence | Off |
| `-h, --help` | Help at the selected command level | — |
| `-V, --version` | Executable version | — |

Use `--output` with two dashes, or `-o`; `-output` is not the long option.
`--query` is only for API operation responses, not built-in management results.
Spec loading has its own fixed timeout; `--timeout` does not change that loader.

## API management

| Command | Behavior |
|---|---|
| `api add <name> [--spec <file-or-url>]` | Import and register; terminal can prompt for source/server |
| `api list` | List registrations |
| `api show <name>` | Servers, saved URL, credential link, schemes and operations |
| `api commands <name> [words...]` | Search operations; every word must match, case-insensitive |
| `api set-server <name> <url>` | Save an absolute HTTP(S) base URL |
| `api set-server <name> --clear` | Remove the saved URL and follow the spec |
| `api set-credentials <name> --from <other-api>` | Link a compatible token scheme to another registration |
| `api set-credentials <name> --clear` | Remove the credential link |
| `api refresh <name> [--spec <source>] [--dry-run]` | Compare and optionally store a spec version |
| `api versions <name>` | List imported versions and mark the active one |
| `api remove <name> [--yes]` | Remove registration, versions, cache and stored credentials |

There is no separate `config` command, API rename command or version-switch
command. `config` is nevertheless a reserved API name. To reuse older spec
content, refresh from that document. Use a separate registration for a separate
base URL/credential setup if you need both at once.

## Authentication commands

| Command | Options |
|---|---|
| `auth login <api>` | `--scheme`, `--flow client-credentials\|authorization-code`, `--client-id`, repeated `--scope`, `--username`, `--with-token`, `--redirect-port`, `--no-browser` |
| `auth status <api>` | Reports schemes, configuration source, variable names and OAuth expiry |
| `auth logout <api>` | Optional `--scheme`; otherwise removes all that API's stored schemes |

Secret values are prompted for hidden or read from stdin with `--with-token`.
There is no password/token value argument. See [authentication](authentication.md)
for sign-in detection, links, credential representation and storage.

## Dynamic operation options

| Option | Availability and behavior |
|---|---|
| `--<spec-parameter> <value>` | Path/query/header/cookie parameters; names kebab-cased |
| `--<body-field> <value>` | Supported scalar fields; secret fields omitted |
| `--body <JSON\|@FILE\|->` | Operations with a body; a whole body inline, from file or stdin |
| `-i, --interactive` | Ask for all inputs, including optional ones; terminal required |
| `--describe` | Print the JSON contract and variants; no call or credentials needed |
| `--dry-run` | Print the validated request plan; no call or credentials needed |
| `--yes` | Non-GET operations; approve the request without a prompt |
| `--no-validate` | Body-bearing operations; bypass supported body schema checks |
| `--no-save` | Recognized sign-in operations; skip saving the returned token |

Flags are generated per command and selected variant. Reserved flags get
location prefixes when a spec parameter collides. Consult the operation's help
rather than assuming every option exists on every command.

## Local MCP server

```sh
opencli mcp serve --api trading
```

Launch this command through a local MCP client with piped stdin/stdout. An
explicit `--api` is required and may be repeated. Optional repeated
`--operation "METHOD /spec/path"` filters narrow the allowed operations.
Execution defaults to GET only. `--allow-writes` permits writes only when the
individual call also sets `confirmed: true` after user authorization.

`--max-output-bytes` defaults to 32768, with a range of 1024–262144. It limits
the complete serialized tool result. Output arrays default to 20 items and
accept a per-call limit of 1–100. The five tools are `list_apis`,
`search_operations`, `describe_operation`, `plan_request` and
`execute_operation`. Errors are structured and set MCP `isError`.

Configure saved server URLs before launch: global `--server` and `--query`
are rejected for this command. `--output` does not change the MCP protocol
stream. The normal global `--timeout` controls outbound requests. No HTTP
listener is started. `mcp` is a reserved API registration name.

See [agent integration](agents.md) for client setup, the usage skill, linked
credentials, expiry recovery and a verified live-read walkthrough.

## Plugins and completion

| Command | Behavior |
|---|---|
| `plugin install <file-or-url> [--yes]` | Show additions and programs, then install a manifest |
| `plugin list` | List installed manifests and executable plugins |
| `plugin remove <name> [--yes]` | Remove a manifest plugin |
| `completion <shell>` | Print completions for built-ins and currently registered APIs |

Completion shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`. Completion output
is a shell script rather than a JSON document, even with `-o json`.

## Environment variables

| Variable | Purpose |
|---|---|
| `OPENCLI_HOME` | Nonempty path overriding the platform data directory |
| `OPENCLI_CREDENTIAL_STORE` | `keyring` or `file`; unset/empty chooses automatically |
| `OPENCLI_PASSPHRASE` | Passphrase for encrypted file credentials; inject securely |
| `OPENCLI_<API>_<SCHEME>` | Per-scheme credential override; exact name in `auth status` |
| `OPENCLI_AUDIT=off` | Disable audit writes; comparison is case-insensitive |
| `RUST_LOG` | Logging filter; overrides the `-v` fallback |
| `NO_COLOR` | Disable terminal colors |
| `PATH` | Executable plugin/provider discovery and installed CLI lookup |

Credential providers receive `OPENCLI_API`, `OPENCLI_SCHEME` and
`OPENCLI_SCHEME_TYPE`. Executable plugins receive `OPENCLI_BINARY`, the current
CLI executable path. Those variables are supplied to child programs; they are
not general user configuration. See [plugin contracts](plugins.md).

There is no general user configuration file reader or automatic `.env` loader.
Set variables in the calling shell, process environment or CI secret injection.
An `OPENCLI_HOME` override selects another directory; it does not migrate files.

## Data storage

| Platform | Default data directory |
|---|---|
| macOS | `~/Library/Application Support/opencli/` |
| Linux | `$XDG_DATA_HOME/opencli/`, or `~/.local/share/opencli/` |
| Windows | `%APPDATA%\opencli\data\` |
| Any | `$OPENCLI_HOME` when set to a nonempty path |

| Entry | Contents and role |
|---|---|
| `metadata.db` | SQLite registrations, original/imported specs, version history and local settings |
| `cache/<api>.json` | Derived command tree; rebuilt on a cache miss or incompatible version |
| `credentials.enc` | Encrypted secrets when file-backed storage is used |
| `audit.log`, `audit.log.1` | Current and rotated JSON-line activity records |
| `plugins/<name>.toml` | Installed plugin manifests |

These are separate from `dist/`, the executable and your original spec file.
The normal command path can use the cache without rereading the original spec.
If the original file is moved, calls still work, but refresh needs a new source.

OS keyring entries use service `opencli` and account `<api>/<scheme>`; they are
not files inside `OPENCLI_HOME`. SQLite can create companion journal/WAL files.
For a backup, stop OpenCLI processes and copy the whole data directory rather
than a database while it is changing. File credentials also need their original
passphrase. OS keyring credentials need separate handling or a new sign-in on
the destination machine. Rebuildable caches are not a substitute for metadata.

## Logging and audit

Default logging level is `warn`. `-v` enables CLI debug details such as phase
timings; `RUST_LOG` supplies an explicit filter. Logs and prompts go to stderr;
results go to stdout. Spinners and colors only appear at terminals.

Audit records API registration changes, base URL/credential-link changes,
credential login/logout and sent requests. Request records contain API,
command, method, URL without query, status/outcome, timing and scheme names.
They exclude credential values, headers, query strings and response bodies.
The log rotates past 10 MiB, retaining one previous file, and is owner-only on
Unix. Audit failures are logged but do not fail the command. It is a local
activity trail, not a remote immutable compliance log.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Application error: invalid spec, body validation, missing credentials, failed selection, etc. |
| 2 | Usage error: unknown command, invalid/missing arguments, ambiguous variant, etc. |
| 3 | HTTP 4xx |
| 4 | HTTP 5xx |
| 5 | Connection failure, timeout or runtime transport failure |

HTTP error bodies still go to stdout; diagnostics go to stderr. Executable
plugins pass through their exit code (subject to the CLI's code conversion),
so their failures need not use the table above. Inspect both status and body in
automation; a JSON response alone does not establish success.

## Supported input and practical limits

| Area | Behavior |
|---|---|
| HTTP methods exposed as operations | GET, POST, PUT, PATCH and DELETE; other methods are outside the current domain model |
| Spec formats | OpenAPI `3.0.x`, `3.1.x`, `3.2.x`; Swagger `2.0`; JSON/YAML UTF-8 |
| Normalization | Newer/older formats adapted into an OpenAPI 3.0-shaped internal model; not every source-format feature becomes a CLI feature |
| Spec size | 20 MiB maximum |
| Spec fetch | 10-second connect timeout, 30-second request timeout, at most five redirects |
| Runtime requests | 10-second connect timeout, configurable total request timeout (default 30 seconds), at most five redirects |
| HTTP response size | 64 MiB maximum; responses are buffered before rendering |
| Multipart file size | 100 MiB maximum per file |
| Generic table output | At most eight columns, cells limited to 60 characters |
| Body validation | Supported schema subset, up to 20 reported problems |
| Response selection | Dotted fields/numeric indices, not full JMESPath |
| Credential links | One level, token-compatible schemes only |

The CLI does not automatically walk pagination links, retry failed business
requests, or poll jobs to completion. Supply the spec's paging flags and make
subsequent calls explicitly. TLS verification stays enabled; credentials are
not sent to non-loopback plain HTTP targets. Server-side authorization and
business validation remain the API's responsibility.
