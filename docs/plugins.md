# Plugins and shell integration

[Documentation home](README.md) · [Reference](reference.md)

## Manifest plugins

A TOML manifest can add aliases, table columns and credential providers to one
registered API. It is local configuration; installing it does not add endpoints
to the OpenAPI document or compile code.

For the bundled Petstore registration named `demo`, save this as
`petstore-helpers.toml`:

```toml
name = "petstore-helpers"
version = "1.0.0"
description = "Petstore shortcuts and columns"
api = "demo"

[aliases.few]
command = "pets"
args = ["--limit", "5"]
description = "Read up to five pets"

[formats."pets"]
columns = ["id", "name", { path = "tag", header = "TYPE" }]
```

Install and inspect it:

```sh
opencli plugin install ./petstore-helpers.toml
opencli plugin list
opencli demo few --dry-run
opencli demo few -o json
opencli plugin remove petstore-helpers --yes
```

Installation displays aliases, formats and every configured program before
confirmation. Use `--yes` for noninteractive installation. Manifest sources can
be local files or allowed spec-source URLs. The installed copy lives in
`plugins/<name>.toml` under the data directory. Unknown manifest fields and
invalid names/paths are rejected.

### Aliases

`opencli demo few ...` expands to `opencli demo pets --limit 5 ...`. An alias is
one kebab-case word at the API root. Its target uses command words **inside**
the API, without `opencli` or the API name. The target must match the current
command surface; old `pets list` aliases need updating after the naming change.

A real root command takes precedence over an alias. If several installed
plugins define the same alias, the first plugin by name wins. Arguments are
passed as argument values rather than evaluated as shell text. Strings such as
`currentUser()` are sent literally; they are not interpreted by OpenCLI.

### Table columns

Format keys are the complete command path within the API, for example `pets`,
`v1 countries` or `v1 users post`. Each column is a dotted path, optionally with
an explicit header. Paths can include array indices (`labels.0`). Without a
header, the final path segment is uppercased. Missing values are blank.

Arrays produce one row per item; another JSON value produces one row. For an
envelope, use `--query data` first to select the array. Plugin columns operate
on that selected value. `-o json` returns the selected JSON instead of applying
the table layout. Generic table truncation rules do not define a plugin's
custom column selection.

## Credential providers

Add a provider to a manifest only when a program on your machine can supply the
named security scheme's credential. For a scheme named `bearer`:

```toml
[credentials.bearer]
exec = "company-token"
args = ["--profile", "work"]
```

`exec` must be a bare program name on PATH or an absolute path; paths such as
`./company-token` are refused because they depend on the caller's directory.
Providers execute directly, never through a shell. They receive no stdin and
must exit before the command's timeout.

The program must print one JSON object containing a nonempty string `value`:

```json
{"value":"<credential-from-your-secret-store>"}
```

That shape is a protocol example, not a real credential. For bearer/OAuth the
value is an access token, for an API key the exact key value, and for basic auth
`user:password`. Use stderr for non-secret diagnostics; OpenCLI may include the
first bounded stderr line when reporting a provider failure. Do not put
credentials in provider arguments or diagnostic output.

Child environment:

| Variable | Value |
|---|---|
| `OPENCLI_API` | Registered API name |
| `OPENCLI_SCHEME` | Spec security scheme name |
| `OPENCLI_SCHEME_TYPE` | Scheme kind such as `http-bearer` |

The target's environment credential wins over its provider. A provider wins
over linked/stored credentials; it is called when a request needs that scheme.
`auth status` describes providers without running them. Providers are not
followed through the source of a credential link. See [authentication](authentication.md).

## Executable plugins

An executable named `opencli-report` on PATH can be called as:

```sh
opencli report --help
```

A registered API named `report` takes precedence. The process receives the
remaining arguments, the terminal streams and `OPENCLI_BINARY`, which points
to the invoking CLI executable. It can call that path for built-in operations.
Its exit status is passed back. No manifest install is needed for this form;
`plugin list` discovers executables on PATH. Manage the executable's installation
and removal outside the manifest commands.

## Shell completion

Generate completions after registering the APIs you use. They are snapshots of
built-ins and those API command trees. Regenerate after add, refresh, remove or
an upgrade that changes names. The shell must also be configured to load the
resulting file; writing a script alone does not enable completion.

For zsh:

```sh
mkdir -p "$HOME/.zfunc"
opencli completion zsh > "$HOME/.zfunc/_opencli"
```

Add these lines to `~/.zshrc` before or as part of your completion setup, avoiding
a second `compinit` if your framework already runs it:

```sh
fpath=("$HOME/.zfunc" $fpath)
autoload -Uz compinit
compinit
```

For bash with a bash-completion setup that loads user completions:

```sh
mkdir -p "$HOME/.local/share/bash-completion/completions"
opencli completion bash > "$HOME/.local/share/bash-completion/completions/opencli"
```

For fish:

```sh
mkdir -p "$HOME/.config/fish/completions"
opencli completion fish > "$HOME/.config/fish/completions/opencli.fish"
```

For a PowerShell session:

```powershell
opencli completion powershell | Out-String | Invoke-Expression
```

Put an equivalent command in your PowerShell profile if you want it on every
session. `opencli completion elvish` prints an Elvish script for that shell's
configuration. Completion generation reads local registrations, not live API
responses.
