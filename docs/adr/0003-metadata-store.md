# 0003. Metadata store and command cache

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** Project owner

## Context

Imported specs, their versions and the generated command mappings must persist between runs. A CLI is started on every command, so startup cost matters.

## Options considered

1. **SQLite via `sqlx`** (named in the PRD). Async, with migrations built in. Compile-time-checked queries need a database at build time or prepared query metadata committed to the repo.
2. **SQLite via `rusqlite`.** Synchronous, with a lighter dependency tree.
3. **JSON files only.** No schema or migrations, and version history is awkward.

## Decision

SQLite via `sqlx` (option 1). The database is the system of record for APIs, versions, operations, parameters, schemas and security schemes. We use `sqlx::migrate!` for migrations and keep the prepared-query metadata in `.sqlx/` so builds and CI don't need a database. A command-tree cache file (one per API, keyed by a hash of the spec version) sits next to the database so commands can be dispatched without querying SQLite.

## Consequences

- The cache is derived data. It is invalidated by a hash mismatch and is always safe to delete.
- After editing queries, `cargo sqlx prepare` has to be run, and CI checks the result with `cargo sqlx prepare --check`.
- `tokio` is required from startup. That's acceptable because reqwest needs it anyway.
