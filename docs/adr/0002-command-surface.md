# 0002. Command surface: API name first

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** Project owner

## Context

The PRD shows `opencli users list`. That clashes as soon as two imported APIs both have a `users` resource. Commands also need predictable names, derived from specs whose `operationId`s are often missing or unreadable.

## Options considered

1. **A flat namespace across all APIs.** Short commands, but clashes are unavoidable and resolving them is surprising.
2. **API name first: `opencli <api> <resource...> <verb>`.** One extra word per command, and never ambiguous.

## Decision

Option 2.

- **Management commands** live under `api`: `opencli api add <name> [--spec <file|url>]` (asks for the spec when `--spec` is missing and stdin is a terminal), plus `api list | show | versions | refresh | remove`.
- **Reserved top-level names:** `api`, `auth`, `plugin`, `completion`, `help`, `config`. `api add` rejects an API name that matches one of these or isn't kebab-case.
- **Naming**, for each operation:
  1. The `x-cli-name` (verb) and `x-cli-resource` (string of words, or an array) extensions in the spec. Names set this way are never renamed.
  2. Otherwise, the method and path pattern. Leading `api`, `rest` and `vN` segments that every path shares are stripped. Static segments become resource words, and path parameters are skipped:
     - On a collection: GET → `list`, POST → `create`.
     - On an item (`/{id}`): GET → `get`, PUT → `replace`, PATCH → `update`, DELETE → `delete`, POST → `update` (Stripe-style APIs update with POST).
     - An **action**: a POST under an item, alone at its path, with nothing beneath it. `POST /users/{id}/activate` becomes `users activate`. If the segment has items (`POST …/check-runs` beside `GET …/check-runs/{id}`), it's a collection and the POST is `create`.
     - A **query**: a GET alone at its path, with nothing beneath it, under a static parent. `GET /customers/search` becomes `customers search`, and `GET /search/code` becomes `search code`. Collections with items or writes keep `list`.
     - `/users/{id}/orders` → `users orders list <user-id>`.

     "Alone at its path" and "nothing beneath it" compare paths with parameter names erased, because specs name the same position differently (`{user_id}`, `{username}`).
  3. When names collide, or a command name is also a group of other commands, the operations involved fall back in two steps. First a **short** operationId verb: the part after the last `/` (GitHub's `tag/verb-noun`), minus leading words that repeat the resource words. If that still collides, the **full** operationId is used. Hint-named operations are never renamed.

  Anything that still collides fails the import with a diagnostic listing every operation involved. Nothing is renamed silently.

  *Tuned 2026-11-06 (M10) against GitHub's REST description (1,261 operations) and Stripe's (612). Both failed to import under the original rules. Both now import with no conflicts and dispatch in about 15 ms.*
- **Arguments:** path parameters are positional, in the order they appear in the path. Query, header and cookie parameters become `--kebab-case` flags, typed from their schema (string, integer, number or boolean; arrays repeat; enums restrict values). A flag whose name clashes with a reserved flag (`help`, `version`, `output`, `query`, `server`, `timeout`, `body`, `describe`, `dry-run`, `yes`, `interactive`, `no-validate`) or with another parameter gets its location as a prefix (`--query-output`). `Accept`, `Content-Type` and `Authorization` headers are ignored, as OpenAPI requires. Scalar top-level properties of a JSON request body also become flags (`customers create --name Victor`), prefixed `body-` if their name is taken. They are optional, because `--body` can supply the whole object. Secret fields (`format: password`, or string fields named like a password, secret, API key or token; *added 2026-11-09*) never become flags, so secrets stay out of shell history and process lists, matching the rule for `auth`.
- **Agent-friendly from day one:** `--output json|table`, plus `--describe` (always JSON). `--describe` prints the operation's arguments, where each one goes in the request, and the body schema with `$ref`s inlined. It works without the operation's required arguments. Also `--dry-run`, `--body <json>|@file|-`, and `--yes`, which is required for operations other than GET when stdin isn't a terminal. Interactive prompts only appear when stdin is a terminal. `opencli help <api> <resource> <verb>` works as well as `--help`.
- **Filling in inputs** (*added 2026-11-09*). In a terminal, required arguments and body fields left out are asked for, instead of failing. `-i`/`--interactive` asks for every argument and body field, optional ones skippable with Enter. Prompts walk nested objects and lists of objects, show types and allowed values, re-ask on invalid values, and read secret fields hidden. Shapes a prompt can't walk (`oneOf`, untyped) take a JSON value. Without a terminal nothing is asked: clap's usage errors (exit 2) stand, and `--interactive` is refused. A body read from stdin (`--body -`) also turns questions off.
- **Body validation** (*added 2026-11-09*). JSON bodies are checked against the operation's schema after flags and prompts are merged, before sending and in `--dry-run`. The check covers types, `nullable`, `enum`, `required` (except read-only fields), nested properties and array items, and `allOf`/`anyOf`/`oneOf`. Unknown fields and string formats are allowed: the server stays the judge of the rest. Every problem is listed with its path (`body.address.city`), up to 20, and exits 1; `--no-validate` sends the body anyway. It is a small built-in checker, not a JSON Schema library, because OpenAPI 3.0 schemas differ from JSON Schema (`nullable`) and the subset requests need is small.
- **Base URL at onboarding** (*added 2026-11-06*). `api add` decides where requests go.
  - In a terminal, it lists the spec's usable servers and asks. Enter accepts the first, a number picks another, or you type any URL.
  - Elsewhere, the global `--server` answers up front. Without it, the spec's first usable server is used, and if there isn't one (for example, a file-imported spec with only relative servers), the import fails with a hint rather than every call failing later.
  - Accepting the spec's default stores nothing, so the API keeps following the spec across refreshes. A chosen URL is stored with the API (`apis.base_url`), kept across refreshes, changed with `api set-server <api> <URL>` or `--clear`, shown by `api show`, and audited.
- **Sending:** GET runs immediately. Operations that change data (POST, PUT, PATCH, DELETE) ask for confirmation in a terminal and require `--yes` otherwise, so an agent or script never changes data by accident. JSON responses follow `--output`: in table mode, arrays of objects become columns of their scalar fields, and objects become field/value rows. Text and binary bodies are written as received. The base URL comes from a per-call `--server`, then the API's stored base URL, then the spec's first server. A relative server (or none, which OpenAPI treats as `/`) resolves against the spec's source URL, and specs imported from a file need `--server` in that case. `--timeout <seconds>` defaults to 30.
- **Response field selection** (*added 2026-11-07*). `--query <path>` selects a field from successful JSON API responses before table/JSON rendering or plugin columns. Dot-separated field names and numeric array indices are supported (`data.items.0`). This is a field path, not JMESPath. Missing fields and non-JSON success responses fail explicitly; HTTP error bodies and exit codes remain intact. Built-in management commands, `--dry-run` and `--describe` reject selection. A spec parameter named `query` uses its location prefix, e.g. `--query-query`.
- **Exit codes** (part of the public contract):

  | Code | Meaning |
  |---|---|
  | 0 | Success (HTTP 2xx) |
  | 1 | Error (invalid spec, missing confirmation, bad input, …) |
  | 2 | Usage error (unknown command, missing or invalid argument) |
  | 3 | HTTP 4xx. The response body is still printed to stdout. |
  | 4 | HTTP 5xx. The response body is still printed to stdout. |
  | 5 | Network failure or timeout |

## Consequences

- Command names stay stable across spec refreshes unless the path changes, which keeps scripts working.
- Spec authors control naming through `x-cli-*` without touching OpenCLI.
- Adding a top-level built-in command later can clash with an existing API name. Treat the reserved list as part of the public contract.
