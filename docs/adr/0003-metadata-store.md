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

SQLite via `sqlx` (option 1). The database is the system of record for APIs, versions, operations, parameters, schemas and security schemes. We use `sqlx::migrate!` for migrations and runtime-checked queries (`sqlx::query*`, literal SQL only, with bound parameters). Integration tests run every query against a real SQLite file, so we don't commit `.sqlx/` metadata or need `sqlx-cli` in CI. Tables use `STRICT` typing. Nested values (request bodies, security requirements, JSON Schemas) are stored as JSON columns, because nothing queries inside them. A command-tree cache file per API (`cache/<api>.json`) sits next to the database, so `opencli <api> …` can dispatch without opening SQLite. `api add` writes it and `api remove` deletes it. It's rebuilt from the store when it's missing, can't be read, or was written by a different cache format or OpenCLI version. Only the API named on the command line is loaded.

## Consequences

- The cache is derived data and always safe to delete. Every write to the store for an API (add, remove, and refresh in M5) must also write or delete that API's cache file.
- Measured with a release build on Apple Silicon: Petstore dispatches in about 6 ms. A generated 2,000-operation spec dispatches in 16 ms from cache and 32 ms on a cold rebuild, both within the 50 ms target.
- The current version is explicit (`apis.current_version_id`, migration 0003), not "the newest row". `spec_versions` keeps `UNIQUE (api_id, content_hash)`, so refreshing back to earlier content reactivates that version instead of storing a duplicate. Version numbers count from 1 in import order.
- Concurrent first runs race to apply migrations. `Store::open` retries, and each retry sees the other process's committed migrations.
- SQL mistakes surface in tests, not at compile time. Every store method needs an integration test.
- If the schema grows complex, we can switch to compile-time-checked `query!` macros (with `.sqlx/` committed) without changing the interface.
- `tokio` is required from startup. That's acceptable because reqwest needs it anyway.
