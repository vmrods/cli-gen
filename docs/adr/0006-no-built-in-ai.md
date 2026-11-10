# 0006. No built-in AI provider

- **Status:** Accepted; MCP deferral amended by [ADR 0012](0012-local-mcp-adapter.md)
- **Date:** 2026-11-05
- **Deciders:** Project owner

## Context

The PRD proposes an optional `opencli ask "..."` that uses an LLM to pick and run an operation, with the rule that the AI may only call operations defined in imported specs. AI agents that can run shell commands (Claude Code, Codex, Cursor) can already use any CLI.

## Options considered

1. **A built-in LLM client (`opencli ask`).** Needs provider keys, provider-specific code and prompt maintenance.
2. **An MCP server (`opencli mcp serve`).** Works with AI clients that can't run commands, but sends every operation's schema to the agent up front, which is expensive for large APIs.
3. **A CLI designed for agents to use directly.**

## Decision

Option 3. OpenCLI ships no LLM integration. Agents use it like any other CLI, discovering commands step by step with `--help` and `--describe`, and using `--output json`, `--dry-run` and `--yes` ([ADR 0002](0002-command-surface.md)).

## Consequences

- The PRD's rule holds by construction: OpenCLI only runs operations defined in imported specs, whoever calls it. Stopping an agent from making raw HTTP calls is a matter of how the agent's environment is configured, not something OpenCLI controls.
- No AI-related secrets, dependencies or costs.
- If AI clients without shell access become a requirement, an MCP layer can be added as a thin adapter over the same `--describe` metadata.
