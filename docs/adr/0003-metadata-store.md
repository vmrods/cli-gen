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

SQLite via `sqlx` (option 1). The database is the system of record for APIs, versions, operations, parameters, schemas and security schemes. We use `sqlx::migrate!` for migrations and runtime-checked queries (`sqlx::query*`, literal SQL only, with bound parameters). Integration tests run every query against a real SQLite file, so we don't commit `.sqlx/` metadata or need `sqlx-cli` in CI. Tables use `STRICT` typing. Nested values (request bodies, security requirements, JSON Schemas) are stored as JSON columns, because nothing queries inside them. A command-tree cache file (one per API, keyed by a hash of the spec version) sits next to the database so commands can be dispatched without querying SQLite.

## Consequences

- The cache is derived data. It is invalidated by a hash mismatch and is always safe to delete.
- SQL mistakes surface in tests, not at compile time. Every store method needs an integration test.
- If the schema grows complex, we can switch to compile-time-checked `query!` macros (with `.sqlx/` committed) without changing the interface.
- `tokio` is required from startup. That's acceptable because reqwest needs it anyway.
