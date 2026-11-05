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

Option 2, behind one `CredentialStore` interface (defined in `opencli-auth`, with a fake in tests):

- **Primary:** the OS keyring through `keyring` 4 (`v1` feature): Keychain, Credential Manager or Secret Service. The service is `opencli` and the account is `<api>/<security-scheme>`. The value is the credential as JSON.
- **Fallback:** `credentials.enc` in the data directory. It holds one JSON map of all credentials, encrypted with ChaCha20-Poly1305 under a key derived from a passphrase with Argon2id (19 MiB, 2 iterations, 1 lane, per OWASP). There's a fresh nonce on every write, and the format version and KDF parameters are bound as associated data, so tampering with either fails decryption. Writes are atomic. On Unix the file is `0600`. On Windows it inherits the user profile's owner-only ACL.
- **Passphrase:** `OPENCLI_PASSPHRASE`, or a hidden prompt (confirmed twice when the file is created). It's requested lazily, so commands that need no credentials never prompt.
- **Choosing a store:** the keyring when `keyring` reports a usable store, otherwise the file. `OPENCLI_CREDENTIAL_STORE=keyring|file` forces the choice. Forcing `keyring` without one available is an error.
- **Environment overrides,** for CI and agents: `OPENCLI_<API>_<SCHEME>`, upper-cased with other characters as `_` (e.g. `OPENCLI_CRM_APIKEY`). The value is the API key, the token, or `username:password` for basic auth. It's used as-is and never stored. `opencli auth status <api>` shows each scheme's variable name.
- **Input:** secrets come from a hidden prompt or from stdin (`auth login --with-token`), never from command-line arguments, which other users can read in the process list.

## Consequences

- Losing the passphrase means logging in again. There is no recovery by design.
- OAuth2 refresh tokens, client secrets (for client credentials) and expiry times are stored with the access token, so renewal is transparent. Refreshed tokens are written back after the request is planned. If that write fails, it's only logged, and the token is refreshed again next time.
- Credentials are keyed by scheme name, not spec version, so they survive spec refreshes. A stored credential whose type no longer matches the scheme (e.g. it changed from API key to OAuth2) is ignored with a warning.
- `api remove` also deletes the API's credentials, as a best effort.
- `--dry-run` never reads credentials, so it never prompts and never refreshes tokens.
- Credentials are refused over plain HTTP to anything but localhost. That covers the API and its token and authorization endpoints.
- The real keyring backends aren't exercised in CI, because CI machines have no unlocked keyring. Tests use the file store or a fake. The keyring adapter is small, and is checked by hand on each platform before release.
- On Linux, `keyring`'s Secret Service store depends on `zbus` 5, whose split-out crates RustSec flags as unmaintained (they were merged into `zbus` 6). Those advisories are ignored in `deny.toml` until `secret-service` upgrades.
