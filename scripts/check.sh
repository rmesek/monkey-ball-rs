#!/usr/bin/env bash
# The quality gates (AGENTS.md), run locally: free, on your machine. Tests that need the game read your own
# disc image (MB_DISC, or "Super Monkey Ball*.rvz|iso|gcm" next to the repository) and skip without it.
# The browser check is separate: scripts/build-web.sh, then deno run -A scripts/web-e2e.ts.
set -euo pipefail
cd "$(dirname "$0")/.."
if git rev-parse --git-dir >/dev/null 2>&1; then
  if git ls-files | grep -E '\.(iso|gcm|rvz|wia|ciso|gcz|gma|tpl|lz|adp|pack|ogg|wav)$'; then
    echo "Game data must not be tracked"; exit 1
  fi
fi
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
# The product configurations: the desktop program without the dev feature, and the browser build.
cargo clippy -p mb-app -- -D warnings
cargo clippy -p mb-web --target wasm32-unknown-unknown --target-dir target/wasm -- -D warnings
cargo test --workspace --release
