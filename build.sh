#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'HELP'
Usage: ./build.sh [--os mac|linux|windows] [--help]

Build a release folder for this computer under dist/opencli-<os>-<arch>/.
The OS defaults to this computer's OS. Build each OS on a matching host;
on Windows, run this script with Git Bash and the MSVC Rust toolchain.
Requires the pinned Rust toolchain and the platform's native build tools.
HELP
}

fail() {
    printf 'Error: %s\n' "$1" >&2
    exit 1
}

requested_os=''
while [[ $# -gt 0 ]]; do
    case "$1" in
        --os)
            [[ $# -ge 2 && -n "$2" ]] || fail '--os requires mac, linux, or windows.'
            requested_os="$2"
            case "$requested_os" in
                mac|linux|windows) ;;
                *) fail "Unsupported OS: $requested_os. Use mac, linux, or windows." ;;
            esac
            shift 2
            ;;
        --help|-h) usage; exit 0 ;;
        *) fail "Unknown argument: $1. Use --help for usage." ;;
    esac
done

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$project_dir"
command -v cargo >/dev/null 2>&1 || fail 'cargo is required. Install the pinned Rust toolchain.'
command -v rustc >/dev/null 2>&1 || fail 'rustc is required. Install the pinned Rust toolchain.'
host_target="$(rustc -vV | sed -n 's/^host: //p')"
case "$host_target" in
    *-apple-darwin) host_os=mac ;;
    *-unknown-linux-gnu) host_os=linux ;;
    *-pc-windows-msvc) host_os=windows ;;
    *) fail "Unsupported Rust host: $host_target. Use macOS, GNU/Linux, or Windows MSVC." ;;
esac
requested_os="${requested_os:-$host_os}"
[[ "$requested_os" == "$host_os" ]] || fail "Build --os $requested_os on a $requested_os computer; this Rust host is $host_os."

architecture="${host_target%%-*}"
case "$architecture" in
    aarch64) architecture=arm64 ;;
    x86_64) architecture=x64 ;;
esac
binary_name=opencli
if [[ "$host_os" == windows ]]; then
    binary_name=opencli.exe
fi

# An explicit target and directory keep Cargo environment overrides from
# selecting a different platform or hiding the binary from the packaging step.
build_dir="$project_dir/target/package"
package_dir="$project_dir/dist/opencli-$host_os-$architecture"
cargo build --locked --release --package opencli --bin opencli \
    --target "$host_target" --target-dir "$build_dir"
mkdir -p "$package_dir"
cp "$build_dir/$host_target/release/$binary_name" "$package_dir/$binary_name"
cp "$project_dir/README.md" "$package_dir/README.md"
printf '\nBuilt: %s\nRun: "%s" --help\n' "$package_dir" "$package_dir/$binary_name"
