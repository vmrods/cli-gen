# 0012. Local MCP adapter with progressive operation discovery

- **Status:** Accepted
- **Date:** 2026-11-09
- **Deciders:** Project owner
- **Amends:** [ADR 0006](0006-no-built-in-ai.md), which anticipated MCP when clients without shell access became a requirement. The decision to ship no model client remains.

## Context

The owner requested Claude and ChatGPT integration. Shell-capable clients can
already use JSON, describe and dry-run commands, but MCP gives clients typed
discovery and execution without shell parsing. Registering thousands of endpoint
tools up front would recreate the context-cost problem identified in ADR 0006.
Credentials, request validation and audit must retain the CLI's existing rules.

## Options considered

1. **Usage skill only.** Small and useful for shell clients, but does not provide a tool protocol.
2. **Shell-wrapper MCP process.** Reuses CLI output but adds subprocess parsing and makes policy depend on argv filtering.
3. **Local stdio MCP adapter in the binary.** Shares the Rust domain/runtime and authentication without a model provider or a network listener.
4. **Hosted HTTP service immediately.** Supports remote chat clients but adds service hosting, independent authorization and credential isolation before the local boundary is proven.

## Decision

Ship a reusable usage skill and option 3. Use the official maintained Rust MCP
SDK, pinned in workspace dependencies with only server/stdio features enabled.
The adapter lives in the binary; domain and parser crates remain independent of
MCP. A shared response-sending function preserves CLI transport and audit.

Expose five fixed tools: list APIs, search operations, describe one operation,
plan a request and execute it. Operation identifiers combine HTTP method and
original spec path, avoiding command-variant ambiguity. Tool inputs are decoded
into strict request types and validated against the chosen registered operation.

Require an explicit API allowlist at startup, with optional exact operation
filters applied to discovery, description, planning and execution. Execution defaults to GET only;
writes additionally require operator opt-in and per-call confirmation after user
authorization. Confirmation is a caller assertion, not proof of a separate
human approval. No model-controlled server override, local file upload,
validation bypass, built-in management operation or sign-in execution is exposed.
Credentials come from the existing environment/provider/store/link path. The
adapter does not save sign-in responses.

Responses are JSON with HTTP status, structured errors, bounded output and
explicit array paging/truncation metadata. Responses and post-response errors
retain HTTP status and a request-sent marker. Writes cannot be replayed to
page their output; truncated write results require a separate read. Known secret-looking fields are
redacted before selection, with header/cookie values omitted from previews.
This redaction is best effort, not a guarantee that an arbitrary API contains no
sensitive business data. Spec and response text remain untrusted model input.

## Consequences

- Claude Code and Codex can launch the same local process. A shared SKILL.md also supports direct CLI use.
- The SDK adds transitive dependencies but no model keys, model calls or managed-service cost. Run the dependency audit after updates.
- `mcp` becomes a reserved API name; existing registrations with that name must use another registration name before this upgrade.
- The server can use and refresh local credentials for allowed requests. API allowlisting scopes access; HTTP method checks cannot prove a poorly designed GET has no side effects.
- Stdout belongs exclusively to MCP. Terminal prompts are unsupported; authenticate outside the server. Blocking OS keyring access retains the existing platform behavior.
- Array offsets page within a returned response, not across server-side pagination. Oversized results fail explicitly instead of returning broken JSON.
- Remote ChatGPT connections still need an HTTP transport or a separate local bridge/tunnel integration. That is deliberately a follow-up, not a public listener hidden in this local feature.
