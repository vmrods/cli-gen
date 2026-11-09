# Getting started

[Documentation home](README.md) · Next: [User guide](user-guide.md)

## 1. Install a build

### From the repository on macOS or Linux

Run from the repository root:

```sh
./build.sh
./install.sh
opencli --version
```

Building requires rustup, the toolchain in `rust-toolchain.toml` and the native C
build tools. On macOS, use Xcode Command Line Tools. Linux needs a C compiler.
`build.sh` increments the workspace patch version and creates
`dist/opencli-<version>-<os>-<arch>/`. The installer chooses the package matching
the workspace version and this machine, verifies its binary checksum and runs
`--version` before replacing `~/.local/bin/opencli`.

If a matching package already exists, `./install.sh` is enough. Neither step
requires sudo for the default destination. If the installer prints an `export
PATH=...` line, run it and add it to your shell startup file. It does not edit
that file for you. In zsh, run `rehash` if a previously open terminal does not
find a newly installed command.

To choose another package or destination:

```sh
./install.sh --package ./dist/opencli-0.1.6-mac-arm64
./install.sh --bin-dir "$HOME/.local/bin"
```

The explicit package path is an example; choose a version and platform you have
built. An older package installs its older behavior even when the source tree
has been updated.

### From an unpacked package

On macOS/Linux, run `./install.sh` inside the package. Rust and the repository
are not needed to run the packaged executable. You can also run
`./bin/opencli --help` without installing it.

On Windows, put the package's `bin` directory on your user PATH and open a new
terminal. The executable is `opencli.exe`; the shell installer is for macOS and
Linux. See [packaging](development.md#build-a-package) for platform build options.

## 2. Try the command model without a server

From a repository checkout, use the bundled Petstore fixture with an isolated
home directory. These examples use a POSIX shell and make no API requests.

```sh
export OPENCLI_HOME="$(mktemp -d)"
opencli --server https://example.com api add demo --spec ./fixtures/petstore.yaml
opencli api list
opencli api commands demo
opencli demo pets --help
opencli demo pets --limit 10 --dry-run
opencli demo pets --pet-id 123 --dry-run
opencli demo pets post --name Rex --tag dog --dry-run
opencli demo pets post --describe
unset OPENCLI_HOME
```

`api add` reads and stores the local spec. `--server` answers the onboarding
question up front. `--dry-run` prints the planned request, without sending it or
loading credentials. `--describe` prints the command's variants, arguments and
body schema as JSON. The fixture's server URLs are examples, not live services.

Unsetting `OPENCLI_HOME` returns subsequent commands to your normal data
location. The temporary directory remains on disk until you remove it.

## 3. Register Jci Trading

If `opencli api list` already shows `jci-trading`, skip `api add`; adding the same
name twice is an error. Set its base URL if necessary:

```sh
opencli api set-server jci-trading https://api.trader.joiningclouds.com
```

For a first registration on your Mac:

```sh
opencli --server https://api.trader.joiningclouds.com api add jci-trading \
  --spec "$HOME/Desktop/Jci-trading-api.yaml"
```

The registered name is the command name. The filename and OpenAPI title do not
create alternative names such as `trading-api`.

```sh
opencli api show jci-trading
opencli api commands jci-trading afterhours
opencli jci-trading v1 afterhours --help
opencli jci-trading v1 afterhours --dry-run
```

These path-shaped examples apply to the current source. Older installed builds
used `jci-trading afterhours list`; see [migration](troubleshooting.md#commands-changed-after-an-upgrade)
if your help output differs.

## 4. Authenticate and call the API

```sh
opencli auth status jci-trading
opencli auth login jci-trading
opencli jci-trading v1 afterhours --query data -o table
```

`auth login` asks for a credential according to the spec. For a trading API that
accepts a token issued by a separate identity API, register the identity API and
[link its credentials](authentication.md#share-one-sign-in-between-two-apis)
instead of copying tokens after every login.

For the complete envelope rather than just its `data` field:

```sh
opencli jci-trading v1 afterhours -o json
```

The server must be reachable and the credentials must have permission for the
operation. A successful import or dry run does not verify either condition.

## 5. Upgrade or uninstall

```sh
./build.sh && ./install.sh
opencli --version
```

Each successful packaging run consumes a new patch version, including repeated
runs and builds for other platforms. Build one package at a time. Ordinary
`cargo build` does not bump the version.

Remove the default installation with `rm "$HOME/.local/bin/opencli"`.
Registered APIs and credentials are stored separately and survive installation,
upgrades and removal of the executable. See [data storage](reference.md#data-storage)
for their locations.
