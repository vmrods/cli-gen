#!/usr/bin/env bash
# Builds a ready-to-use OpenCLI folder: dist/opencli-<version>-<os>-<arch>/
#
#   ./build.sh                      # this computer's OS and architecture
#   ./build.sh --os linux           # Linux, built in Docker (rust image)
#   ./build.sh --os windows         # Windows x86_64, cross-built in Docker (MinGW)
#   ./build.sh --os mac --arch x86_64 --out /tmp/release
#
# Options: --os mac|linux|windows   --arch arm64|x86_64   --out DIR   --help
set -euo pipefail

cd "$(dirname "$0")"
TOOLCHAIN="$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
readonly TOOLCHAIN
readonly RUST_IMAGE="rust:${TOOLCHAIN}-bookworm"

host_os() {
  case "$(uname -s)" in
    Darwin) echo mac ;;
    Linux) echo linux ;;
    MINGW* | MSYS* | CYGWIN*) echo windows ;;
    *) echo "unsupported host: $(uname -s)" >&2; exit 1 ;;
  esac
}

host_arch() {
  case "$(uname -m)" in
    arm64 | aarch64) echo arm64 ;;
    x86_64 | amd64) echo x86_64 ;;
    *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
  esac
}

usage() { sed -n '2,10s/^# \{0,1\}//p' "$0"; }

os="$(host_os)"
arch=""
out="dist"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --os) os="${2:?--os needs mac, linux or windows}"; shift 2 ;;
    --arch) arch="${2:?--arch needs arm64 or x86_64}"; shift 2 ;;
    --out) out="${2:?--out needs a directory}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done
[[ "$os" =~ ^(mac|linux|windows)$ ]] || { echo "--os must be mac, linux or windows" >&2; exit 2; }
# Default: this computer's architecture, except Windows, which is x86_64 only.
if [[ -z "$arch" ]]; then
  if [[ "$os" == windows ]]; then arch=x86_64; else arch="$(host_arch)"; fi
fi
[[ "$arch" =~ ^(arm64|x86_64)$ ]] || { echo "--arch must be arm64 or x86_64" >&2; exit 2; }
[[ "$os" == windows && "$arch" == arm64 ]] && { echo "Windows builds are x86_64 only" >&2; exit 2; }

rust_arch="${arch/arm64/aarch64}"
binary="opencli"
case "$os" in
  mac) target="${rust_arch}-apple-darwin" ;;
  linux) target="${rust_arch}-unknown-linux-gnu" ;;
  # Native Windows builds use MSVC (Git Bash + Visual Studio Build Tools);
  # cross-builds from other hosts use MinGW in Docker.
  windows) target="x86_64-pc-windows-gnu"; binary="opencli.exe" ;;
esac

native=false
if [[ "$os" == "$(host_os)" ]]; then native=true; fi
# Linux and Windows builds run in Docker unless built on their own OS; check
# before anything is written, so a failed start leaves no half-made folder.
if [[ "$native" == false && "$os" != mac ]]; then
  command -v docker >/dev/null || { echo "Docker is required for --os $os on this computer" >&2; exit 1; }
  docker info >/dev/null 2>&1 || { echo "Docker is installed but not running: start Docker Desktop and retry" >&2; exit 1; }
fi
if [[ "$native" == false && "$os" == mac ]]; then
  echo "macOS builds need a Mac: run ./build.sh --os mac on one" >&2
  exit 1
fi

# Reserve the next patch version; restore both manifests if packaging fails.
[[ "$VERSION" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || {
  echo "workspace version must be major.minor.patch" >&2; exit 1;
}
VERSION="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.$((10#${BASH_REMATCH[3]} + 1))"
readonly VERSION
command -v rustup >/dev/null || { echo "rustup is required (https://rustup.rs)" >&2; exit 1; }
cargo="$(rustup which --toolchain "$TOOLCHAIN" cargo)"
backup="$(mktemp -d)"
cp Cargo.toml Cargo.lock "$backup/"
destination=""
remove_partial() {
  local status=$?
  if [[ $status -ne 0 ]]; then
    cp "$backup/Cargo.toml" Cargo.toml
    cp "$backup/Cargo.lock" Cargo.lock
    if [[ -n "$destination" ]]; then rm -rf "$destination"; fi
  fi
  rm -rf "$backup"
  exit "$status"
}
trap remove_partial EXIT
awk -v version="$VERSION" '
  /^\[/ { workspace = ($0 == "[workspace.package]") }
  workspace && /^version = / { $0 = "version = \"" version "\"" }
  { print }
' Cargo.toml >"$backup/next.toml"
cat "$backup/next.toml" >Cargo.toml
"$cargo" update --workspace --offline

package="opencli-${VERSION}-${os}-${arch}"
destination="${out}/${package}"
rm -rf "$destination"
mkdir -p "$destination/bin"
destination="$(cd "$destination" && pwd)"
echo "Building ${package} ..."

# Linux and Windows builds run in Docker with the pinned toolchain, so they
# work from any host; mac builds need a Mac. Build output is kept in named
# volumes so repeat builds are incremental.
in_docker() {
  local platform="$1"; shift
  docker run --rm --platform "$platform" \
    -v "$PWD":/src:ro -v "$destination":/out \
    -v "opencli-build-${os}-${arch}:/target" -v "opencli-build-cargo:/usr/local/cargo/registry" \
    -e CARGO_TARGET_DIR=/target -w /src "$RUST_IMAGE" bash -c "$*"
}

if [[ "$native" == true ]]; then
  [[ "$os" == windows ]] && target="x86_64-pc-windows-msvc"
  rustup target add --toolchain "$TOOLCHAIN" "$target" >/dev/null
  # Use the pinned toolchain's own tools: a cargo/rustc earlier on PATH (e.g.
  # Homebrew's) has no standard library for other targets. A fixed target dir
  # keeps a CARGO_TARGET_DIR in the environment from hiding the binary.
  build_dir="$PWD/target/package"
  RUSTC="$(rustup which --toolchain "$TOOLCHAIN" rustc)" \
    "$(rustup which --toolchain "$TOOLCHAIN" cargo)" build --release --locked -p opencli \
    --target "$target" --target-dir "$build_dir"
  cp "${build_dir}/${target}/release/${binary}" "$destination/bin/"
elif [[ "$os" == linux ]]; then
  in_docker "linux/${arch/x86_64/amd64}" \
    "cargo build --release --locked -p opencli && cp /target/release/opencli /out/bin/"
else
  in_docker "linux/amd64" \
    "apt-get update -qq && apt-get install -y -qq gcc-mingw-w64-x86-64 >/dev/null \
     && rustup target add ${target} >/dev/null \
     && cargo build --release --locked -p opencli --target ${target} \
     && cp /target/${target}/release/opencli.exe /out/bin/"
fi

cp README.md install.sh "$destination/"
cp -R skills docs "$destination/"
cat >"$destination/INSTALL.txt" <<EOF
OpenCLI ${VERSION} for ${os} (${arch})

1. macOS/Linux: ./install.sh (installs into ~/.local/bin); Windows: put bin/ on PATH.
   Or run directly: bin/${binary} --help
2. Register an API:    opencli api add <name> --spec <file-or-url>
3. Shell completion:   opencli completion <bash|zsh|fish|powershell> (re-run after adding APIs)
Data lives in your platform's data directory, or in \$OPENCLI_HOME if set.
EOF
(cd "$destination" && shasum -a 256 "bin/${binary}" >SHA256SUMS)

# Prove the binary starts, where this computer can run it.
if [[ "$os" == "$(host_os)" && "$arch" == "$(host_arch)" ]]; then
  "$destination/bin/${binary}" --version
fi
echo "Done: ${destination}"
