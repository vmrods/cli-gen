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
- **Naming order of precedence** for each operation:
  1. The `x-cli-name` / `x-cli-resource` extensions in the spec.
  2. The path and method pattern: GET on a collection → `list`, GET on `/{id}` → `get`, POST → `create`, PUT → `replace`, PATCH → `update`, DELETE → `delete`. Nested paths nest: `/users/{id}/orders` → `users orders list <id>`.
  3. The `operationId` converted to kebab-case.

  Any remaining clash fails the import with a miette error naming both operations. Nothing is silently renamed.
- **Agent-friendly from day one:** `--output json|yaml|table`, `--describe` (prints the operation's parameter and body schema as JSON), `--dry-run`, `--body @file|-`, and `--yes`, which is required for operations other than GET when stdin isn't a terminal. Interactive prompts only appear when stdin is a terminal.

## Consequences

- Command names stay stable across spec refreshes unless the path changes, which keeps scripts working.
- Spec authors control naming through `x-cli-*` without touching OpenCLI.
- Adding a top-level built-in command later can clash with an existing API name. Treat the reserved list as part of the public contract.
