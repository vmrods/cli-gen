#!/usr/bin/env bash
# Install a packaged OpenCLI into ~/.local/bin (macOS or Linux; no sudo).
# Usage: ./install.sh [--package DIR] [--bin-dir DIR] [--help]
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)"
package=""
bin_dir="${HOME}/.local/bin"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --package) package="${2:?--package needs a directory}"; shift 2 ;;
    --bin-dir) bin_dir="${2:?--bin-dir needs a directory}"; shift 2 ;;
    -h | --help) sed -n '2,3s/^# //p' "$0"; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done

if [[ -z "$package" ]]; then
  if [[ -f "$root/bin/opencli" ]]; then
    package="$root"
  else
    case "$(uname -s)" in
      Darwin) os=mac ;;
      Linux) os=linux ;;
      *) echo "This installer supports macOS and Linux." >&2; exit 1 ;;
    esac
    case "$(uname -m)" in
      arm64 | aarch64) arch=arm64 ;;
      x86_64 | amd64) arch=x86_64 ;;
      *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
    esac
    version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$root/Cargo.toml")"
    package="$root/dist/opencli-${version}-${os}-${arch}"
  fi
fi
if [[ ! -f "$package/bin/opencli" || ! -f "$package/SHA256SUMS" ]]; then
  echo "No complete package at $package. Run ./build.sh first, or pass --package DIR." >&2
  exit 1
fi
package="$(cd -- "$package" && pwd)"
expected="$(awk '$2 == "bin/opencli" { print $1 }' "$package/SHA256SUMS")"
mkdir -p -- "$bin_dir"
bin_dir="$(cd -- "$bin_dir" && pwd)"
if [[ -d "$bin_dir/opencli" ]]; then
  echo "Installation target is a directory: $bin_dir/opencli" >&2; exit 1
fi
# Validate a private copy before replacing the installed executable atomically.
temporary="$(mktemp "$bin_dir/.opencli.XXXXXX")"
trap 'rm -f "$temporary"' EXIT
install -m 755 "$package/bin/opencli" "$temporary"
actual="$(shasum -a 256 "$temporary")"
if [[ ! "$expected" =~ ^[[:xdigit:]]{64}$ || "$expected" != "${actual%% *}" ]]; then
  echo "Package checksum verification failed: $package/bin/opencli" >&2
  exit 1
fi
"$temporary" --version
mv -f "$temporary" "$bin_dir/opencli"
echo "Installed: $bin_dir/opencli"
case ":$PATH:" in
  *":$bin_dir:"*) echo "Run: opencli --help" ;;
  *)
    echo 'Add this line to your shell startup file, then run it in your current terminal:'
    printf "export PATH=%q:\"\$PATH\"\n" "$bin_dir"
    ;;
esac
