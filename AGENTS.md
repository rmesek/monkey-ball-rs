# AGENTS.md

Guidance for AI agents and contributors working in `monkey-ball-rs`.

## Goal
Super Monkey Ball 1 in Rust, fully offline. It began as a port of WebMonkeyBall (TypeScript, "the
reference") and reached parity with it in the agreed scope at 0.2.0; since then **the project stands on its
own**. Nothing needs the reference to build, test or develop: what the port does is held by this
repository's own tests (`docs/ARCHITECTURE.md`, "Standing without the reference"). Do not invent a new game,
art, audio or levels. Four aims, in this order when they pull against each other:

1. **Feature complete in the agreed scope:** everything the reference shows and plays for Super Monkey
   Ball 1, with all effects, sounds and levels from and only from SMB1. This is done; keep it so. Not multiplayer / netplay / lobby,
   not the replay-file player, not the instant replays after a fall-out and a goal, not the map viewer, not
   SMB2, and not what only the original game has (the monkey in the ball, its voices, jingles, menu music).
2. **Portable between platforms:** the desktop and the web browser now; Android (in a WebView and as a
   native app) later. One crate per platform on a game library that knows no platform
   (`docs/ARCHITECTURE.md`).
3. **Minimal footprint:** download size, memory, storage and per-frame work are measured and held by tests.
4. **Minimal engine complexity,** using only portable and basic features. Prefer removing code to adding
   it; add no pass, pipeline, option or abstraction that nothing on the SMB1 disc needs.

`docs/PLAN.md` spells each aim out as rules that can be checked, and holds the current state, the budgets,
the recorded decisions and the candidates for simplification; keep it current. `docs/ARCHITECTURE.md` says
how the crates fit together, what a platform driver has to supply and what each part is worth.
**Start with those two when picking the work up:** PLAN.md's "Unverified" and "Open items" sections are the
list of what is unfinished.

## The targets
Two today, and every change must keep both working (both worked in 0.1.0; neither may regress):

1. **The desktop executable** (`crates/mb-desktop`, binary `mb-app`).
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
- The same files can be shown in an Android `WebView` by someone else (`docs/ANDROID_WEBVIEW.md`), and a
  phone's browser opens the site as it is.

**A native Android app is a later target.** It is not built and there is no driver for it in this
repository; do not add it, or any other target, unless the project owner asks. The ground is prepared and
must stay so (`docs/ARCHITECTURE.md` has the contract and the list of what a port will meet):

- `mb-app` and every crate under it know no window, event loop or operating system; `cargo xtask check`
  compiles them for `aarch64-linux-android`. Code for one platform goes into that platform's crate
  (`mb-desktop`, `mb-web`), never behind a `cfg` in shared code.
- Nothing in the renderer beyond what WebGL2 allows; platform access only through `mb_app::host` and the
  input seam; game files only through `mb_app::content`; saves only through `mb_game::Storage`; every screen
  usable by touch alone.

The user's flow, on both targets: the app asks for the Super Monkey Ball 1 game file and says exactly what it
expects; the user picks their own file; the app validates it, keeps what it needs on the device, tells the
user what is kept and lets them erase it; the game runs and survives a sudden exit without corruption.

## Hard rules
1. A checkout of WebMonkeyBall beside this repository (`../WebMonkeyBall`) is optional. If it is there it is
   an **external, read-only reference**: never modify, create files in, commit to, or push from it. Nothing
   in the build, the product or the gates may come to depend on it.
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
   stay, for the tests.
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
All must hold at every commit. `cargo xtask check` runs the first six.

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p mb-desktop -- -D warnings
cargo clippy -p mb-web --target wasm32-unknown-unknown --target-dir target/wasm -- -D warnings
cargo clippy -p mb-app --target aarch64-linux-android --target-dir target/android -- -D warnings -A clippy::missing_const_for_thread_local
cargo test --workspace --release
cargo xtask build-web --single
deno run -A scripts/web-e2e.ts
```

The second clippy line is the desktop program as shipped (without the `dev` feature), the third the browser
build, the fourth the game library for a native Android target: a compile check that needs the target's
standard library only (no NDK), there so that shared code cannot quietly become desktop-only. The tests need
the disc image to mean anything (set `MB_DISC`); the browser check needs it too, plus Chrome and Python 3.
`cargo test` needs no flags: `mb-app` and `mb-desktop` switch their own `dev` feature on for their tests.
The tests that compare with the reference run when a checkout and Deno are there and skip otherwise; the
gates must pass either way.

The workspace is Rust edition 2024 (`rust-version` 1.88).

What the gates hold beyond "it works" (the numbers are in `docs/PLAN.md`, "Budgets"):

- **The simulation, recorded:** `crates/mb-engine/tests/golden.rs` runs every stagedef of the disc through
  fixed inputs and compares a hash over every tick with a table. It is what says the simulation did not
  change; a change that is meant updates the table in the same commit and says why.
- **Budget tests** in `cargo test`: draw calls of every course stage against a recorded table, no GPU object
  created and no heap allocation per tick during play, texture bytes, the mirror's cost
  (`crates/mb-app/tests/budgets.rs`, `crates/mb-engine/tests/effects.rs`); heap while a stage loads and
  plays (`crates/mb-app/tests/memory.rs`); the size of the game pack (`crates/mb-app/tests/pack.rs`).
- **The wasm size budget** in `cargo xtask build-web`: the build fails, before `web/dist` is touched, when
  the optimised module is larger than `WASM_BUDGET_BYTES` in `xtask/src/main.rs`.
- **The browser check** (`scripts/web-e2e.ts`, 97 checks, about eight minutes): besides import, storage,
  play, saves and the degraded environments, it measures wasm memory and frame time at 6x CPU throttle on
  WebGL2 against 0.1.0, with Reflections on and off, sees the new drawing on WebGL2 and WebGPU through
  `window.mbPerf`, loads a save written by 0.1.0 and fails on a console error or a graphics driver warning
  (`docs/WEB.md`, "Checking it").

## Where things live, and how to add a feature

| Layer | Crate | Belongs here |
|---|---|---|
| Files | `mb-assets` | reading the disc image, RVZ, LZSS, the game pack format |
| Formats | `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio` | parsing and decoding; std only; hostile input never panics or over-allocates |
| Simulation | `mb-engine` | everything that affects the outcome of a tick; deterministic; ported operation by operation from the reference and held by golden hashes. Also the particles' state (`effects.rs`), which is display only: it is ported here because it steps with the tick and is compared with the reference, but the simulation never reads it |
| Rules | `mb-game` | courses, lives, score, unlocks, settings, saves |
| Drawing | `mb-render` | GPU resources, materials, passes (`renderer.rs`, `material.rs`), instanced models and the quad batch (`instances.rs`, `quads.rs`, `effects.rs`), the vertex wave (`wave.rs`), mirror floors (`mirror.rs`); knows nothing of game rules, stages or windows |
| The game | `mb-app` | states (`menu`, `loading`, `play`, `hud`), scene assembly (`stage_scene`, `play_scene`, `particles`, `backdrop`, `mirror`), audio requests, and the seams a platform plugs into: `input`, `host`, `content`, `gpu`, `audio::AudioBackend`. A library: no window, no event loop, no `winit` except key codes behind a feature |
| Desktop | `mb-desktop` | the desktop driver `app.rs` (window, event loop, native gamepads), the command line, the file dialog, where saves and the image are found |
| Browser | `mb-web`, `web/` | the browser driver `shell.rs` (canvas, event loop) and what only the page can do: the file picker, the import worker, storage, Web Audio, history, sensors |

Rules of thumb:

- **Simulation or look?** If it changes where the ball is, it goes into `mb-engine` with a synthetic test,
  and the golden table changes with it on purpose. If it only changes what is drawn or heard, it reads the simulation state from `mb-app`
  (`play_scene.rs`, `audio.rs`) and never writes back.
- **Effects never write into the simulation.** Particles draw from the reference's separate visual random
  stream (`runtime.visual_rng`) and are outside `Sim::hash` (`Sim::hash_visual` includes them). Random
  numbers for anything else that is only drawn come from a generator of their own.
- **Budgets are recorded constants.** A test that holds a table or a limit (`DRAWS`, `MIRROR_DRAWS`,
  `USAGE_0_1_0`, `WASM_BUDGET_BYTES`, the pack's size) fails when the number moves: the draw tables and the
  pack in either direction, the others upward. Do not edit the constant to make it pass: find the cause
  first. When the change is meant, change the
  constant in the same commit as its cause, say why in the message, and update the figure in the docs.
- **A loader that uploads much must stay under the staging limit.** Uploads go through the renderer, which
  submits whenever `Renderer::STAGED_LIMIT` (1 MB) is staged; on WebGL2 anything staged is held twice in
  memory that never shrinks. `crates/mb-app/tests/memory.rs` fails on a load that stages more at once.
- **Nothing is created on the GPU during play.** Buffers, bind groups and targets are made while a stage
  loads (`Renderer::reserve_objects`, `reserve_instances`, `reserve_quads`, `warm_sprite_texture`).
- **GPU tests are serialised.** Tests that create a device take the file's `static GPU: Mutex<()>` first;
  several devices at once make the counts and the memory figures of other tests wrong.
- **Input goes through one seam.** Drivers write logical actions and axes into `mb_app::input::InputState`;
  game states read `tilt()`, actions and the pointer and never see a device. A new device is a few lines in
  `mb-desktop`'s `app.rs` or in `mb-web`'s `Platform` implementation.
- **Platform features go through `mb_app::host`**: states read `host::caps()` and post `host::Command`s; the
  driver carries them out. Do not call platform APIs from a state.
- **Platform code lives in a platform crate.** Something only one platform has (a dialog, a sensor, a
  storage API) goes into `mb-desktop` or `mb-web`; what the game needs from it becomes a seam in `mb-app`
  that every driver can implement. No `cfg(target_os)` in shared code; the `cfg(target_arch = "wasm32")`
  that is there separates blocking from awaiting and nothing else.
- **A loader that reads a new file must add it to the file lists** in `crates/mb-app/src/content.rs`
  (`StageFiles` for per-stage files, `common_files`, `boot_files`). The browser import copies exactly
  `all_files()`, and the browser can only read files that were prepared from those lists. The tests
  `the_import_copies_exactly_what_the_game_reads` and `every_course_stage_prepares_and_loads_from_the_pack`
  (`crates/mb-app/tests/pack.rs`) fail when a loader reads something the lists do not name. A change to what
  is copied also needs `mb_assets::gamepack::VERSION` raised, so existing imports are redone.
- **Both drivers.** A change to frame pacing, pausing, resizing or input events has to be made in
  `crates/mb-desktop/src/app.rs` and in `crates/mb-web/src/shell.rs`; `shell.rs` explains why there are two,
  and `docs/ARCHITECTURE.md` what to decide before there are three.
- **Developer and test support stays out of the product.** The windowless driver, the autopilot and test
  doubles are behind `mb-app`'s `dev` feature, the program's developer options behind `mb-desktop`'s. Do not add developer options to the product's command line or
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
- **No release without the owner's test.** No release tag and no push before the project owner has tried
  the release candidate by hand and confirmed. Until then the docs call it a release candidate and do not
  say it is released.
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
| Game and desktop | `mb-app`, `mb-desktop` |
| Browser | `mb-web`, `web/`, `scripts/`, `xtask` |

- An agent edits only the files it owns; shared interfaces are agreed in its brief. Agents work in the same
  checkout and do not revert or reformat each other's files.
- `claude-opus-5-5` for complex work (ports of reference logic, rendering, cross-crate design),
  `claude-haiku-5-5` for mechanical work (lint cleanup, doc updates, simple searches).
- Agents never run state-changing git commands (`git status`, `diff` and `log` are fine). An agent reports
  what it built, what it verified and how, and what is open; the lead re-runs the gates and commits.

## Where the code came from (WebMonkeyBall, optional, read-only)

Provenance, for reading the code: which file of the reference each part was ported from. Nothing here has to
be consulted to change the game.

| Topic | Reference files | Here |
|---|---|---|
| LZSS | `src/lzs.ts`, `src/noclip/SuperMonkeyBall/AVLZ.ts` | `mb-assets/src/lzss.rs` |
| Stagedef, collision | `src/stage.ts`, `src/noclip/SuperMonkeyBall/Stagedef.ts`, `src/collision.ts` | `mb-stage`, `mb-engine/src/collision.rs` |
| Models, textures | `src/gma.ts`, `src/gx_displaylist.ts`, `src/tpl.ts`, `src/noclip/SuperMonkeyBall/{Gma,NaomiLib,AVTpl}.ts` | `mb-gma`, `mb-tpl` |
| Materials, lighting, scene | `src/noclip/SuperMonkeyBall/{Material,TevLayer,Shape,Model,Lighting,Render,World,AnimGroup,Background,StageInfo}.ts` | `mb-render`, `mb-app/src/{stage_scene,play_scene,stage_info,anim}.rs` |
| Backgrounds that move, the bonus wave | `src/noclip/SuperMonkeyBall/{Background,BonusWave}.ts` | `mb-app/src/backdrop.rs`, `mb-render/src/wave.rs` |
| Mirror floors | `src/noclip/Render.ts`, `src/noclip/SuperMonkeyBall/World.ts` (`getMirrorMode`, `getMirrorPlaneMatrix`, `prepareToRenderMirrors`) | `mb-app/src/mirror.rs`, `mb-render/src/{mirror.rs,mirror.wgsl}` |
| Physics, maths | `src/physics.ts`, `src/math.ts`, `src/math_lut.ts`, `src/world.ts`, `src/animation.ts`, `src/rng.ts` | `mb-engine` |
| Camera | `src/camera.ts` | `mb-engine/src/camera.rs` |
| Game flow | `src/game.ts`, `src/course.ts`, `src/constants.ts` | `mb-engine/src/sim.rs`, `mb-game`, `mb-app/src/{menu,play}.rs` |
| HUD, input | `src/hud.ts`, `src/input.ts` | `mb-app/src/{hud,text,input,ui}.rs` |
| Audio | `src/audio.ts`, `src/audio/smb1_sfx_ids.ts` | `mb-audio`, `mb-app/src/audio.rs` |
| Determinism | `src/determinism.ts`, `src/sim_hash.ts` | `mb-engine/src/{determinism,hash}.rs` |
| Particles, confetti, shadow | `src/effects.ts`, `src/stage.ts` (confetti), `src/noclip/SuperMonkeyBall/World.ts` (`drawEffects`, `drawConfetti`, `drawProjectedShadow`) | `mb-engine/src/effects.rs`, `mb-render/src/{instances,quads,effects}.rs`, `mb-app/src/particles.rs` |
| Result replays (dropped by decision, not ported) | `src/game.ts` (`startResultReplay`, `stepResultReplay`), `src/camera.ts` (`FALLOUT_REPLAY`, `GOAL_REPLAY`) | `docs/PARITY_PLAN.md`, "Dropped: result replays" |
| Not used (network, replay files, SMB2) | `src/netplay.ts`, `src/netcode_protocol.ts`, `src/rollback.ts`, `src/replay.ts`, `src/course_smb2.ts`, `src/course_mb2ws.ts`, `src/smb2_render.ts` | |

Format and porting notes: `docs/REFERENCE_NOTES.md`, `docs/STAGEDEF.md`, `docs/GMA.md`, `docs/TPL.md`,
`docs/ENGINE.md`, `docs/AUDIO.md`, `docs/GAMEPLAY.md`. The parity tests compare against the reference's output
on locally supplied content when a checkout is present; never commit that content or dumps of it.
