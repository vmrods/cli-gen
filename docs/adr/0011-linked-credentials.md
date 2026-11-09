# 0011. Linked credentials: one API uses another's token

- **Status:** Accepted
- **Date:** 2026-11-09
- **Deciders:** Project owner

## Context

Some APIs accept tokens issued by another: the Jci Trading API takes the JWTs the Jci Identity API's login returns. Credentials are stored per API and scheme (ADR 0004), so the same token had to be stored twice, in two formats (the identity spec sends `Bearer <token>` as an API key; the trading spec declares HTTP bearer and wants the bare token), and signing in again meant updating both.

## Options considered

1. **Copy the token** into the second API whenever the first signs in. Two stored secrets that drift, and logging out of one leaves the other.
2. **Link the APIs**: the second reads the first's stored credential at call time. One secret, one sign-in, one logout.

## Decision

Option 2.

- `opencli api set-credentials <api> --from <other>` links, `--clear` unlinks. Stored as `apis.credentials_from`, a nullable reference to the other API (migration 0007, `ON DELETE SET NULL`): removing the other API ends the link, and the linked API's cache is rebuilt. Linking is audited (`api.set-credentials`) and rebuilds the API's cache (ADR 0003).
- **What it covers:** token schemes only, an `Authorization` API key or HTTP bearer, and each side must have exactly one. The token is converted between them: `Bearer ` is stripped from an API key and added back where the target expects it.
- **Precedence** for the linked API's token scheme: its environment variable, then a plugin provider, then the other API's credential (its environment variable, then its store). The linked API's own stored credential is ignored while the link exists. With no credential on the other side, calls fail with the command to sign in there.
- **One level:** an API can't link to itself, to an API that is itself linked, or take credentials from another while others link to it. No chains, no cycles.
- **Signing in:** `auth login <linked>` signs in to the other API instead, says so, and stores the credential there. `auth status` shows `from <other>`; `api show` shows `Credentials: from <other>`.

## Consequences

- One `opencli auth login secops` serves both APIs; signing out of it signs out of both.
- The link is local setup, like the base URL: it isn't part of either spec and survives refreshes.
- OAuth 2.0 and basic credentials can't be shared this way; nothing so far needs it.
