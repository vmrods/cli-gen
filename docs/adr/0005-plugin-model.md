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

Option 3. Config-file plugins (a TOML manifest) cover aliases, output templates and saved prompt templates. Executable plugins are found on `PATH` and talk JSON over stdin/stdout for auth extensions and custom commands. Inside OpenCLI, both kinds sit behind the same `Plugin` interface, which is registered when the binary is wired together.

## Consequences

- Plugins can be written in any language, with no shared binary interface and no `unsafe`.
- Executable plugins run with the user's privileges, so installing one means trusting it. `plugin install` shows the plugin's origin and asks for confirmation.
- WebAssembly can be revisited if sandboxed third-party plugins become a requirement.
