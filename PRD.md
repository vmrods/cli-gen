You are a Principal Rust Architect and Staff\-Level Platform Engineer.

Design and implement a production\-ready Rust application named OpenCLI.

# Objective

Build a cross\-platform CLI that can dynamically consume OpenAPI (Swagger) specifications and automatically expose API operations as CLI commands without requiring recompilation whenever a new OpenAPI specification is imported.

The platform should behave similarly to:

- AWS CLI
- Azure CLI
- kubectl
- GitHub CLI

except that commands are generated from imported OpenAPI specifications.

# Core Requirements

## OpenAPI Support

Support:

- OpenAPI 3.0
- OpenAPI 3.1
- Swagger 2.0 where reasonably possible

Accepted sources:

- Local swagger.json files
- Local swagger.yaml files
- Remote URLs
- Versioned specifications

Examples:

opencli import petstore.json

opencli import [https://api.company.com/swagger/v1/swagger.json](https://api.company.com/swagger/v1/swagger.json)

opencli import crm\-api.yaml

The system must validate specifications during import.

Invalid specifications must produce meaningful validation errors.

---

## Dynamic Command Generation

After importing a specification like:

GET /users

GET /users/\{id\}

POST /users

DELETE /users/\{id\}

the CLI should automatically expose:

opencli users list

opencli users get 123

opencli users create

opencli users delete 123

Operations should be discovered dynamically from the OpenAPI specification.

No source code generation should be required for normal usage.

---

## Metadata Store

Design a metadata store containing:

- APIs
- versions
- paths
- operations
- parameters
- schemas
- security requirements

Use:

SQLite

through:

sqlx

The metadata database should allow:

opencli specs list

opencli specs show crm\-api

opencli specs versions crm\-api

---

## Authentication

Automatically discover security definitions.

Support:

- OAuth2 Client Credentials
- OAuth2 Authorization Code
- Bearer Token
- API Key
- Basic Authentication

When credentials are required:

opencli auth login crm\-api

Examples:

Client ID: Client Secret:

Store credentials securely using:

- Windows Credential Manager
- macOS Keychain
- Linux Secret Service

using Rust keyring integration.

Never store secrets in plaintext.

---

## Runtime API Execution Layer

Use:

reqwest

for HTTP communication.

Support:

- GET
- POST
- PUT
- PATCH
- DELETE

Automatically:

- resolve path parameters
- resolve query parameters
- resolve headers
- serialize JSON request bodies
- deserialize JSON responses

Examples:

opencli customers get 100

opencli customers create \-\-name "Victor" \-\-email "victor@example.com"

---

## Import and Refresh Process

Implement:

opencli import api.json

opencli refresh crm\-api

opencli remove crm\-api

When a new version of a Swagger file is imported:

- detect changes
- update metadata
- regenerate command mappings
- preserve stored credentials whenever possible

No recompilation should be necessary.

---

## AI Assistant Layer (Optional Plugin)

Create an extensible AI plugin layer.

Example:

opencli ask "show all open tickets assigned to me"

Process:

1. AI analyzes user request.
2. AI inspects imported OpenAPI metadata.
3. AI chooses the correct operation.
4. Runtime executes the API call.
5. Result is returned.

The AI must NOT generate arbitrary HTTP calls.

It may only invoke operations defined in imported OpenAPI specifications.

This constraint is mandatory.

---

## Plugin Architecture

Support plugins.

Examples:

opencli plugin install jira

opencli plugin install salesforce

opencli plugin install github

Plugin capabilities:

- custom command aliases
- response formatting
- authentication extensions
- AI prompt templates

Design using Rust traits and dependency injection patterns.

---

## User Experience

Provide a modern CLI experience.

Use:

- clap
- clap\_complete
- miette
- indicatif

Requirements:

- shell completion
- colored output
- progress indicators
- rich error messages
- help system

Examples:

opencli help users get

opencli users get \-\-help

---

## Caching

Implement command metadata caching.

Requirements:

- persistent cache
- cache invalidation
- specification version tracking
- startup optimization

Use:

SQLite and local cache files.

---

## Observability

Use:

- tracing
- tracing\-subscriber

Provide:

- debug logging
- audit logging
- performance metrics

Support:

RUST\_LOG=debug

---

## Security

Follow:

OWASP API Security Top 10

Requirements:

- secret isolation
- token expiration handling
- credential rotation
- TLS verification
- request timeouts
- secure local storage

Never disable certificate validation by default.

---

## Testing

Create:

- unit tests
- integration tests
- OpenAPI parser tests
- authentication tests
- command\-generation tests

Target minimum:

80% coverage.

Use:

cargo test

and GitHub Actions CI.

---

## Architecture Deliverables

Produce:

1. Overall architecture diagram
2. Cargo workspace structure
3. Rust project layout
4. Domain model design
5. Database schema
6. Sequence diagrams
7. Core Rust traits
8. Example implementations
9. Security review
10. MVP roadmap
11. Enterprise roadmap
12. Complete build plan

---

## Technology Stack

Language: Rust stable

Key crates:

- clap
- reqwest
- tokio
- serde
- serde\_json
- serde\_yaml
- openapiv3
- sqlx
- keyring
- oauth2
- tracing
- miette
- indicatif

Database:

SQLite

Platforms:

- Linux
- macOS
- Windows

---

# Important Design Constraint

The system must be specification\-driven, not code\-generated.

The imported OpenAPI document is the source of truth.

The runtime should interpret specifications dynamically and execute API operations without requiring Rust code regeneration or recompilation after importing a new specification.

Generate the complete architecture, Rust workspace structure, implementation plan, database schema, trait definitions, sample code, and MVP roadmap needed to build this system.


