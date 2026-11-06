# 0005. Plugin model

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** Project owner

## Context

The PRD wants installable plugins for command aliases, response formatting, auth extensions and prompt templates. Rust has no stable binary interface between separately compiled code, so loading compiled plugins at runtime (`libloading`) is fragile and unsafe.

## Options considered

1. **Compiled dynamic libraries (`cdylib`).** Fast, but break whenever the compiler version changes and need `unsafe`.
2. **WebAssembly (wasmtime / extism).** Sandboxed and portable, but a heavy runtime and a complex host interface.
3. **Config-file plugins plus `opencli-<name>` executables** (the git and kubectl model).

## Decision

Option 3, *refined when implemented in M8 (2026-11-06)*:

- **Manifest plugins** (`plugin.toml`, installed into `$OPENCLI_HOME/plugins/<name>.toml`, `0600`). Each extends one API (`api = "…"`) with:
  - `[aliases.<word>]`, with a `command` and optional `args`: `opencli <api> <word> [more…]` expands to the command before parsing. An alias never shadows a real command; if one would, it's ignored with a warning. Aliases are listed in `opencli <api> --help`.
  - `[formats."<command path>"]`, with `columns` given as JSON paths (`fields.status.name`, optionally `{ path, header }`). This replaces the generic table for that command's JSON responses. `-o json` is unaffected.
  - `[credentials.<scheme>]`, with `exec` and optional `args`: a **credential provider**. This is the auth extension point. It runs only when the operation needs that scheme and `OPENCLI_<API>_<SCHEME>` isn't set, so the precedence is environment, then provider, then store. The program gets `OPENCLI_API`, `OPENCLI_SCHEME` and `OPENCLI_SCHEME_TYPE` and no stdin, and must print `{"value": "…"}` within the request timeout. The value means the same as the environment variable.
  - Unknown keys are rejected, and every validation problem is reported at once. Programs must be bare names on `PATH` or absolute paths, and are run directly, never through a shell.
- **Executable plugins:** an `opencli-<name>` executable on `PATH` (kebab-case names only) runs as `opencli <name> …` with the terminal attached. It receives the arguments after `<name>` plus `OPENCLI_BINARY` (the running `opencli`), and its exit code is passed through. A registered API with the same name always wins.
- **Lifecycle:** `opencli plugin install <file|url> [--yes]` shows the plugin's origin, aliases, formats and every program it would run, then asks for confirmation (`--yes` is required when not in a terminal). `plugin list` shows manifests and executables. `plugin remove <name> [--yes]`. Installs and removals are audited.
- **Not built:**
  - AI prompt templates, because there's no AI layer to use them (ADR 0006).
  - A `Plugin` trait. Manifest plugins are the only in-process kind, and nothing needs a substitute in tests.
  - A plugin registry. `plugin install jira` by bare name would need one.

## Consequences

- Plugins can be written in any language, with no shared binary interface and no `unsafe`.
- Installing a manifest with credential providers, or putting an executable on `PATH`, means trusting that program with your privileges. Install output names every such program, and `auth status` shows `plugin <name>` as a scheme's source without running anything.
- Aliases change only the command line, never the request, so `--dry-run` and `--describe` work on them as on the commands they expand to.
- WebAssembly can be revisited if sandboxed third-party plugins become a requirement.
