#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
package="$fixture/package with spaces"
bin_dir="$fixture/installed tools"
mkdir -p "$package/bin"
cp "$root/install.sh" "$package/"
write_binary() {
  printf '#!/usr/bin/env bash\nset -euo pipefail\necho "opencli %s"\n' "$1" >"$package/bin/opencli"
  (cd "$package" && shasum -a 256 bin/opencli >SHA256SUMS)
}
write_binary 0.1.2
bash "$package/install.sh" --help >/dev/null
[[ ! -e "$bin_dir" ]]
if bash "$package/install.sh" --unknown >"$fixture/error.log" 2>&1; then exit 1; fi
bash "$package/install.sh" --bin-dir "$bin_dir" >"$fixture/install.log"
[[ "$("$bin_dir/opencli" --version)" == 'opencli 0.1.2' ]]
rg -q 'export PATH=' "$fixture/install.log"
write_binary 0.1.3
PATH="$bin_dir:$PATH" bash "$package/install.sh" --bin-dir "$bin_dir" >"$fixture/install.log"
[[ "$("$bin_dir/opencli" --version)" == 'opencli 0.1.3' ]]
if rg -q 'export PATH=' "$fixture/install.log"; then exit 1; fi

printf 'tampered\n' >>"$package/bin/opencli"
if bash "$package/install.sh" --bin-dir "$bin_dir" >"$fixture/error.log" 2>&1; then exit 1; fi
rg -q 'checksum' "$fixture/error.log"
[[ "$("$bin_dir/opencli" --version)" == 'opencli 0.1.3' ]]
write_binary 0.1.4
printf '#!/usr/bin/env bash\nset -euo pipefail\nexit 1\n' >"$package/bin/opencli"
(cd "$package" && shasum -a 256 bin/opencli >SHA256SUMS)
if bash "$package/install.sh" --bin-dir "$bin_dir" >"$fixture/error.log" 2>&1; then exit 1; fi
[[ "$("$bin_dir/opencli" --version)" == 'opencli 0.1.3' ]]
[[ -z "$(find "$bin_dir" -name '.opencli.*' -print)" ]]

write_binary 0.1.5
bash "$root/install.sh" --package "$package" --bin-dir "$bin_dir" >/dev/null
[[ "$("$bin_dir/opencli" --version)" == 'opencli 0.1.5' ]]
mkdir -p "$fixture/repo"
cp "$root/install.sh" "$fixture/repo/"
printf '[workspace.package]\nversion = "0.1.5"\n' >"$fixture/repo/Cargo.toml"
case "$(uname -s)" in Darwin) os=mac ;; Linux) os=linux ;; esac
case "$(uname -m)" in arm64 | aarch64) arch=arm64 ;; *) arch=x86_64 ;; esac
mkdir -p "$fixture/repo/dist"
cp -R "$package" "$fixture/repo/dist/opencli-0.1.5-$os-$arch"
bash "$fixture/repo/install.sh" --bin-dir "$bin_dir" >/dev/null
[[ "$("$bin_dir/opencli" --version)" == 'opencli 0.1.5' ]]
rm -rf "$fixture/repo/dist"
if bash "$fixture/repo/install.sh" --bin-dir "$bin_dir" >"$fixture/error.log" 2>&1; then exit 1; fi
rg -q 'build.sh' "$fixture/error.log"
echo 'Installer tests passed.'
