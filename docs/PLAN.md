# Plan

Goal: a Rust parser and engine mirroring WebMonkeyBall, fully offline. SMB1 first.

## Stage 1 (current goal): complete playable game on desktop and web browser (WASM)
The full offline Super Monkey Ball loop, running on native desktop and in modern browsers through
WebAssembly with WebGPU or the WebGL2 fallback.

- **Single-file user entry.** The game accepts `Super Monkey Ball (USA).rvz` directly: file picker or
  drag-and-drop in the browser, CLI argument or file dialog on desktop. Stages, collision, models and textures
  are extracted on demand from that image. No copyrighted assets are distributed, bundled or written out.
- **Complete gameplay.** Level/course select, deterministic 60 Hz physics (stage tilt, sphere rolling,
  collision, camera tracking, goal triggers, fall-out resets), lives/score/timer HUD, next-floor progression.
- **Decoupled input.** Desktop keyboard/gamepad and web controls map to normalized tilt/action values in
  `InputState`; nothing above the shells sees a device. Touch and device tilt come in through the same seam.
- **Low-power, web-friendly graphics.** Cached GPU buffers and textures, shared pipelines, per-frame uniform
  updates only; nothing beyond WebGL2 downlevel limits.

### Workstreams
| # | Workstream | Crates | Done when |
|---|---|---|---|
| 1 | Disc ingestion | `mb-assets` | `STAGE*.lz`, stage/background models and textures stream from the RVZ on native and `wasm32-unknown-unknown` (in-memory file buffer from picker/drag-and-drop) |
| 2 | Rendering | `mb-render` | stage, background, ball, goals, bananas and HUD sprites draw on a desktop window and an HTML `<canvas>` (WebGPU, WebGL2 fallback) |
| 3 | Simulation + game flow | `mb-engine`, `mb-game`, `mb-app` | 60 Hz tick drives tilt, ball, collision, camera; Level Select -> Active Play -> Goal / Fall-out -> next floor or retry |
| 4 | Desktop runner | `mb-app` | native window, takes the RVZ by argument or dialog, stable 60 FPS |
| 5 | Web shell | `mb-web`, `web/`, `scripts/` | one build command; page with RVZ drop zone/picker, level select, smooth play |
| 6 | Validation | all | gates in `AGENTS.md` green; a stage is played start to goal on both platforms |

### Stage 1 done when
`cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` are clean; the desktop
binary and the browser build both open the RVZ, show the level select, play a stage to the goal, handle a
fall-out with a retry, and advance to the next floor.

### Status
Implemented and verified (2026-10-08):
- Desktop: `cargo run -p mb-app --release -- "<rvz>"` (or no argument for a file dialog) opens the title menu;
  Course Play and Practice run from fly-in to goal with HUD, pause, retry, game over/continue and saving.
  Headless autoplay (`--autoplay 1 --screenshot-dir <dir>`) walks the menu and reaches the goal.
- Browser: `scripts/build-web.*` produces the static site `web/dist/` (and, with `--single`, one
  self-contained `.html`); served by `python -m http.server` the page imports the picked image once into
  on-device storage (144 MB of 475 MB), starts without a picker afterwards, and plays on WebGPU and WebGL2.
  Without storage (plain http on a network address, a page opened from disk) it plays from the picked file.
  `deno run -A scripts/web-e2e.ts` checks all of this in the installed Chrome.
- Touch-only menus (scrolling, Back button), device tilt, stage loading with a progress screen, in-place
  retry after a lost life, load failures reported in the menu, crash-safe saves and import.
- Gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, and wasm32 clippy for `mb-web` are clean.

Not verified: a real phone, an Android web view, iOS, Firefox/Safari, a real gamepad, a full course played to
its end.

Not in scope (see `docs/GAMEPLAY.md` for the full list): the monkey's voices, the monkey inside the ball, ball
shadow, goal confetti and other effects, result replays, SMB2 content, minigames.

## Targets
Exactly two: the desktop executable and the static web site. There is no native Android target; the site is
plain files with relative paths, no server-side component and no dependency on a service worker, with WebGL2
as a first-class backend, so it can be placed inside another application's web view by whoever packages one.

## Asset policy
Code only. The engine reads the user's own disc image through a pack-provider trait (disc image, imported pack, memory
backends). The repo never contains or fetches original game files.

## Decisions (recorded)
- Graphics: `wgpu` (Vulkan/Metal/DX12/GL + WebGPU/WebGL2): one API for desktop and browser.
- Windowing/input: `winit` (desktop, web canvas).
- Disc reading: the project's own readers in `mb-assets` on every target: `rvz.rs` (RVZ, none and Zstandard
  via `ruzstd`) and `disc.rs` (GameCube header, file table, `main.dol`), plain ISO/GCM included. The `nod` crate
  is a dev-dependency of the comparison test and of `xtask check-rvz` only (identical over the whole tested
  disc). Other containers are refused with a message that says what the file is and what is supported.
- Web: `wasm-bindgen` + `web-sys`, built with `wasm-bindgen-cli --target no-modules` (classic scripts, so the
  same glue runs in the page, in the import worker and inlined in the single file). The disc image is never
  held in memory: a worker copies the files the game uses into one pack in the Origin Private File System
  (or serves them from the picked file where that is unavailable); the game prepares files asynchronously
  before each state that needs them (see `docs/WEB.md`).
- Saves: two checksummed slots per value (`mb_game::SlotStorage`) over files (desktop), the private file
  system or `localStorage` (browser): crash-safe without relying on an atomic rename.
- Hosting: any static server; the reference is `python -m http.server`. There is no project web server.
- Parsers (`mb-tpl`, `mb-gma`, `mb-stage`) are std-only.
- Audio: `mb-audio` decodes and mixes with no output dependency; device output is `cpal` on desktop and Web
  Audio (`web-sys`) in the browser, where the browser does the mixing. See `docs/AUDIO.md`.

## Scope rule: strictly offline
No networking of any kind: no multiplayer, lobbies, netplay, rollback netcode, WebRTC, telemetry, update checks or
online leaderboards. Do not add networking crates. Deterministic simulation and state hashing stay (replays, tests).

## UI direction
Mobile- and browser-first layout: large tap targets, safe-area insets, portrait and landscape, scalable HUD,
menus usable with touch, keyboard and gamepad. Keep the original game flow.
