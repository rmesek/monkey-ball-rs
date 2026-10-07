#!/usr/bin/env bash
# Local replacement for CI: free, runs on your machine.
set -euo pipefail
cd "$(dirname "$0")/.."
if git rev-parse --git-dir >/dev/null 2>&1; then
  if git ls-files | grep -E '\.(iso|gcm|rvz|wia|ciso|gcz|gma|tpl|lz|ogg|wav)$|^(content|smb[12]_content|mb2ws_content|audio)/'; then
    echo "Game content must not be tracked"; exit 1
  fi
fi
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
