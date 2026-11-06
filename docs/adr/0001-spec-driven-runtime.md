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

Option 3. `opencli-spec` turns every format into `opencli-core` domain types, and nothing outside `opencli-spec` imports a parser crate. There is one adapter (3.0, via `openapiv3`). The other formats are rewritten into 3.0 shape before reaching it:

- **OpenAPI 3.1** is normalized at the JSON level (`openapi31.rs`), *amended 2026-11-05 (M6)*. The original plan was a second adapter via `oas3`. But 3.1's structure matches 3.0 except for a handful of objects, and a second adapter would have repeated all the `$ref` resolution, security mapping and validation. The normalizer rewrites the schema dialect: `type` arrays become `nullable`, numeric `exclusiveMinimum` and `exclusiveMaximum`, `const`, `examples`, and `prefixItems`. It inlines `components.pathItems` references and drops `webhooks`, `jsonSchemaDialect`, and the license `identifier`. A `mutualTLS` scheme surfaces as unsupported.
- **OpenAPI 3.2** (*added 2026-11-06, M9*) shares 3.1's schema dialect and is otherwise additive. `openapi32.rs` removes what has no command form, then the 3.1 normalizer runs. It removes: the `QUERY` method and `additionalOperations`; `in: querystring` parameters (including `$ref`s to them); the `deviceAuthorization` OAuth flow; and `$self`. Tag hierarchy, server names, streaming `itemSchema` and `oauth2MetadataUrl` are accepted and ignored.
- **Swagger 2.0** is converted (`swagger20.rs`):
  - `host`/`basePath`/`schemes` become servers (HTTPS first; relative without `host`).
  - `body` and `formData` parameters become request bodies.
  - Parameter validation keywords move into schemas, and `collectionFormat` maps to `style`/`explode` (2.0's default `csv` is joined, not repeated).
  - `definitions` and their `$ref`s move to `components.schemas`, and shared `#/parameters/` references are inlined.
  - `securityDefinitions` become security schemes (`application` → client credentials, `accessCode` → authorization code).

## Consequences

- Parser crates can be swapped or upgraded without touching the runtime.
- A contract test (`opencli-spec/tests/contract.rs`) parses the same Petstore written as 3.0, 3.1, 3.2 and 2.0 (YAML and JSON). It requires identical commands, arguments, bodies and security requirements.
- Normalizing 3.1 schemas to 3.0 form loses spelling, not meaning for commands. `--describe` shows `nullable: true` rather than `type: [..., "null"]`, and a schema with several non-null types accepts any value.
- Schemas are kept as normalized JSON Schema, not fully typed, because they're only used for flag generation, `--describe` output and body fields.
- `multipart/form-data` bodies are sent as multipart (M9). Binary fields (`format: binary`, Swagger `type: file`, or a 3.1 `contentMediaType`) become `--<field> <FILE>` flags, read at send time and capped at 100 MiB. Other multipart subtypes are rejected with a clear error.
- OpenAPI 3.2 operations using `QUERY`, custom methods or `querystring` parameters don't appear as commands. Versions after 3.2 are rejected with a message naming the supported ones.
