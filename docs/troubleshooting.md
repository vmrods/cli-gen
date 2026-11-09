# Troubleshooting

[Documentation home](README.md) · [Reference](reference.md)

## Start with these checks

```sh
opencli --version
opencli api list
opencli api show jci-trading
opencli api commands jci-trading afterhours
opencli auth status jci-trading
```

Then preview the discovered operation with `--dry-run`. A dry run checks local
inputs and request planning, not networking or authentication. Use `-v` for
phase timings when diagnosing a real call. Avoid publishing sign-in responses,
secret body files or dry-run bodies that contain secrets.

## The command is not found

On macOS/Linux, run `./install.sh` from the repository or unpacked package. If
needed, add the install directory to your shell PATH:

```sh
export PATH="$HOME/.local/bin:$PATH"
command -v opencli
opencli --version
```

Persist that export in your shell startup file. Open a fresh terminal or run
`rehash` in zsh after installation. On Windows, add the package's `bin`
directory to user PATH, then open a new terminal.

If you can run `./dist/.../bin/opencli` but plain `opencli` behaves differently,
there may be multiple versions installed. In a POSIX shell, `type -a opencli`
lists resolutions. Compare their `--version` output and install the intended
package. `./install.sh` selects the current workspace version, not the most
recently modified directory in `dist/`.

## Commands changed after an upgrade

The current source follows [ADR 0009](adr/0009-path-shaped-commands.md). It keeps
version path segments, omits a GET verb and uses flags for path parameters:

| Older command style | Current style for the corresponding spec path |
|---|---|
| `demo pets list` | `demo pets` |
| `demo pets get 123` | `demo pets --pet-id 123` |
| `demo pets create --name Rex` | `demo pets post --name Rex` |
| `jci-trading afterhours list` | `jci-trading v1 afterhours` |

The exact parameter name comes from the spec (`petId` in the bundled example).
Use `api commands <api>` and `--help` instead of mechanically replacing words.
Existing registrations and credentials remain; incompatible command caches
rebuild. Update scripts, plugin aliases/format keys and completion files too.
`--describe` now wraps operations in a `variants` array.

## An API or resource is unrecognized

Use the registered name from `api list`. Registering `jci-trading` does not
create a `trading-api` alias. Generated command words are kebab case, so
`afterhours` is different from `AfterHours`.

A leading `v1` can be part of the command because it is part of the URL. Search
for the operation before guessing its structure:

```sh
opencli api commands jci-trading afterhours
opencli jci-trading v1 afterhours --help
```

Also check `OPENCLI_HOME`: the directory used by one terminal or CI job may be
different from the one where you registered the API.

## A required flag is missing or a variant is ambiguous

Collection and item URLs can share one command. Path flags select the variant.
Check all variants in `--help` or `--describe`, then supply the right path flags.
Use `-i` in a terminal if you want interactive variant selection and input.
A query parameter may have a location prefix when its name clashes with a path
flag or a built-in flag; a request parameter `query` may be `--query-query`.

There are no positional path values in the current command model. A value such
as `123` needs its flag, for example `--id 123`.

## The response shows a `data` JSON blob instead of rows

Select the array before rendering:

```sh
opencli jci-trading v1 afterhours --query data -o table
```

If you need every field without truncation, use `--query data -o json`.
Use `--output`, with two leading dashes, or `-o`. Generic tables show at most
eight scalar columns and shorten long cells. For chosen columns and nested
fields, use a [plugin format](plugins.md#table-columns).

A selection error means the successful response lacks the path or is not JSON.
Repeat without `--query` and inspect JSON. The server may use `items`,
`result.data`, or a different envelope. `--query` is a dotted field path, not
JMESPath; `data[*].id` and filters are unsupported. It does not apply to
management commands, dry runs or describe output.

## Authentication fails with missing credentials, 401 or 403

```sh
opencli auth status jci-trading
opencli api show jci-trading
opencli auth login jci-trading
```

Check the exact scheme name and environment override shown by status. A stale
environment value overrides a newly saved credential. A plugin provider can
override the store as well. For a linked API, sign in to the source; logging out
of the target does not remove the source token. If the link no longer fits,
clear it with `api set-credentials <api> --clear`.

HTTP bearer takes a raw token, whereas an API-key Authorization header sends
exactly its stored string and can require `Bearer `. Automatic sign-in and
credential links convert these forms. Other manual inputs must match the
scheme. Convention-based sign-in tokens are not automatically refreshed;
OAuth grants have their separate refresh handling.

A 401 usually means the server did not accept the credentials. A 403 may mean
they were accepted but lack permission; investigate the server's response and
identity configuration. OpenCLI cannot grant access on the server.

## Credential access stalls or the file passphrase fails

Keyring access is separate from the HTTP request and may need OS approval,
an unlocked keychain or an available Linux Secret Service session. A request
`--timeout` is not a blanket timeout for blocking OS keyring interaction.
Check OS dialogs and keyring availability if the command stalls before sending.

You can deliberately use file storage for subsequent commands:

```sh
OPENCLI_CREDENTIAL_STORE=file opencli auth login jci-trading
OPENCLI_CREDENTIAL_STORE=file opencli jci-trading v1 afterhours --query data -o table
```

This selects a different credential store; it does not migrate existing keyring
secrets. An existing encrypted file needs its original passphrase. For CI,
inject `OPENCLI_PASSPHRASE`; without stdin being a terminal there is no password
prompt. If you cannot recover a passphrase, preserve a backup before resetting
file credentials and signing in again. The CLI cannot recover the passphrase.

## Secret body fields have no flags, or body validation fails

Password/token-like body fields are intentionally absent from flags. Use a
hidden prompt, protected JSON file or stdin. `--describe` gives the schema.
Validation errors identify fields and expected types; fix all listed problems.
Scalar flags can override corresponding top-level fields from `--body`.

Only use `--no-validate` when the API really accepts a body that the spec does
not describe correctly. It bypasses supported body checks, not JSON parsing,
authentication or the server's validation. Noninteractive writes also require
`--yes`.

## A call uses localhost or the wrong API path

Check the saved and spec servers with `api show`, then inspect a dry run:

```sh
opencli api set-server jci-trading https://api.trader.joiningclouds.com
opencli jci-trading v1 afterhours --dry-run
```

The command's version words are path segments, not part of the registered API
name. Do not put a full endpoint into the base URL if the spec already includes
it. A per-call `--server` overrides the saved URL; `set-server --clear` returns
to following the spec.

## TLS, connection and timeout failures

Confirm the server URL and reachability from the machine running the CLI.
`--timeout 60` can increase runtime request time, but does not change spec-fetch
limits or blocking credential-store behavior. Credentials are refused over
non-loopback plain HTTP. Spec URLs themselves must be HTTPS or localhost HTTP.
There is no insecure-TLS flag: fix certificate trust or the server certificate.
An oversized response also fails transport handling instead of rendering a
partial response; use the API's paging/filter parameters to reduce it.

## Refresh or import fails

The original file may have moved, the URL may need access the loader does not
provide, or the spec may be invalid. Point refresh to a new source explicitly:

```sh
opencli api refresh jci-trading --spec ./Jci-trading-api.yaml --dry-run
```

Fetch protected documents through your approved tooling and import a local
copy; the spec loader has no login-header configuration. Resolve syntax and
semantic diagnostics, unsupported format versions, or naming conflicts before
refreshing without `--dry-run`. Name collisions may need `x-cli-name` or
`x-cli-resource` hints in the spec. Import support does not mean every OpenAPI
or JSON Schema feature has a corresponding runtime CLI feature.

## A package is missing or its checksum fails

`./install.sh` expects a complete package for the current workspace version and
host platform. Run `./build.sh`, or pass the directory explicitly with
`--package`. A cross-platform package will not run on this machine; choose a
compatible build. The installer checks the staged binary before replacement,
so a checksum or startup failure preserves the installed executable. Rebuild
or obtain an intact package rather than bypassing verification.

See [development](development.md#build-a-package) for Docker prerequisites and
native build requirements. Failed builds restore the previous version and
lockfile; successful repeated runs each create another patch version.

## Completion or a plugin still uses old commands

Regenerate completion after spec changes and upgrades, and ensure your shell
loads it. Update aliases and format keys to the discovered command paths.
For a credential provider, inspect its exit code and protocol: it must print a
nonempty JSON `value` to stdout before timeout. Its executable must be on PATH
or given as an absolute path. Keep secrets out of diagnostic output.

When reporting a problem, include the executable version, operating system,
redacted command, exit code and relevant diagnostics. Check body/response data
for credentials before sharing it.
