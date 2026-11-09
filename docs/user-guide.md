# User guide

[Documentation home](README.md) · [Command reference](reference.md)

Examples use `demo` for the bundled Petstore spec. Commands for your own API are
derived from its paths, parameters and extension hints; discover them with help.

## Register and inspect an API

```sh
opencli api add demo --spec ./fixtures/petstore.yaml
opencli api list
opencli api list -o json
opencli api show demo
opencli api commands demo
opencli api commands demo pets post
```

In a terminal, `api add` asks for a spec if omitted and for the base URL. It
lists the spec's usable servers: accept the first, choose a listed number, or
enter a URL. Without a terminal, provide `--spec`; the first usable spec server
is used unless `--server` is supplied. A file with no usable absolute server
needs an explicit server.

Names use lowercase letters and digits in hyphen-separated words, start with a
letter, and have at most 64 characters. Built-in names such as `api` and `auth`
are reserved. Specs can be JSON or YAML, from a local file or HTTPS URL.
Localhost HTTP is also allowed for spec fetching. The original source is saved
for refresh; local sources are stored as absolute paths.

`api commands` searches command names, HTTP methods, endpoint paths, summaries
and operationIds. Every search word must match, ignoring case. Its results
include endpoint and path flags. This is useful when the API has thousands of
operations and you only know a word such as `login` or `countries`.

## Understand generated command names

The shape is:

```text
opencli <registered-api> <fixed-path-segments> [method] [--flags ...]
```

Leading `api`, `apis` and `rest` path segments are stripped. Version segments
stay. Words are converted to kebab case. GET has no method word; writes use
`post`, `put`, `patch` or `delete`.

| API endpoint | Command after `opencli crm` |
|---|---|
| `GET /api/v1/users` | `v1 users` |
| `GET /api/v1/users/{userId}` | `v1 users --user-id 123` |
| `POST /api/v1/users` | `v1 users post` |
| `PUT /api/v1/users/{userId}` | `v1 users put --user-id 123` |
| `DELETE /api/v1/users/{userId}` | `v1 users delete --user-id 123` |
| `GET /api/v1/users/{userId}/orders` | `v1 users orders --user-id 123` |

The collection and item GETs can be variants of the same command. Path flags
select the variant. An exact match wins; otherwise a uniquely compatible
variant can ask for missing flags in a terminal. Ambiguous variants require a
more precise set of flags or interactive selection with `-i`.

An operation only accepts flags belonging to its selected variant. A resource
can be readable itself and also have subcommands. Always inspect its help:

```sh
opencli demo --help
opencli demo pets --help
opencli demo pets post --help
opencli demo pets post --describe
```

`--describe` returns a JSON object with a `command` and `variants` array. Each
variant contains its operation contract, including arguments and body schema.
It requires neither credentials nor complete request arguments.

Spec authors can set `x-cli-resource` to replace path words and `x-cli-name` to
replace the method word (or add a word to GET). Naming conflicts fail import
rather than silently selecting an operation. See [ADR 0009](adr/0009-path-shaped-commands.md).

## Pass arguments

Path, query, header and cookie parameters are all named flags. A spec parameter
`petId` becomes `--pet-id`; a `limit` integer becomes `--limit 10`. Booleans take
values such as `--completed true`. Arrays use repeated flags. Enum choices and
basic value types are checked before sending.

```sh
opencli demo pets --limit 10
opencli demo pets --pet-id 123
```

When names collide, the location is used as a prefix. A request parameter named
`query` can become `--query-query` because `--query` selects response output.
A body field can become `--body-name` if `--name` is already taken. Help is the
source of truth for those names. `Authorization`, `Accept` and `Content-Type`
header parameters are handled by auth and request rendering, rather than
ordinary spec-derived header flags.

## Supply a request body

For simple top-level scalar fields:

```sh
opencli demo pets post --name Rex --tag dog --yes
```

For nested structures or a whole document:

```sh
opencli demo pets post --body '{"name":"Rex","tag":"dog"}' --yes
opencli demo pets post --body @pet.json --yes
opencli demo pets post --body @pet.json --name Rex --yes
printf '%s' '{"name":"Rex"}' | opencli demo pets post --body - --yes
```

Body-field flags override fields supplied through `--body`. Schemas are checked
for supported types, required fields (excluding read-only fields), enum values,
nested properties and arrays. This is not a complete implementation of every
JSON Schema validation keyword. Errors identify paths such as
`body.address.city`; up to 20 body problems are reported at once.

Use `--describe` to see what the command expects. `--no-validate` bypasses body
schema checks when the server intentionally accepts something outside the spec;
it does not bypass argument parsing or turn malformed JSON into valid input.

```sh
opencli demo pets post --body @pet.json --no-validate --dry-run
```

Secret fields are excluded from command-line body flags. `format: password`
and string field names resembling passwords, secrets, API keys, private keys
or tokens trigger this behavior. Supply them through a hidden terminal prompt,
a protected body file or stdin. A body file and the response of a sign-in may
contain secrets: manage those files and redirected outputs accordingly.

### Interactive input

```sh
opencli demo pets post -i
```

In a terminal, missing required values are prompted for automatically. `-i` or
`--interactive` asks for optional values too and can walk nested objects and
lists. Provided values are retained. Secrets use hidden prompts. An empty
answer can skip an optional value.

When stdin is redirected or piped, there are no prompts. Provide all required
inputs and `--yes` for writes. Explicit `--interactive` fails without a terminal.
Reading the body from stdin also makes the request noninteractive.

### Form, multipart and text operations

These depend on the operation's declared media type. For a spec with the
corresponding paths and fields, examples would be:

```sh
opencli crm session post --username ada --dry-run
opencli crm uploads post --file ./report.csv --description Report --yes
opencli crm notes post --body @note.txt --yes
```

The first is form-encoded if the spec declares a form body. Multipart file
fields become file-path flags; OpenCLI builds the multipart boundaries and
reads file contents when sending. `--body @file` reads a whole body, whereas a
multipart `--file ./report.csv` supplies an upload field. These are different
inputs. Text operations send a text body. Discover the exact fields and media
type through that operation's help and contract.

## Preview requests and confirm writes

```sh
opencli demo pets post --name Rex --dry-run -o json
opencli demo pets delete --pet-id 123 --dry-run
```

A dry run validates and prints the plan without authorizing or sending it. It
does not verify server availability, permission, certificate trust or actual
HTTP responses. Any body you provide can appear in the plan; treat plans with
secret body fields as sensitive output.

GET runs immediately. Other methods need a confirmation in a terminal or
`--yes` when noninteractive. When a write has a collection variant and an item
variant, omitting item path flags can select the collection. Its confirmation
labels that scope `(all items)`. Preview before approving a broad write.

## Select a server

The server choice is, in order:

1. Per-call `--server`.
2. The API's saved base URL.
3. The spec's first server (relative servers resolve against a URL spec source).

```sh
opencli api set-server demo https://staging.example.com
opencli --server https://production.example.com demo pets --dry-run
opencli api set-server demo --clear
```

A file-imported spec with relative servers needs an absolute base URL. Server
variables use their spec defaults. A server override changes the request
destination, not the spec's paths or the credential identity. Do not append an
endpoint path to a base URL if it would duplicate the path in the spec.

## Choose output

```sh
opencli demo pets -o json
opencli jci-trading v1 afterhours --query data -o table
opencli jci-trading v1 afterhours --query data.0 --output json
```

Table is the default. Arrays of objects become rows with scalar columns;
objects become `FIELD`/`VALUE` rows. Generic tables cap columns at eight and
truncate cells after 60 characters, so JSON is the complete representation.
An empty array prints `No results.`. Text and binary responses are written as
received; selecting `-o json` does not convert them into JSON.

`--query` selects a field before JSON/table rendering and before plugin columns.
It supports dotted object paths and numeric array indices (`data.items.0.id`),
not JMESPath filters, projections, wildcards or property names containing dots.
A missing path or non-JSON success response fails explicitly. HTTP errors retain
their full response instead of trying to select a success-only field. `--query`
is for API responses and cannot be combined with `--dry-run` or `--describe`.

## Scripting and automation

Use JSON for results consumed by other programs. Redirect stderr separately if
you need diagnostic logs, and check the process exit code even when stdout is
valid JSON. For an already registered and authenticated Jci Trading API:

```sh
#!/usr/bin/env bash
set -euo pipefail

if opencli jci-trading v1 afterhours --query data -o json >afterhours.json; then
  printf 'Saved response to afterhours.json\n'
else
  cli_exit=$?
  printf 'OpenCLI failed with exit code %s\n' "$cli_exit" >&2
  exit "$cli_exit"
fi
```

On an HTTP error, the output file can contain the server's error body rather
than the expected data. On an earlier failure it may be empty. Only process it
as success data after checking the exit code. Use `--yes` for writes in CI and
provide all required arguments; piped stdin never triggers interactive prompts.

Inject credentials using the per-scheme variable shown by `auth status` or a
configured credential provider. Do not put token values in command arguments.
Use `OPENCLI_HOME` to select a job-specific registration directory, but also
choose file-backed credentials if that job must not access the OS keyring.
Import its spec with explicit `--spec` and `--server` so onboarding needs no
terminal. Exit code meanings and environment variables are in the
[reference](reference.md).

There is no built-in pagination walker or retry loop. Pass page/cursor parameters
defined by the API and handle further pages in your script. Retry a write only
when the API's contract makes it safe, using its idempotency mechanism where
available.

## Refresh, inspect versions and remove

```sh
opencli api refresh demo --dry-run
opencli api refresh demo
opencli api refresh demo --spec ./petstore-v2.yaml
opencli api versions demo
opencli api remove demo --yes
```

Refresh compares commands and security changes before storing a new version.
Identical content leaves the active version alone. Previously imported content
reactivates its existing version. A new `--spec` becomes the refresh source.
Saved server and credential links survive refresh; credentials for schemes
removed by the new spec are deleted. There is no dedicated version rollback
command: re-import an older document to reactivate its content.

Removing a registration deletes its stored spec versions, cache and credentials.
It does not delete server-side API resources or the original spec file. Links
from other registrations to the removed API are cleared. Regenerate completion
scripts after changing registrations or command surfaces.
