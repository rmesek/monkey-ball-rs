# State and roadmap

A Rust port of Super Monkey Ball 1 that follows WebMonkeyBall (TypeScript) as closely as possible, fully
offline, for exactly two targets.

## Targets

| Target | Built from | Entry |
|---|---|---|
| Desktop executable | `crates/mb-app` (binary `mb-app`) | `cargo run -p mb-app --release` |
| Static web site (WebAssembly) | `crates/mb-web` + `web/` | `cargo xtask build-web`, output `web/dist/` |

There is no native mobile target. The site is plain files with relative paths, no server-side component and
no dependency on a service worker, with WebGL2 as a first-class backend. It must keep working under a static
server no more capable than `python -m http.server`, and as the single file `monkey-ball.html` opened from
disk. Someone else may show the same files in an Android `WebView`; `docs/ANDROID_WEBVIEW.md` says what that
takes.

## What works

- The user supplies one file, their own disc image of the game (RVZ uncompressed or Zstandard, ISO/GCM; USA
  tested, Europe and Japan accepted by id). The desktop program reads it in place and remembers its path;
  the page copies the 144 MB of files the game uses into the browser's private storage once and starts
  without asking afterwards, or plays from the picked file where it cannot store.
- Title menu, Course Play (Beginner, Advanced, Expert and the Extra / Master courses), Practice, Options.
  Deterministic 60 Hz simulation, bit-identical to the reference on every stage of the USA disc for the
  scripted runs tried (`docs/ENGINE.md`); fly-in,
  goal, fall-out, time over, bonus stages, lives, score, continues, unlocks; HUD; pause and in-place retry.
- Rendering of stages, backgrounds, ball, bananas, goals and HUD on wgpu: Vulkan / Metal / DX12 on the
  desktop, WebGPU or WebGL2 in the browser.
- Sound effects and stage music from the disc's own banks and streams, through cpal on the desktop and Web
  Audio in the browser.
- Keyboard, gamepad, mouse, touch (on-screen stick, touch-only menus) and device tilt.
- Saves in two checksummed slots; the import is atomic; both survive a kill at any moment. The options screen
  shows what is kept on the device and removes it.

Verified by `cargo test --workspace` (398 tests with the disc; without it the disc-backed ones skip) and `deno run -A scripts/web-e2e.ts` (60
checks in the installed Chrome). Not verified: a real phone, an Android web view, iOS, Firefox, Safari, a
real gamepad, a full course played to its end by a person.

## Decisions (recorded)

- **Graphics:** `wgpu`, one API for desktop and browser; nothing beyond `downlevel_webgl2_defaults`.
- **Windowing / input:** `winit` for the desktop window and the browser canvas. Two drivers on top of it,
  `mb-app/src/app.rs` (desktop) and `mb-app/src/shell.rs` (browser); they differ in pacing, GPU setup and
  devices and share everything from the state stack up (`shell.rs` explains the split).
- **Disc reading:** the project's own readers in `mb-assets` on every target: `rvz.rs` (RVZ, none and
  Zstandard via `ruzstd`) and `disc.rs` (GameCube header, file table, `main.dol`), plain ISO/GCM included.
  The `nod` crate is a dev-dependency of `mb-assets`' comparison test only. Other containers are refused with
  a message that says what the file is and what is supported.
- **Browser:** `wasm-bindgen` + `web-sys`, built with `wasm-bindgen-cli --target no-modules` (classic
  scripts, so the same glue runs in the page, in the import worker and inlined in the single file). The disc
  image is never held in memory: a worker copies the files the game uses into one pack in the Origin Private
  File System (or serves them from the picked file where that is unavailable); the game prepares files
  asynchronously before each state that needs them (`docs/WEB.md`). rustc's default WebAssembly feature set
  is used as it is; the browser floor is Chrome / WebView 97, Safari 16, Firefox 79.
- **Saves:** two checksummed slots per value (`mb_game::SlotStorage`) over files (desktop), the private file
  system or `localStorage` (browser): crash-safe without relying on an atomic rename.
- **Hosting:** any static server; the reference is `python -m http.server`. There is no project web server.
- **Parsers** (`mb-tpl`, `mb-gma`, `mb-stage`) and `mb-audio`, `mb-game`, `mb-engine` are std-only.
- **Audio:** `mb-audio` decodes and mixes with no output dependency; device output is `cpal` on the desktop
  and Web Audio in the browser, where the browser mixes and music is decoded straight into its buffers
  (`docs/AUDIO.md`).
- **Developer support stays out of the product:** the windowless driver, the autopilot and test doubles are
  behind `mb-app`'s `dev` feature, which only `cargo test` and an explicit `--features dev` switch on.
- **Strictly offline:** no multiplayer, lobbies, netplay, rollback, telemetry, update checks or online
  leaderboards, and no networking crates. Deterministic simulation and state hashing stay (replays, tests).

## Possible next features

Drawn from the known gaps (`docs/GAMEPLAY.md` "Not ported", `docs/AUDIO.md` "Known gaps"). None is started.

| Feature | Where it would live | Notes |
|---|---|---|
| The monkey inside the ball | `mb-gma` (skeletal models), `mb-app/play_scene.rs` | a separate skeletal system in the reference; its files must be added to `StageFiles` / `common_files` |
| Ball shadow | `mb-render`, `mb-app/play_scene.rs` | projected shadow as in the reference's renderer |
| Effects: goal confetti, sparks, streaks, banner flashes | `mb-engine` (state), `mb-render` (additive sprites), `mb-app` | `effects.ts` in the reference |
| Remaining sounds: monkey voices, softest impact tier, time-over explosion, jingles, menu music | `mb-app/audio.rs`, `mb-audio` | more banks and streams mean a larger import; update `sfx_paths` and the file lists |
| Result replays (fall-out and goal) | `mb-engine`, `mb-app/play.rs` | the simulation is deterministic and `Clone` |
| Mirror floors, bonus-wave vertex animation, per-background effects | `mb-render`, `mb-app/stage_scene.rs` | |
| Fewer draw calls on weak phones | `mb-render` | merge static background meshes per material; one render pass instead of two (`docs/WEB.md`) |
| Other regions verified | tests | Europe and Japan are accepted by id but untested |
| SMB2 content | `mb-stage`, `mb-engine`, `mb-game` | wormholes, switches, seesaws; another disc and another file list |
