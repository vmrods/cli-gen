# 0007. OAuth 2.0 flows without the `oauth2` crate

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** Project owner

## Context

The PRD lists the `oauth2` crate. Its current release (5.0) is built on `reqwest` 0.12, while OpenCLI uses `reqwest` 0.13. Using it would either duplicate the HTTP and TLS stack in the binary, or require writing an adapter for its HTTP-client trait. What we need is small: client credentials, refresh, and authorization code with PKCE, each one form POST to a token endpoint.

## Options considered

1. **`oauth2` with its default `reqwest` 0.12.** Well tested, but two HTTP/TLS stacks in one binary (size, build time, two places for TLS fixes).
2. **`oauth2` with a custom HTTP-client adapter.** One stack, but we would maintain glue to someone else's abstraction anyway.
3. **Implement the grants directly over `reqwest` 0.13.** About 300 lines including PKCE and the loopback redirect. We own the correctness.

## Decision

Option 3, in `opencli-auth::oauth`:

- Grants: `client_credentials`, `refresh_token`, `authorization_code` with PKCE S256 (RFC 7636). The implicit and password flows are not supported, since OAuth 2.1 removes them.
- Client authentication uses `client_secret_post`, which the major providers (Entra ID, Okta, Auth0, Keycloak) accept.
- The browser redirect goes to `http://127.0.0.1:<port>/callback` (RFC 8252 §7.3). Pass `--redirect-port` when a provider needs an exact registered URI. The `state` must match, other paths get a 404, and the wait gives up after five minutes.
- Token and authorization endpoints must be HTTPS (localhost excepted). Redirects from the token endpoint are never followed, so client secrets can't be replayed elsewhere.
- Token responses must be bearer tokens. A numeric-string `expires_in` is accepted, because some providers send one. Tokens within 60 s of expiry are renewed before use.

## Consequences

- One HTTP/TLS stack. The flows are covered by tests against a mock token endpoint, including the RFC 7636 PKCE test vector and a real loopback callback round trip.
- We must track OAuth security guidance ourselves (e.g. OAuth 2.1, and DPoP if a provider requires it).
- Switching to `oauth2` later stays cheap, because only `opencli-auth::oauth` would change.
