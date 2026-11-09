# OpenCLI documentation

OpenCLI reads an OpenAPI document and turns its operations into commands at
runtime. An API registration connects a local name, a specification and a server.
The CLI plans and sends requests; the API still owns its data and behavior.

These guides describe the current source tree. Check `opencli --version` before
following examples: older installed builds can expose different command names.
In particular, the current command surface follows URL paths and HTTP methods,
rather than the older `list`, `get` and `create` verbs.

## Choose a starting point

| What you want to do | Read |
|---|---|
| Install, try the CLI without calling a server, then connect Jci Trading | [Getting started](getting-started.md) |
| Find operations, pass arguments, send bodies and shape output | [User guide](user-guide.md) |
| Sign in, use OAuth, share tokens between APIs or automate credentials | [Authentication](authentication.md) |
| Look up commands, flags, environment variables, files and limits | [Reference](reference.md) |
| Add shortcuts, custom table columns or credential providers | [Plugins](plugins.md) |
| Diagnose a failed command or migrate from an older build | [Troubleshooting](troubleshooting.md) |
| Follow a request through the code, build, test or extend the app | [Development guide](development.md) |
| Understand crate boundaries and design decisions | [Architecture](architecture.md) and [ADRs](architecture.md#decisions) |

## A typical session

```sh
opencli api list
opencli api commands jci-trading afterhours
opencli jci-trading v1 afterhours --help
opencli jci-trading v1 afterhours --dry-run
opencli auth login jci-trading
opencli jci-trading v1 afterhours --query data -o table
```

The Jci commands assume that API has already been registered from a spec whose
path is `/api/v1/afterhours`. Use `api commands` and `--help` to discover the
actual names in your spec. Registering an API does not sign in, and importing a
spec does not call its business operations.

For exact arguments available in your installed version, use `opencli --help`,
`opencli api --help`, or an operation's `--help`. For a structured operation
contract, use `--describe`.
