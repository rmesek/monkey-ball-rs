#!/usr/bin/env bash
# Build the browser version into web/pkg/ (git-ignored). Usage:  scripts/build-web.sh [--dev]
#   needs: rustup target add wasm32-unknown-unknown
#          cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock> --locked
# Then serve it:  cargo run -p xtask -- serve-web
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

fail() { echo "build-web: $*" >&2; exit 1; }

# The wasm-bindgen CLI must be exactly the version of the wasm-bindgen crate the build links.
want="$(tr -d '\r' < Cargo.lock | awk '/^name = "wasm-bindgen"$/ { getline; gsub(/version = |"/, ""); print; exit }')"
[ -n "$want" ] || fail "wasm-bindgen is not in Cargo.lock"
command -v wasm-bindgen >/dev/null 2>&1 \
  || fail "wasm-bindgen CLI not found. Install it with: cargo install wasm-bindgen-cli --version $want --locked"
have="$(wasm-bindgen --version | awk '{ print $2 }')"
[ "$have" = "$want" ] \
  || fail "wasm-bindgen CLI is $have but the crate is $want. Run: cargo install wasm-bindgen-cli --version $want --locked"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
# The `web` profile (workspace Cargo.toml) is release plus fat LTO, one codegen unit and panic=abort.
profile=web
flags=(--profile web)
if [ "${1:-}" = "--dev" ]; then
  profile=debug
  flags=()
fi

cargo build -p mb-web --target wasm32-unknown-unknown --target-dir "$target_dir" ${flags[@]+"${flags[@]}"}

out="$root/web/pkg"
rm -rf "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
  "$target_dir/wasm32-unknown-unknown/$profile/mb_web.wasm"

size="$(wc -c < "$out/mb_web_bg.wasm")"
echo "built web/pkg ($((size / 1024)) KiB wasm). Serve it with: cargo run -p xtask -- serve-web"
