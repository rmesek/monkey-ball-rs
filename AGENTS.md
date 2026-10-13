# AGENTS.md

Rules for AI agents and people changing `monkey-ball-rs`. Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
for how the code is put together and [docs/PLAN.md](docs/PLAN.md) for the state of the project; keep
PLAN.md current, it is the progress record.

## Goal

Super Monkey Ball 1 in Rust, fully offline, small enough to understand, port and change by hand. It reads
the user's own disc image on the device, plays the game as it is, never leaves a damaged state behind, and
has the tests that keep it so. Prefer removing code to adding it. Do not invent game content, and add no
feature, option, tool or abstraction that the four aims in PLAN.md do not need.

Targets: the desktop program (`cargo run --release`) and the static web site
(`cargo run --release -p xtask -- build-web`). Android is later, in a WebView and natively; do not start it,
or any other target, unless the owner asks.

## Hard rules

1. **No game data in this repository or in the built site.** Never commit, generate or place copies of
   models, textures, stages, audio or HUD images, or anything derived from them. Hashes and counts are
   fine, bytes are not. Tests and tools write no game data to disk outside a directory the developer names.
2. **Never download game files.** Tests that need the game read the developer's own disc image (`MB_DISC`,
   or `Super Monkey Ball*.rvz|iso|gcm` in the directory that contains the repository) and skip without it.
3. **Never modify, move or rename the user's disc image.** It is opened read-only.
4. **Strictly offline.** No networking crates. The page requests nothing but its own files, and its
   Content-Security-Policy says so. The build tool serves nothing either: it builds, and hosting the
   built site is the user's own static file server.
5. **Rust only** for the game and the build tool; no shell scripts. The exceptions are the page (`web/`)
   and the browser test (`scripts/web-e2e.ts`).
6. **Every third-party dependency has a one-line reason** in its `Cargo.toml`. Remove one when its last
   user goes.
7. **The site stays plain files** that `python -m http.server` can host from any sub-path: no server-side
   code, no custom headers (so no threads), relative paths only. Service worker, private file system,
   `localStorage`, WebGPU, fullscreen and sensors are all optional at run time; WebGL2 is a first-class
   backend.
8. **No release without the owner's test.** No tag and no push before the owner has tried the release
   candidate by hand and confirmed.

## What must stay true

- **One pack of game data, imported atomically.** The pack's header is written last (and on a file system
  the pack is renamed into place when complete); a pack that is not whole is removed at the next start.
  Every file in it carries a checksum that is checked when it is read.
- **Saves use two checksummed slots** (`mb_game::SlotStorage`) and are written when they happen (a stage
  result, an option change), never at exit. A write cut short loses nothing older.
- **The user can see and erase what is stored:** the options screen says what is kept and offers "Remove
  game data" and "Erase save data".
- **Platform code lives in a platform crate** (`mb-desktop`, `mb-web`). `mb-app` and everything under it
  know no window, event loop or operating system, and must keep compiling for Android.
- **States reach the platform only through the seams:** `mb_app::input` for input, `mb_app::host` for
  capabilities and commands, `mb_app::content` for files, `mb_game::Storage` for saves.
- **Effects never write into the simulation.** Particles use their own random stream and are outside
  `Sim::hash`.
- **Nothing is created on the GPU during play,** and nothing in the renderer goes beyond
  `wgpu::Limits::downlevel_webgl2_defaults`.
- **Every screen works by touch alone,** with safe-area insets, in portrait and landscape.

## Gates

All must hold at every commit. `cargo run --release -p xtask -- check` runs the first six.

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p mb-desktop -- -D warnings
cargo clippy -p mb-web --target wasm32-unknown-unknown --target-dir target/wasm -- -D warnings
cargo clippy -p mb-app --target aarch64-linux-android --target-dir target/android -- -D warnings -A clippy::missing_const_for_thread_local
cargo test --workspace --release
cargo run --release -p xtask -- build-web
deno run -A scripts/web-e2e.ts
```

The tests need the disc image to mean anything. The browser test needs it too, plus Deno, Chrome and
Python 3; it takes about eight minutes. Rust edition 2024.

What the tests hold, and how to change it:

- **Recorded hashes** (`tests/golden.rs` in `mb-engine` and the parsers, `mb-assets/tests/disc.rs`) say
  that the simulation, the parsers and the disc reader did not change. A change that is meant updates the
  table in the same commit and says why; the failure prints the new values.
- **Budgets** (`mb-app/tests/{budgets,memory,pack}.rs`, `mb-engine/tests/effects.rs`, `WASM_BUDGET_BYTES`
  in `xtask`, the figures in `scripts/web-e2e.ts`) are recorded constants. Do not edit one to make a test
  pass: find the cause first. When the change is meant, change the constant with its cause.
- **GPU tests are serialised:** a test that creates a device takes the file's `static GPU: Mutex<()>`.

## Where a change goes

| It changes | Crate |
|---|---|
| how a file of the disc is read | `mb-assets`, `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio` (no GPU, window or browser code) |
| where the ball is | `mb-engine`, with a synthetic test; the golden table changes with it on purpose |
| courses, score, settings, saves | `mb-game` |
| how something is drawn | `mb-render` (knows nothing of stages, rules or windows) |
| what is on screen or heard, and when | `mb-app` (reads the simulation, never writes back) |
| a window, a device, a file dialog, page storage | `mb-desktop` or `mb-web`; what the game needs from it becomes a seam in `mb-app` |

- A loader that reads a new file adds it to the file lists in `mb-app/src/content.rs` and raises
  `mb_assets::gamepack::VERSION`; `tests/pack.rs` fails otherwise.
- A change to frame pacing, pausing, resizing or input events is made in both drivers
  (`mb-desktop/src/app.rs`, `mb-web/src/shell.rs`).
- Developer and test support stays behind the `dev` feature and out of the product.

## Working

- Work until the task is done and verified on both targets. Stop only for something destructive or
  something only the owner can supply.
- No placeholders: no stubs, `todo!()`, or tests weakened to pass. What is out of scope is left out and
  listed in PLAN.md.
- A feature is done when it has been seen working: through the tests that drive the real states on the
  desktop, through the browser test in the browser. Say what was not verified.
- One logical change per commit, gates green, a message that says what now works.
- Nothing runs on a push, a tag or a schedule. Run the checks locally.
- Match the surrounding code's style; keep comments sparse and explain why.
