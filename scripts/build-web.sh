#!/usr/bin/env bash
# Build the browser bundle into web/dist/ (git-ignored). Usage:  scripts/build-web.sh [--dev] [--single]
#   --dev     unoptimised build (quick to compile, slow to run)
#   --single  also write web/dist/monkey-ball.html: the whole game page in one file that opens from disk
#   needs: rustup target add wasm32-unknown-unknown
#          cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock> --locked
# web/dist/ is a static site: host it anywhere, or try it with the developer server:
#   cd web/dist && python -m http.server 8080      (then open http://localhost:8080/)
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

fail() { echo "build-web: $*" >&2; exit 1; }

dev=0
single=0
for arg in "$@"; do
  case "$arg" in
    --dev) dev=1 ;;
    --single) single=1 ;;
    *) fail "unknown option $arg" ;;
  esac
done

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
if [ "$dev" = 1 ]; then
  profile=debug
  flags=()
fi

cargo build -p mb-web --target wasm32-unknown-unknown --target-dir "$target_dir" ${flags[@]+"${flags[@]}"}

# Glue as a classic script (no ES modules): the same file then works in the page, in the import worker and
# inlined in the single-file build.
pkg="$target_dir/web-pkg"
rm -rf "$pkg"
wasm-bindgen --target no-modules --no-typescript --out-dir "$pkg" \
  "$target_dir/wasm32-unknown-unknown/$profile/mb_web.wasm"

bundle=(bundle-web --pkg "$pkg")
if [ "$single" = 1 ]; then
  # The single file carries the module gzipped (the browser unpacks it with DecompressionStream).
  gzip -9 -c "$pkg/mb_web_bg.wasm" > "$pkg/mb_web_bg.wasm.gz"
  bundle+=(--single --wasm-gz "$pkg/mb_web_bg.wasm.gz")
fi
cargo run --quiet -p xtask --target-dir "$target_dir" -- "${bundle[@]}"

size="$(wc -c < "$root/web/dist/pkg/mb_web_bg.wasm")"
echo "built web/dist ($((size / 1024)) KiB wasm). Serve it with: cd web/dist; python -m http.server 8080"
