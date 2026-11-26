# Authentication

[Documentation home](README.md) · [User guide](user-guide.md)

A registration, a server URL and a credential are separate things. Importing a
spec discovers security schemes; it does not acquire tokens or authorize you
against the server. Scheme names are the exact keys from the spec, including
case. They are not necessarily called `bearer` or `apiKey`.

## Inspect and choose a scheme

```sh
opencli api show crm
opencli auth status crm
opencli auth login crm
opencli auth login crm --scheme bearer
```

With one supported scheme it can be selected automatically. With several,
choose one when prompted or pass `--scheme`. `auth status` reports the source,
environment variable name and OAuth expiry where available, without showing
credentials. A configured credential can still be expired or rejected by the
server. Status reports plugin/link configuration without proving that its
provider or source will return a usable token.

Supported schemes are API keys in headers/query/cookies, HTTP basic, HTTP bearer
and OAuth 2.0 client credentials or authorization code. Unsupported security
schemes are not made usable just by setting an environment variable.

## Store an existing key or bearer token

In a terminal, let OpenCLI ask for the secret with hidden input:

```sh
opencli auth login crm --scheme bearer
```

For automation, read a secret from stdin, not a command-line argument:

```sh
opencli auth login crm --scheme bearer --with-token < /secure/path/token.txt
```

For HTTP bearer, provide the raw token; OpenCLI adds `Bearer `. An API-key
scheme sends exactly the stored value, even if its header is `Authorization`.
For that kind of API key, include any prefix required by the server. The
[automatic sign-in flow](#use-an-apis-own-sign-in-operation) handles the bearer
prefix for recognized Authorization-header token schemes.

HTTP basic login asks for a username and password. The username may be passed
as a flag, and the password through stdin:

```sh
opencli auth login crm --scheme basic --username ada --with-token < /secure/path/password.txt
```

## Use an API's own sign-in operation

Some APIs expose `POST /api/v1/account/login` rather than an OAuth token
endpoint. With a matching spec, either of these can sign in:

```sh
opencli auth login identity
opencli identity v1 account login post -i
```

The first offers the detected sign-in operation in a terminal when no
`--with-token` is supplied, asks for its fields (secrets hidden), sends the
request and stores the returned token. The second calls the operation directly
and also saves its token. The command path is an example; discover yours with:

```sh
opencli api commands identity login
```

Automatic detection requires a POST whose last fixed path word is `login`,
`signin`, `authenticate`, `token`, or ends in `login`, `signin` or `token`, plus
exactly one token-compatible scheme: HTTP bearer or an API key in the
Authorization header. Tokens are recognized as a nonempty `token`,
`accessToken`, `access_token` or `jwt` string at the top level or beneath `data`
or `result`. Several possible token schemes are not guessed.

The normal API response still goes to stdout, and a save confirmation goes to
stderr. A login response can therefore expose its token in terminal output or
redirected files; saving it does not redact the response body.

On the detected operation, `--no-save` skips credential storage:

```sh
opencli identity v1 account login post --body @login.json --yes --no-save
```

That example assumes a protected file containing the required login fields.
Unsuccessful responses, missing tokens and dry runs do not store a credential.
These convention-based tokens do not get automatic refresh or expiry tracking;
sign in again when the server rejects an expired token.

## Share one sign-in between two APIs

If both APIs accept the same identity token, register both and link the caller
to the token issuer:

```sh
opencli api set-credentials trading --from identity
opencli auth login identity
opencli auth status trading
opencli api show trading
```

`identity` here is the name you registered for your identity API. Both sides
must have exactly one compatible token scheme (HTTP bearer or an
Authorization-header API key). Basic and OAuth credentials cannot be linked.
Only one level is allowed: no self-links, chains or cycles.

The token is read from the source at call time, with its `Bearer ` representation
converted for the destination scheme. Signing in again at the source therefore
updates both. `auth login trading` without an explicit `--scheme` follows the link and
signs in to the source. A target's own stored token is ignored while it is linked.

```sh
opencli api set-credentials trading --clear
```

Clearing the link returns the target to its own credentials. Removing the source
registration also clears the link. To revoke the shared stored token, log out
of the source:

```sh
opencli auth logout identity
```

Logging out of the target does not delete the source's stored token. Environment
credentials remain effective until they are unset; logout removes stored
credentials, not environment variables or server-side sessions.

## OAuth 2.0

Client credentials, with the client secret read from stdin:

```sh
opencli auth login crm --scheme oauth --flow client-credentials \
  --client-id my-app --scope users:read --with-token < /secure/path/client-secret.txt
```

Authorization code with PKCE:

```sh
opencli auth login crm --scheme oauth --flow authorization-code \
  --client-id my-app --no-browser
```

The browser flow binds a loopback callback at `127.0.0.1`, uses PKCE and checks
OAuth state. `--no-browser` prints the sign-in URL instead of launching it.
`--redirect-port <port>` chooses a fixed port if your identity provider requires
one; otherwise an available port is used. Register the appropriate loopback
redirect with your provider. Scope flags are repeatable; when omitted the
flow's declared scopes are used. The interactive callback wait is five minutes.

Stored OAuth access tokens nearing expiry are renewed using a refresh token
for authorization code, or a new client-credentials grant. Renewal is not
available when the stored grant lacks what the provider needs. An OAuth token
supplied via environment/provider is used as-is, with no stored refresh data.

## Environment credentials and precedence

Each scheme has an override named `OPENCLI_<API>_<SCHEME>`, uppercased with
non-alphanumeric characters replaced by underscores. For API `trading`
and scheme `Bearer`, that is `OPENCLI_JCI_TRADING_BEARER`. `auth status` prints
the exact name, so scripts need not guess it.

Use your CI secret injection to populate that variable. Its value is a raw key
or token, or `user:password` for basic auth; it is used for calls and not saved.

For ordinary schemes, the order is:

1. The target API's environment override.
2. Its plugin credential provider.
3. Its stored credential.

For a linked token scheme, the link replaces step 3: source environment first,
then the source's store. Source plugin providers are not followed. A missing
source token gives instructions to sign in there. Within an OpenAPI security
alternative, all schemes are required; alternatives are tried for a satisfied
combination. An anonymous alternative allows a call without credentials when
none is available.

## Credential storage

By default, OpenCLI uses the OS keyring when available: macOS Keychain, Windows
Credential Manager or Linux Secret Service. Otherwise it uses a
passphrase-encrypted `credentials.enc` in its data directory. The file uses
Argon2id key derivation and authenticated encryption; the passphrase is not
stored with it.

To force file storage for one command:

```sh
OPENCLI_CREDENTIAL_STORE=file opencli auth login crm
```

In a terminal, a new file asks you to choose and repeat a passphrase. Subsequent
access asks for it again. For automation, inject `OPENCLI_PASSPHRASE` from your
secret manager. A missing passphrase fails instead of prompting in a pipeline.
`OPENCLI_CREDENTIAL_STORE=keyring` forces keyring use and fails if unavailable.

Changing `OPENCLI_HOME` changes file-backed credential storage, but the OS
keyring is still keyed by API and scheme rather than that directory. For truly
isolated tests, use a temporary home **and** file storage with a test passphrase.

Credentials are refused over plain HTTP except on loopback. There is no switch
to disable TLS certificate verification. See [troubleshooting](troubleshooting.md)
for missing credentials, certificate errors and stalled credential access.
