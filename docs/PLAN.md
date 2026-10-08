# Plan

Goal: a Rust parser and engine mirroring WebMonkeyBall, fully offline. SMB1 first. Two stages.

## Stage 1 (current goal): complete playable game on desktop and web browser (WASM)
The full offline Super Monkey Ball loop, running on native desktop and in modern browsers through
WebAssembly with WebGPU or the WebGL2 fallback.

- **Single-file user entry.** The game accepts `Super Monkey Ball (USA).rvz` directly: file picker or
  drag-and-drop in the browser, CLI argument or file dialog on desktop. Stages, collision, models and textures
  are extracted on demand from that image. No copyrighted assets are distributed, bundled or written out.
- **Complete gameplay.** Level/course select, deterministic 60 Hz physics (stage tilt, sphere rolling,
  collision, camera tracking, goal triggers, fall-out resets), lives/score/timer HUD, next-floor progression.
- **Decoupled input.** Desktop keyboard/gamepad and web controls map to normalized tilt/action values in
  `InputState`; nothing above the shells sees a device. Stage 2 injects gyro and touch through the same seam.
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
Stage 1 is implemented and verified (2026-10-08):
- Desktop: `cargo run -p mb-app --release -- "<rvz>"` (or no argument for a file dialog) opens the title menu;
  Course Play and Practice run from fly-in to goal with HUD, pause, retry, game over/continue and saving.
  Headless autoplay (`--autoplay 1 --screenshot-dir <dir>`) walks the menu and reaches the goal; integration
  tests in `crates/mb-app/tests/gameplay.rs` cover goal -> next floor, fall-out -> retry, game over/continue.
- Browser: `scripts/build-web.ps1` then `cargo run -p xtask -- serve-web`; the page takes the RVZ by picker or
  drop, shows the menu, plays Beginner floor 1 to the goal and loads floor 2, on WebGPU and WebGL2; progress is
  saved in `localStorage`.
- Gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, and wasm32 clippy for `mb-web` are clean.

Not verified: a real phone, a real gamepad, Firefox/Safari, a full course played to its end.

Not in Stage 1 (see `docs/GAMEPLAY.md` for the full list): audio output (PCM decoding exists), the monkey
inside the ball, ball shadow, goal confetti and other effects, result replays, render interpolation above
60 Hz, SMB2 content, minigames, Android packaging.
## Stage 2 (next target): standalone Android APK
Native Android packaging with `android-activity`, gyro tilt through `SensorManager` feeding the tilt axes of
`InputState`, on-screen touch controls (the layout maths already lives in `mb-app::ui`), disc import through the
Storage Access Framework, safe-area insets, suspend/resume surface handling.

## Asset policy
Code only. The engine reads the user's own disc image through a pack-provider trait (disc, directory, memory
backends). The repo never contains or fetches original game files.

## Decisions (recorded)
- Graphics: `wgpu` (Vulkan/Metal/DX12/GL + WebGPU/WebGL2): one API for desktop, browser and Android.
- Windowing/input: `winit` (desktop, web canvas, Android via android-activity).
- Disc reading: `nod` (feature `disc`) on native. nod's RVZ reader calls `std::time::Instant` and panics on
  wasm32, so the browser uses the pure-Rust GameCube RVZ reader in `mb-assets::rvz` (compression "none" and
  Zstandard via `ruzstd`; verified byte-identical to nod over the whole disc with `xtask check-rvz`) and nod only
  parses the resulting ISO stream. RVZ with bzip2/LZMA/LZMA2, WIA and GCZ are refused in the browser with a message.
- Web: `wasm-bindgen` + `web-sys`, built with `wasm-bindgen-cli`; the disc image stays on the JS heap in 8 MiB
  pieces and only requested ranges are copied into wasm memory (see `docs/WEB.md`).
- Parsers (`mb-tpl`, `mb-gma`, `mb-stage`) are std-only.
- Audio: `mb-audio` produces PCM and has no output dependency; device backend undecided.

## Scope rule: strictly offline
No networking of any kind: no multiplayer, lobbies, netplay, rollback netcode, WebRTC, telemetry, update checks or
online leaderboards. Do not add networking crates. Deterministic simulation and state hashing stay (replays, tests).

## UI direction
Mobile- and browser-first layout: large tap targets, safe-area insets, portrait and landscape, scalable HUD,
menus usable with touch, keyboard and gamepad. Keep the original game flow.
