# AGENTS.md

Guidance for AI agents and contributors working in `monkey-ball-rs`.

## Goal
A Rust parser and game engine that mirrors WebMonkeyBall (TypeScript) as closely as possible, fully offline.
SMB1 first. Do not invent a new game, art, audio or levels.

The roadmap has two stages (details in `docs/PLAN.md`):
- **Stage 1 (current goal): complete playable game on desktop and in the web browser (WASM).** The user supplies
  one file, `Super Monkey Ball (USA).rvz`; everything else is read from it on demand.
- **Stage 2 (next target): standalone Android APK** (`android-activity`, gyro tilt through `SensorManager`,
  on-screen touch controls). Do not start Stage 2 work until Stage 1 is done, but do not design it out either:
  input stays abstracted so tilt and touch can be injected later.

## Hard rules
1. `../WebMonkeyBall` is an **external, read-only reference**. Never modify, create files in, commit to, or push
   from it. Read it with read-only tools only.
2. **No game data in this repo.** Never commit, generate, or place copies of original models, textures, stagedefs,
   audio or HUD images. Disc images and the content root (`content/`, `smb*_content/`, `audio/`, `assets/`) are
   git-ignored. The built web shell must not bundle game data either; the disc image stays in the user's tab.
3. **Do not download original game files** from any host (including monkeyball-online.pages.dev). Tests that need
   content must skip cleanly when no disc image is available (`MB_DISC`, or `../Super Monkey Ball (USA).rvz`).
4. **Rust only** for the game (the web shell's `index.html` and its small loader script are the exception).
   Everything is designed for desktop, wasm32 and Android. Keep `mb-assets` free of GPU and OS-specific code.
5. Any new third-party dependency needs a one-line justification in `Cargo.toml` and in the commit message.
   Recorded stack: `wgpu`, `winit`, `nod`, `glam`, `bytemuck`, `wasm-bindgen`/`web-sys` (see `docs/PLAN.md`).
6. **Strictly offline.** No network or multiplayer functionality, and no networking dependencies. Skip the
   reference's `netplay`, `netcode_protocol`, `rollback`, lobby/worker code. Deterministic sim + `sim_hash` stay
   for replays and tests.

## Operating rules
- **Autonomous execution.** Work continuously, without pausing for user intervention, until Stage 1 is fully
  playable and verifiable both on desktop and in the browser. Diagnose and work through ordinary obstacles.
  Stop only for a destructive or irreversible action, or for something only the user can supply.
- **Zero placeholders.** Do not commit stubs, dummy buffers, empty mocks, `todo!()`/`unimplemented!()`, or tests
  weakened to get past a check. Every subsystem that is committed must be functional. If something is out of
  scope, leave it out and say so in `docs/PLAN.md`; do not fake it.
- **Git checkpoints.** Commit cleanly at each validated sub-milestone: one logical change per commit, quality
  gates green, a message that says what now works. Never commit game data (run `scripts/check.sh`).
- **Quality gates** (must hold at every checkpoint):
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings` produces zero errors
  - `cargo test --workspace` passes
  - `cargo check -p mb-web --target wasm32-unknown-unknown` passes
- **Verify by running.** A feature is done when it has been seen working: desktop through the headless
  capture/autoplay paths and the window, the browser build by loading the page, opening the RVZ and playing.

## Subagent orchestration
The lead agent splits Stage 1 into independent workstreams and coordinates specialised subagents:

| Workstream | Owns |
|---|---|
| Asset ingestion / decompression | `crates/mb-assets`, disc streaming, wasm32 disc reading |
| Rendering pipeline | `crates/mb-render`, stage/background/ball/goal drawing, WebGPU + WebGL2 |
| Physics / simulation core | `crates/mb-engine`, parity tests |
| Game flow | `crates/mb-game` (courses, lives, score), menus and gameplay states in `crates/mb-app` |
| Desktop runner | `crates/mb-app` binary: window, RVZ dialog/CLI, 60 FPS loop |
| Web harness | `crates/mb-web`, `web/`, build scripts |
| Test verification | workspace gates, headless and in-browser play-through |

Rules for subagents:
- Each agent owns a set of files and edits only those; shared interfaces are agreed in the brief. Agents run
  locally in the same checkout and must not revert or reformat other agents' files.
- Use `claude-opus-5-5` for complex work (ports of reference logic, rendering, cross-crate design) and
  `claude-haiku-5-5` for mechanical work (lint cleanup, doc updates, simple searches).
- An agent reports what it built, what it verified and how, and anything left open. The lead agent re-runs the
  gates, integrates, and commits. No cloud/remote agents and no paid services.

## Reference map (WebMonkeyBall)
| Topic | Files |
|---|---|
| Loader / packs | `src/pack.ts`, `src/constants.ts` (`STAGE_BASE_PATHS`), `src/main.ts` (~L403-480, prefetch ~L3126-3208) |
| LZSS | `src/lzs.ts`, `src/noclip/SuperMonkeyBall/AVLZ.ts` |
| Stagedef / collision | `src/stage.ts`, `src/noclip/SuperMonkeyBall/Stagedef.ts`, `src/collision.ts` |
| Models / textures | `src/gma.ts`, `src/gx_displaylist.ts`, `src/tpl.ts`, `src/noclip/SuperMonkeyBall/NaomiLib.ts` |
| Physics / math | `src/physics.ts`, `src/math.ts`, `src/math_lut.ts` |
| Game flow | `src/game.ts`, `src/course.ts`, `src/hud.ts` |
| Audio | `src/audio.ts`, `src/audio/smb1_sfx_ids.ts` |
| Determinism | `src/determinism.ts`, `src/sim_hash.ts` |

See `docs/REFERENCE_NOTES.md` for details.

## Workflow
- Follow `docs/PLAN.md`; keep its status section current.
- Before finishing: `cargo fmt --all`, the quality gates above, then commit.
- Parity tests compare against the reference's output on locally supplied content; never commit that content or
  its derived dumps.
- Match the surrounding code's style; keep comments sparse and explain *why*.

## Input and UI
- Input is decoupled: platform shells (keyboard, gamepad, browser events, later touch and gyro) only write
  normalized tilt/action values into `InputState`; game states read logical actions and a tilt vector in
  -1..=1 and never see devices.
- Design for mobile and browser: responsive/safe-area layout, scalable HUD, big tap targets.

## Graphics
Low-power and web-friendly: upload vertex/index/texture data once and cache it, reuse pipelines and bind groups,
update only per-frame uniforms, stay within WebGL2 limits (no features beyond `downlevel_webgl2_defaults`).

## Cost rules
- No paid services: no GitHub Actions on push/PR (CI is `workflow_dispatch` only), no cloud/remote agents, no
  hosted builders. Run checks locally with `scripts/check.sh`.
- Prefer small, targeted test runs while iterating; run the full gates before each commit.
