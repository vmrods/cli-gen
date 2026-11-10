#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
cp "$root/build.sh" "$root/install.sh" "$root/rust-toolchain.toml" "$root/README.md" "$fixture/"
cp -R "$root/skills" "$root/docs" "$fixture/"
mkdir -p "$fixture/tools" "$fixture/src"
cat >"$fixture/Cargo.toml" <<'TOML'
[workspace]
members = ["."]
[workspace.package]
version = "0.1.9"
[package]
name = "opencli"
version.workspace = true
TOML
printf 'fn main() {}\n' >"$fixture/src/main.rs"
cargo_path="$(rustup which --toolchain "$(sed -n 's/^channel = "\(.*\)"/\1/p' "$root/rust-toolchain.toml")" cargo)"
(cd "$fixture" && "$cargo_path" generate-lockfile --offline)
cat >"$fixture/tools/rustup" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  target) exit 0 ;;
  which)
    case "${*: -1}" in
      cargo) echo "$BUILD_TEST_TOOLS/cargo" ;;
      rustc) command -v rustc ;;
    esac ;;
esac
STUB
cat >"$fixture/tools/cargo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" != build ]]; then exec "$BUILD_TEST_CARGO" "$@"; fi
[[ "${BUILD_TEST_FAIL:-0}" != 1 ]] || exit 1
version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
"$BUILD_TEST_CARGO" metadata --locked --offline --no-deps --format-version 1 >/dev/null
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) target="$2"; shift 2 ;;
    --target-dir) build_dir="$2"; shift 2 ;;
    *) shift ;;
  esac
done
mkdir -p "$build_dir/$target/release"
printf '#!/usr/bin/env bash\necho "opencli %s"\n' "$version" >"$build_dir/$target/release/opencli"
chmod +x "$build_dir/$target/release/opencli"
STUB
chmod +x "$fixture/tools/"*
export BUILD_TEST_TOOLS="$fixture/tools" BUILD_TEST_CARGO="$cargo_path"
export PATH="$fixture/tools:$PATH"
cd "$fixture"

bash build.sh --help >/dev/null
[[ "$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)" == 0.1.9 ]]
if bash build.sh --os invalid >/dev/null 2>&1; then exit 1; fi
bash build.sh >build.log 2>&1
rg -q 'opencli 0.1.10' build.log
rg -q 'version = "0.1.10"' Cargo.lock
skill_files=(dist/opencli-0.1.10-*/skills/opencli/SKILL.md)
[[ -f "${skill_files[0]}" ]]
agent_guides=(dist/opencli-0.1.10-*/docs/agents.md)
[[ -f "${agent_guides[0]}" ]]
installers=(dist/opencli-0.1.10-*/install.sh)
bash "${installers[0]}" --bin-dir "$fixture/installed" >/dev/null
[[ "$("$fixture/installed/opencli" --version)" == 'opencli 0.1.10' ]]
bash build.sh >build.log 2>&1
rg -q 'opencli 0.1.11' build.log
cp Cargo.toml Cargo.toml.before
cp Cargo.lock Cargo.lock.before
if BUILD_TEST_FAIL=1 bash build.sh >build.log 2>&1; then exit 1; fi
cmp Cargo.toml Cargo.toml.before
cmp Cargo.lock Cargo.lock.before
[[ -z "$(find dist -maxdepth 1 -name 'opencli-0.1.12-*' -print)" ]]
echo 'Build version tests passed.'
