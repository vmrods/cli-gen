# 0004. Credential storage

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** Project owner

## Context

Secrets must never be stored in plaintext. The OS keyring (Windows Credential Manager, macOS Keychain, Linux Secret Service) is the primary store, but headless Linux machines (servers, containers, CI) usually have no Secret Service.

## Options considered

1. **Keyring only; fail on headless Linux.** Simplest, but unusable on servers.
2. **Keyring, with an encrypted-file fallback.** Works everywhere, at the cost of handling a passphrase.
3. **Environment variables only.** Works in CI, but there's no persistence or token refresh.

## Decision

Option 2, behind one `CredentialStore` interface:

- **Primary:** the `keyring` crate, with entries keyed `opencli/<api>/<security-scheme>`.
- **Fallback:** a single file in the data directory, encrypted with ChaCha20-Poly1305 using a key derived from a passphrase with Argon2id. The passphrase comes from `OPENCLI_PASSPHRASE` or an interactive prompt. The file is restricted to the owner (0600 on Unix, owner-only ACL on Windows).
- The fallback is used only when the keyring is unavailable, or when the user selects it explicitly (`config set credential-store file`). It is never chosen silently once a keyring entry exists.
- Environment variables (`OPENCLI_<API>_TOKEN`, etc.) override both, for CI.

## Consequences

- Losing the passphrase means logging in again. There is no recovery by design.
- OAuth2 refresh tokens and expiry times are stored with the access token, so refreshing is transparent.
- Credentials are keyed by scheme name, not spec version, so they survive spec refreshes.
