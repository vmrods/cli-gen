# 0001. Specification-driven runtime with an internal domain model

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** Project owner

## Context

The PRD requires commands to come from imported OpenAPI specs without code generation or recompilation. It also requires support for OpenAPI 3.0, 3.1 and Swagger 2.0. The `openapiv3` crate the PRD names only parses 3.0. Most current specs are 3.1, and 3.1 uses a different schema dialect (JSON Schema 2020-12).

## Options considered

1. **Use `openapiv3` types throughout.** Least code. But 3.1 and 2.0 can't be supported without converting them into a 3.0 model, which loses information.
2. **Generate Rust code from each spec.** Gives typed clients, but breaks the core requirement.
3. **One adapter per format, all producing our own domain model.** More code at the edges. The runtime, store and CLI only ever see one model.

## Decision

Option 3. `opencli-spec` has one adapter per format: 3.0 via `openapiv3`, 3.1 via `oas3`, and Swagger 2.0 converted to 3.0 first. Each produces `opencli-core` domain types. Nothing outside `opencli-spec` imports a parser crate.

## Consequences

- Parser crates can be swapped or upgraded without touching the runtime.
- The domain model is ours to keep in step with the formats. Each adapter gets the same contract tests: the same API described in each format must produce the same model.
- Schemas are kept as normalized JSON Schema, not fully typed, since we only need them for flag generation, `--describe` output and body validation.
