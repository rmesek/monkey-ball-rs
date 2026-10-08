# AGENTS.md

Guidance for AI agents and contributors working in `monkey-ball-rs`.

## Goal
A Rust port of Super Monkey Ball 1 that mirrors WebMonkeyBall (TypeScript) as closely as possible, fully
offline. Do not invent a new game, art, audio or levels. The current state, the recorded decisions and the
list of possible next features are in `docs/PLAN.md`; keep it current. **Start there when picking the work up:** its
"Open items" table is the list of what is unverified or unfinished.

## The two targets
Exactly two, and every change must keep both working:

1. **The desktop executable** (`crates/mb-app`, binary `mb-app`).
2. **The game in a web browser, as a light static site** (`crates/mb-web` compiled to WebAssembly, plus
   `web/`), built into `web/dist/` by `cargo xtask build-web`.

The static site is a design constraint, not a packaging detail:

- It must provide all functionality under a general-purpose static file server no more capable than
  `python -m http.server`: no server-side code, no custom headers (so no `SharedArrayBuffer` or threads), no
  required MIME types beyond what such a server guesses, relative paths only, any sub-path.
- It must also work as the single file `web/dist/monkey-ball.html` opened from disk.
- Nothing may be required that some supported environment lacks: service worker, private file system,
  `localStorage`, WebGPU, fullscreen, wake lock and the sensors are all optional (`docs/WEB.md` lists what the
  page does without each). WebGL2 is a first-class backend.
- The same files will be shown in an Android `WebView` by someone else (`docs/ANDROID_WEBVIEW.md`). There is
  no native mobile target in this repository; do not add one.

The user's flow, on both targets: the app asks for the Super Monkey Ball 1 game file and says exactly what it
expects; the user picks their own file; the app validates it, keeps what it needs on the device, tells the
user what is kept and lets them erase it; the game runs and survives a sudden exit without corruption.

## Hard rules
1. `../WebMonkeyBall` is an **external, read-only reference**. Never modify, create files in, commit to, or
   push from it. Read it with read-only tools only.
2. **No game data in this repository or in the built site.** Never commit, generate, or place copies of
   original models, textures, stagedefs, audio or HUD images, or anything derived from them (dumps, hashes of
   content are fine, bytes are not). `web/dist` contains the engine only. Icons are original art. Tests and
   tools must not write game data to disk, except to a directory the developer names explicitly (see
   `MB_AUDIO_OUT` in `crates/mb-app/tests/audio.rs`) and never inside the repository.
3. **Never download game files** from any host. Tests that need the game read the developer's own disc image
   (`MB_DISC`, or `Super Monkey Ball*.rvz|iso|gcm` in the directory that contains the repository) and skip
   cleanly without it.
4. **Never modify, move or rename the user's disc image.** It is opened read-only, in place.
5. **Strictly offline.** No network or multiplayer functionality and no networking crates. Skip the
   reference's `netplay`, `netcode_protocol`, `rollback` and lobby code. The page makes no request except for
   its own bundle files, and its Content-Security-Policy says so. Deterministic simulation and `sim_hash`
   stay, for replays and tests.
6. **Rust only** for the game. The build tool is Rust too (`xtask`, standard library only; no shell scripts). The
   exceptions are the page (`web/*.html`, `web/*.js`) and the browser test (`scripts/web-e2e.ts`). Keep `mb-assets`, the parsers, `mb-audio`,
   `mb-engine` and `mb-game` free of GPU, windowing and browser code.
7. **Every third-party dependency needs a one-line justification** in its `Cargo.toml` and in the commit
   message. The recorded stack is in `docs/PLAN.md` and the root `Cargo.toml`. Remove a dependency when its
   last user goes.

## Storage and crash-safety invariants
Changes to import, storage or saves must keep these, and the tests that prove them:

- **One pack per disc image, never two.** The browser keeps one game pack (`mb_assets::gamepack`); importing
  again replaces it. The desktop copies nothing and remembers only the image's path.
- **The import is atomic.** The pack's header is written last and the marker after that; a pack without a
  valid marker is deleted at the next start. Every file carries a checksum that is verified after the import
  and again whenever the file is read. (`an_import_cut_off_after_any_write_is_never_taken_for_a_pack`, the
  browser test's "closing the tab mid-import".)
- **Saves use two checksummed slots** (`mb_game::SlotStorage`): a write replaces the older slot, a read takes
  the newest complete one, so a write cut short loses nothing older, on any backend. Saves are written when
  they happen (a stage result, an option change), never at exit. (`mb-game`'s crash-point tests,
  `crates/mb-app/tests/kill.rs`.)
- **The user can see and erase what is stored.** The options screen shows what is kept and where
  (`mb_app::host::info`) and offers "Remove game data" / "Forget game image" and "Erase save data".
- **Not wasteful.** Only the files the game reads are copied (144 MB of 475 MB), the image is never held in
  memory, and files fetched for a load are released after it.

## Quality gates
All must hold at every commit. `cargo xtask check` runs the first five.

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p mb-app -- -D warnings
cargo clippy -p mb-web --target wasm32-unknown-unknown --target-dir target/wasm -- -D warnings
cargo test --workspace --release
cargo xtask build-web --single
deno run -A scripts/web-e2e.ts
```

The second clippy line is the desktop program as shipped (without the `dev` feature), the third the browser
build. The tests need the disc image to mean anything (set `MB_DISC`); the browser check needs it too, plus
Chrome and Python 3. `cargo test` needs no flags: `mb-app` switches its own `dev` feature on for its tests.

## Where things live, and how to add a feature

| Layer | Crate | Belongs here |
|---|---|---|
| Files | `mb-assets` | reading the disc image, RVZ, LZSS, the game pack format |
| Formats | `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio` | parsing and decoding; std only; hostile input never panics or over-allocates |
| Simulation | `mb-engine` | everything that affects the outcome of a tick; deterministic; ported operation by operation from the reference |
| Rules | `mb-game` | courses, lives, score, unlocks, settings, saves |
| Drawing | `mb-render` | GPU resources, materials, passes; knows nothing of game rules or windows |
| The game | `mb-app` | states (`menu`, `loading`, `play`, `hud`), scene assembly, input, audio requests, content access, and the two drivers: `app.rs` (desktop window) and `shell.rs` (browser canvas) |
| Browser glue | `mb-web`, `web/` | what only the page can do: the file picker, the import worker, storage, Web Audio, history, sensors |

Rules of thumb:

- **Simulation or look?** If it changes where the ball is, it goes into `mb-engine` with a parity or synthetic
  test. If it only changes what is drawn or heard, it reads the simulation state from `mb-app`
  (`play_scene.rs`, `audio.rs`) and never writes back.
- **Input goes through one seam.** Drivers write logical actions and axes into `mb_app::input::InputState`;
  game states read `tilt()`, actions and the pointer and never see a device. A new device is a few lines in
  `app.rs` (desktop) or in `mb-web`'s `Platform` implementation (browser).
- **Platform features go through `mb_app::host`**: states read `host::caps()` and post `host::Command`s; the
  driver carries them out. Do not call platform APIs from a state.
- **A loader that reads a new file must add it to the file lists** in `crates/mb-app/src/content.rs`
  (`StageFiles` for per-stage files, `common_files`, `boot_files`). The browser import copies exactly
  `all_files()`, and the browser can only read files that were prepared from those lists. The tests
  `the_import_copies_exactly_what_the_game_reads` and `every_course_stage_prepares_and_loads_from_the_pack`
  (`crates/mb-app/tests/pack.rs`) fail when a loader reads something the lists do not name. A change to what
  is copied also needs `mb_assets::gamepack::VERSION` raised, so existing imports are redone.
- **Both drivers.** A change to frame pacing, pausing, resizing or input events has to be made in `app.rs`
  and in `shell.rs`; `shell.rs` explains why there are two.
- **Developer and test support stays out of the product.** The windowless driver, the autopilot and test
  doubles are behind `mb-app`'s `dev` feature. Do not add developer options to the product's command line or
  developer code paths to the wasm.
- **Graphics stay low-power and web-friendly:** upload vertex, index and texture data once and cache it,
  reuse pipelines and bind groups, update only per-frame uniforms, do not redraw an unchanged frame, and use
  nothing beyond `wgpu::Limits::downlevel_webgl2_defaults`.
- **UI is touch-first:** safe-area insets, portrait and landscape, tap targets of at least 48 dp, every screen
  usable with touch alone as well as with keys and a gamepad.
- **Document what changed** in the matching file under `docs/`, and measurements with how they were taken.

## Operating rules
- **Autonomous execution.** Work continuously until the task is done and verified on both targets. Work
  through ordinary obstacles. Stop only for a destructive or irreversible action, or for something only the
  user can supply.
- **Zero placeholders.** No stubs, dummy buffers, empty mocks, `todo!()` / `unimplemented!()`, or tests
  weakened to get past a check. What is committed works. What is out of scope is left out and listed in
  `docs/PLAN.md`.
- **Verify by running.** A feature is done when it has been seen working: on the desktop through the tests
  that drive the real states (and the window), in the browser through `scripts/web-e2e.ts`, extended to cover
  the new behaviour. State what was not verified (real phones, other browsers).
- **Git checkpoints.** One logical change per commit, gates green, a message that says what now works.
- **No paid services.** Nothing runs on a push, a tag or a schedule. `ci.yml` (the gates) is
  `workflow_dispatch` only; `release.yml` starts only when a release is published on the GitHub website and
  attaches that version's builds to it (the gates, then Linux, Windows and Apple-silicon executables and the
  single-file page). Run the checks locally. No cloud or remote agents, no hosted builders.
- Match the surrounding code's style; keep comments sparse and explain *why*.

## Subagent orchestration
For work that splits cleanly, the lead agent hands workstreams to subagents by crate ownership:

| Workstream | Owns |
|---|---|
| Files and formats | `mb-assets`, `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio` |
| Simulation and rules | `mb-engine`, `mb-game` |
| Rendering | `mb-render` |
| Game and desktop | `mb-app` |
| Browser | `mb-web`, `web/`, `scripts/`, `xtask` |

- An agent edits only the files it owns; shared interfaces are agreed in its brief. Agents work in the same
  checkout and do not revert or reformat each other's files.
- `claude-opus-5-5` for complex work (ports of reference logic, rendering, cross-crate design),
  `claude-haiku-5-5` for mechanical work (lint cleanup, doc updates, simple searches).
- Agents never run state-changing git commands (`git status`, `diff` and `log` are fine). An agent reports
  what it built, what it verified and how, and what is open; the lead re-runs the gates and commits.

## Reference map (WebMonkeyBall, read-only)

| Topic | Reference files | Here |
|---|---|---|
| LZSS | `src/lzs.ts`, `src/noclip/SuperMonkeyBall/AVLZ.ts` | `mb-assets/src/lzss.rs` |
| Stagedef, collision | `src/stage.ts`, `src/noclip/SuperMonkeyBall/Stagedef.ts`, `src/collision.ts` | `mb-stage`, `mb-engine/src/collision.rs` |
| Models, textures | `src/gma.ts`, `src/gx_displaylist.ts`, `src/tpl.ts`, `src/noclip/SuperMonkeyBall/{Gma,NaomiLib,AVTpl}.ts` | `mb-gma`, `mb-tpl` |
| Materials, lighting, scene | `src/noclip/SuperMonkeyBall/{Material,TevLayer,Shape,Model,Lighting,Render,World,AnimGroup,Background,StageInfo}.ts` | `mb-render`, `mb-app/src/{stage_scene,play_scene,stage_info,anim}.rs` |
| Physics, maths | `src/physics.ts`, `src/math.ts`, `src/math_lut.ts`, `src/world.ts`, `src/animation.ts`, `src/rng.ts` | `mb-engine` |
| Camera | `src/camera.ts` | `mb-engine/src/camera.rs` |
| Game flow | `src/game.ts`, `src/course.ts`, `src/constants.ts` | `mb-engine/src/sim.rs`, `mb-game`, `mb-app/src/{menu,play}.rs` |
| HUD, input | `src/hud.ts`, `src/input.ts` | `mb-app/src/{hud,text,input,ui}.rs` |
| Audio | `src/audio.ts`, `src/audio/smb1_sfx_ids.ts` | `mb-audio`, `mb-app/src/audio.rs` |
| Determinism | `src/determinism.ts`, `src/sim_hash.ts` | `mb-engine/src/{determinism,hash}.rs` |
| Effects, replays (not ported) | `src/effects.ts`, `src/replay.ts` | `docs/PLAN.md`, "Possible next features" |
| Not used (network, SMB2) | `src/netplay.ts`, `src/netcode_protocol.ts`, `src/rollback.ts`, `src/course_smb2.ts`, `src/course_mb2ws.ts`, `src/smb2_render.ts` | |

Format and porting notes: `docs/REFERENCE_NOTES.md`, `docs/STAGEDEF.md`, `docs/GMA.md`, `docs/TPL.md`,
`docs/ENGINE.md`, `docs/AUDIO.md`, `docs/GAMEPLAY.md`. Parity tests compare against the reference's output on
locally supplied content; never commit that content or dumps of it.
