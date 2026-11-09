# 0010. Save the token a sign-in returns

- **Status:** Accepted
- **Date:** 2026-11-09
- **Deciders:** Project owner

## Context

Many APIs, ASP.NET exports in particular, sign in through one of their own operations (`POST /api/v1/account/login` with email and password) and return a JWT in the response body. They declare it as an API key in the `Authorization` header, so `opencli auth login` asks for a raw key. Users had to call the login command, copy `data.token` out of the response, run `auth login`, and remember to type `Bearer ` in front. The spec says nothing about which operation signs in or where its token is, so OpenCLI can't learn it from the document.

## Options considered

1. **Detect sign-ins.** Recognize the operation and the token by convention. Nothing to set up.
2. **Configure once per API.** `auth setup <api> --login <command> --token <path>`, stored in a new table. Predictable, but a setup step and a migration.
3. **A `[login]` section in a plugin manifest.** No schema change, but a TOML file per API.

## Decision

Option 1.

- **A sign-in command** is a POST whose last fixed path word is `login`, `signin`, `authenticate` or `token`, or ends with `login`, `signin` or `token` (`federated-login`, `refresh-token`), on an API with exactly one scheme a token can fill:
  - an API key in the `Authorization` header: saved as `Bearer <token>`, because the key is sent exactly as stored;
  - an HTTP bearer scheme: saved bare; the runtime adds `Bearer `.

  With several such schemes OpenCLI doesn't guess; `auth login --scheme` stays the way in.
- **The token** is the first non-empty string in `token`, `accessToken`, `access_token` or `jwt`, at the top level or under `data` or `result`.
- **Saving.** After a successful (2xx) sign-in with a token, the token is stored like `auth login` would store it (keyring or encrypted file, ADR 0004) and audited as `auth.login`, never with the token. A line on stderr says so, so stdout keeps only the response. `--no-save` (only on sign-in commands) skips it. Failed sign-ins, responses without a token, and `--dry-run` save nothing.
- **`opencli auth login <api>`** offers the API's own sign-in when it has one for the chosen scheme, in a terminal and without `--with-token`: `Sign in with your account (POST …/login)? [Y/n]`. It asks for every field of that operation (secrets hidden, optional ones skippable), sends it, and saves the token. The best match wins: `login` over `federated-login`.

## Consequences

- One step to sign in: `opencli auth login secops`, or the login command itself.
- Detection can misfire on an API whose `…/token` POST returns a token that isn't for calling the API. The stderr line and `auth status` make that visible, `--no-save` and `auth logout` undo it.
- Refresh tokens are not used yet: when the access token expires, sign in again. Renewing through the API's own refresh operation would need the same conventions plus expiry tracking; it is left for later.
