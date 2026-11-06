# 0008. `sqlx` with hand-written SQL instead of an ORM

- **Status:** Accepted
- **Date:** 2026-11-06
- **Deciders:** Project owner

## Context

The database standards default to an ORM and treat raw SQL as an exception to raise first. This store was built in M1 on `sqlx`, as the PRD specifies, without raising it. The standards list no Rust ORM. The data is a small local SQLite metadata file owned by one crate (`opencli-store`).

## Options considered

1. **SeaORM or Diesel.** Typed query builders and generated models. Both add sizeable dependencies and a second schema description to maintain, and give little back for about 15 straightforward queries.
2. **`sqlx` with literal, parameterized SQL in the store crate.** The PRD's choice. Queries are plain and reviewable, migrations are SQL files, and every query is covered by integration tests against real SQLite.

## Decision

Option 2. SQL lives only in `opencli-store` (the adapter layer, per the standards). Every statement is a literal (`sqlx` 0.9 rejects non-literal SQL), and every value is bound, never concatenated. Shared fragments are composed with `concat!`, not `format!`.

Partial alignment with the rest of the standard:
- `apis` gets `updated_at` alongside `created_at` (migration 0005). Child rows (versions, operations, parameters, schemes, schemas) are immutable once written, so they have no `updated_at`.
- `created_by` doesn't apply to a single-user local tool.
- Existing constraints keep SQLite's default names, because renaming them means rebuilding each table. New constraints use the `pk_`/`fk_`/`uq_`/`ix_` naming rule.

## Consequences

- SQL mistakes surface in tests rather than at compile time. Compile-time-checked `query!` macros with committed `.sqlx/` metadata remain an option without changing the store's interface (ADR 0003).
- The database standard now has a Rust entry, so this isn't an undocumented exception.
