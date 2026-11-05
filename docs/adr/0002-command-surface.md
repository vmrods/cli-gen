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
     - On an item (`/{id}`): GET → `get`, PUT → `replace`, PATCH → `update`, DELETE → `delete`.
     - An action: `POST /users/{id}/activate`, with no other operation on that path, becomes `users activate`.
     - `/users/{id}/orders` → `users orders list <user-id>`.
  3. Otherwise (e.g. POST on an item), the `operationId` converted to kebab-case, or the lowercased method if there is none.

  When pattern names collide (e.g. `DELETE /users` and `DELETE /users/{id}`), only the colliding operations switch to their `operationId`. Anything that still collides, or a command name that is also a group of other commands, fails the import with a diagnostic listing every operation involved. Nothing is renamed silently.
- **Arguments:** path parameters are positional, in the order they appear in the path. Query, header and cookie parameters become `--kebab-case` flags, typed from their schema (string, integer, number or boolean; arrays repeat; enums restrict values). A flag whose name clashes with a reserved flag (`help`, `version`, `output`, `body`, `describe`, `dry-run`, `yes`) or with another parameter gets its location as a prefix (`--query-output`). `Accept`, `Content-Type` and `Authorization` headers are ignored, as OpenAPI requires. Scalar top-level properties of a JSON request body also become flags (`customers create --name Victor`), prefixed `body-` if their name is taken. They are optional, because `--body` can supply the whole object.
- **Agent-friendly from day one:** `--output json|table`, plus `--describe` (always JSON). `--describe` prints the operation's arguments, where each one goes in the request, and the body schema with `$ref`s inlined. It works without the operation's required arguments. Also `--dry-run`, `--body <json>|@file|-`, and `--yes`, which is required for operations other than GET when stdin isn't a terminal. Interactive prompts only appear when stdin is a terminal. `opencli help <api> <resource> <verb>` works as well as `--help`.

## Consequences

- Command names stay stable across spec refreshes unless the path changes, which keeps scripts working.
- Spec authors control naming through `x-cli-*` without touching OpenCLI.
- Adding a top-level built-in command later can clash with an existing API name. Treat the reserved list as part of the public contract.
