# State and direction

Super Monkey Ball 1 in Rust, fully offline. It began as a port of WebMonkeyBall (TypeScript, "the
reference"), reached parity with it in the agreed scope at 0.2.0, and stands on its own since. This file is
the entry point: what the project is for, what works and where it lives, what is unverified, what was
decided. How the crates fit together and what a new platform needs is in
[ARCHITECTURE.md](ARCHITECTURE.md); other details are in the files this one links to.

**State: version 0.2.0, released 2026-10-13** (tag `v0.2.0`), after the project owner tried the release
candidate by hand and confirmed it. The rule for the next release is the same (see "Decisions").
What each version contains: [CHANGELOG.md](../CHANGELOG.md).

**Since 0.2.0, unreleased** (branch `cleanup/multiplatform-base`): no change for a player. The result
replays were dropped from the scope, which ends the parity work; the simulation is recorded as golden hashes
so that the reference is not needed any more; the workspace is one crate per platform on a game library
without a window, compile-checked for Android; Rust edition 2024. The next direction is more platforms:
Android in a WebView and as a native app. No port has been started.

## Goal

Four things, in this order when they pull against each other.

1. **Feature complete in the agreed scope.** Everything WebMonkeyBall shows and plays for Super Monkey Ball 1
   is in the port: all effects, sounds and levels from, and only from, SMB1. Not in scope, by decision:
   multiplayer / netplay / lobby, the replay-file player, the instant replays after a fall-out and a goal,
   the map viewer, SMB2. What the original game has
   and the reference lacks is outside parity (list below).
2. **Portable between platforms.** Desktop and web browser now; Android (WebView, then native) later.
3. **Minimal footprint.** Download size, memory, storage and per-frame work are measured and held.
4. **Minimal engine complexity**, using only portable and basic features.

What each means as a rule that can be checked against the code:

| | Rule | Checked by |
|---|---|---|
| Complete | Nothing is left to port ("Against the reference" below). The simulation of every stage is held by recorded hashes, every other behaviour by a test on the real game states | `crates/mb-engine/tests/golden.rs`, `crates/mb-app/tests/gameplay.rs` |
| Portable | One renderer, `mb-render`, on `wgpu`. Every driver asks for `wgpu::Features::empty()` and for the limits of `mb_app::gpu::required_limits`: a GL adapter (WebGL2, OpenGL ES, desktop OpenGL) gets `downlevel_webgl2_defaults()`, the other backends their adapter's own. So the browser build and the OpenGL tests are what catch a step beyond WebGL2 | `scripts/web-e2e.ts` on WebGL2 (`?gl=1`); `the_gl_backend_draws_the_same` in `crates/mb-render/tests/{effects,materials,mirror}.rs` |
| Portable | No compute shader, no float colour target, no clip distance, no WebGPU-only feature. Targets are the frame's colour format and `Depth32Float` | reading `crates/mb-render/src/*.wgsl` and `renderer.rs`; the WebGL2 run above |
| Portable | The game runs on one thread: states, simulation, drawing. The only others are the audio device's callback on the desktop (`audio_cpal.rs`, it mixes and nothing else) and the browser's import worker (its own instance of the module, no shared memory). Nothing needs `SharedArrayBuffer` | the site runs under `python -m http.server`, which sends no COOP/COEP headers (`scripts/web-e2e.ts`) |
| Portable | Platform access goes through `mb_app::host` (`Caps`, `Command`) and the input seam (`mb_app::input::InputState`). The game library `mb-app` has no window and no event loop; platform crates appear only in the platform's crate: `winit`, `rfd`, `gilrs` in `mb-desktop`; `winit`, `web-sys`, `wasm-bindgen` in `mb-web`. `mb-app` keeps `cpal` and `pollster` for every native target (`audio_cpal.rs`, `gpu.rs`) and `winit`'s key codes behind a feature | the manifests; `cargo clippy -p mb-app --target aarch64-linux-android` in `cargo xtask check` compiles the library and everything under it for Android |
| Portable | The format, simulation and rules crates are std-only and free of GPU, window and browser code: `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio`, `mb-game` have no dependency, `mb-engine` only `mb-stage`, `mb-assets` only `ruzstd` | their `Cargo.toml` |
| Portable | Game files are read through `mb_app::content` and its file lists, saves through `mb_game::Storage` | `the_import_copies_exactly_what_the_game_reads`, `every_course_stage_prepares_and_loads_from_the_pack` (`crates/mb-app/tests/pack.rs`) |
| Footprint | The budgets below. They are recorded constants; a test or the build fails when one is passed | see the table |
| Complexity | Every third-party dependency has a one-line reason in its `Cargo.toml`. There are 5 in the shared crates (`wgpu`, `glam`, `bytemuck`, `web-time`, `ruzstd`), `cpal` and `pollster` on native targets, `winit` in both platform crates, `rfd` and `gilrs` on the desktop, and 5 more in the browser | `AGENTS.md`, hard rule 7 |
| Complexity | No new crate, pass, pipeline family or shader without a thing on the SMB1 disc that needs it. Four WGSL files: `shader.wgsl` (every material), `sprite.wgsl`, `quad.wgsl`, `mirror.wgsl` | "Candidates for simplification" below lists where this does not hold yet |
| Complexity | Prefer removing code to adding it. Developer and test support is behind `mb-app`'s `dev` feature and not in the product | `cargo clippy -p mb-desktop -- -D warnings` builds the product without it |

### Budgets

Counted, not timed, wherever a count exists. None had to be raised for 0.2.0. The column is the release's
figure; where the work since changed one, the open items say so. The plan they come from, with the 0.1.0
baseline, is [PARITY_PLAN.md](PARITY_PLAN.md); how the browser figures are taken is in
[WEB.md](WEB.md), "Measurements".

| What | Limit | 0.2.0 | Enforced by |
|---|---|---|---|
| wasm module | 0.1.0's 3 877 298 bytes + 250 000 = 4 127 298 | 4 016 134 bytes | `WASM_BUDGET_BYTES` in `xtask/src/main.rs`: `cargo xtask build-web` fails above it |
| Game pack (browser storage) | 382 files, 144 084 942 bytes, as 0.1.0 | the same | `the_import_copies_exactly_what_the_game_reads` (`crates/mb-app/tests/pack.rs`) |
| Draw calls, every course stage, fly-in and play | the recorded table `DRAWS`, and never above 0.1.0's count plus what `DRAWS_ADDED` names | 25 517 / 23 343 over the 109 stages (0.1.0: 38 960 / 35 048) | `every_stage_draws_what_it_did_in_0_1_0_outside_the_particles` (`crates/mb-app/tests/budgets.rs`) |
| Draw calls of particles and confetti | 12 | at most 11 by construction | `particles_add_at_most_twelve_draws_at_their_peak` |
| Draw calls of a mirror | the recorded table `MIRROR_DRAWS`; none without a mirror or with Reflections off; never more than the frame's own | 12 to 94 % on top of the frame ([GAMEPLAY.md](GAMEPLAY.md), "Mirror floors") | the same test, `with_reflections_off_the_mirror_stages_cost_what_they_did` |
| GPU objects created in a play frame | 0 | 0 | `no_gpu_object_is_created_while_a_stage_is_played`, `no_gpu_object_is_created_while_a_mirror_stage_is_played_or_after_a_resize` |
| Heap allocations in a simulation tick | 0 | 0 | `stepping_with_particles_allocates_nothing` (`crates/mb-engine/tests/effects.rs`) |
| Texture bytes, heaviest stage (109) | 0.1.0's 16 617 412 + 1 000 000 | 16.7 MB | `the_heaviest_stage_uploads_no_more_texture_bytes_than_its_budget`, `flipbook_stages_upload_less_than_the_heaviest_stage_may` |
| Heap while a stage loads and plays (stages 1, 21, 76, 89, 90, 92, 109, OpenGL backend) | 0.1.0's figure for the stage + 2 000 000 bytes, at the peak and at rest | peaks 0.76 MB below to 0.29 MB above 0.1.0 | `loading_and_playing_a_stage_peaks_within_two_megabytes_of_0_1_0` (`crates/mb-app/tests/memory.rs`) |
| Uploads staged at once during a load | `Renderer::STAGED_LIMIT` (1 MB), or one texture level | held | the same test |
| wasm memory in the browser | 0.1.0's figure + 2 MB | 19.1 MB with one stage loaded, 23.5 MB after several (0.1.0: 22 to 28 MB) | `scripts/web-e2e.ts` |
| Frame time, WebGL2, 6x CPU throttle | update + render within 1.5 times 0.1.0; the mirror stages 89, 76, 21 within 16.6 ms at the 99th percentile, Reflections on and off | 3.2 to 3.4 ms on Beginner 1 (0.1.0: 5.3 ms); 9.4 to 12.5 ms on the mirror stages with Reflections on | `scripts/web-e2e.ts` |
| Saves | the format reads what 0.1.0 wrote | Reflections on from such a save | `a_save_from_0_1_0_loads_with_reflections_on` (`crates/mb-game/src/progress.rs`), `scripts/web-e2e.ts` |

A budget is changed on purpose only: in the same commit as its cause, with the reason in the message and
the new figure here and in the file that measures it.

## Targets

| Target | Built from | Entry | State |
|---|---|---|---|
| Desktop executable | `crates/mb-desktop` (binary `mb-app`) | `cargo run -p mb-desktop --release` | works; Windows built and run, Linux and macOS never built |
| Static web site (WebAssembly) | `crates/mb-web` + `web/` | `cargo xtask build-web`, output `web/dist/` | works; Chrome 154 on Windows run, nothing else |
| Android, the site in a WebView | `web/dist` in a host app somebody writes | none here | **later**; described in [ANDROID_WEBVIEW.md](ANDROID_WEBVIEW.md), never run |
| Android, native (Vulkan, GLES as fallback) | `mb-app` and below, plus a driver crate that does not exist | none | **later; not started.** The library compiles for `aarch64-linux-android`; nothing has run |

The site is plain files with relative paths, no server-side component and no dependency on a service
worker, with WebGL2 as a first-class backend. It must keep working under a static server no more capable
than `python -m http.server`. A phone's browser
opens the same site; that is how the game reaches a phone today. The same files can be shown in an Android
`WebView` by someone else ([ANDROID_WEBVIEW.md](ANDROID_WEBVIEW.md)).

Both targets worked in 0.1.0 and neither may regress.

**Keeping the ground for more platforms** means, for every change made now
([ARCHITECTURE.md](ARCHITECTURE.md) has the driver contract, a table of what each platform needs and the
list of what a port will meet):

* nothing in the renderer beyond what OpenGL ES 3.0 offers, which is what the WebGL2 rule already holds;
* no call to a platform from a game state: `host::Command`s and `InputState` only, so a third driver beside
  `mb-desktop` and `mb-web` is a new crate and not a change to the states;
* code for one platform lives in that platform's crate. `mb-app` must keep compiling for Android, which
  `cargo xtask check` holds: an Android driver replaces the file dialog and the gamepad source and reuses
  the audio output;
* game files only through `Content` and its lists, saves only through `Storage`, so the app can hand over a
  file descriptor and a private directory;
* every screen and option usable by touch alone, in portrait and landscape, with safe-area insets;
* no thread, timer or exit hook the game depends on: saves are written when they happen, a frame is the
  only unit of work.

Nobody adds the Android target, or any other, without the owner asking for it.

## What works

| What | Where | Tests |
|---|---|---|
| The user's own disc image: RVZ (none, Zstandard), ISO / GCM; USA tested, Europe and Japan accepted by id. Read in place on the desktop; in the browser the 144 MB the game uses are copied once into one pack, or served from the picked file where the page cannot store | `mb-assets` (`rvz.rs`, `disc.rs`, `gamepack.rs`), `mb-app/src/{content,import}.rs`, `mb-web` | `mb-assets/tests/disc.rs`, `mb-app/tests/pack.rs`, `scripts/web-e2e.ts` |
| Formats: stagedef, TPL, GMA and NaomiLib, ADP music, MusyX banks; corrupt input never panics or over-allocates | `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio` | each crate's `tests/disc.rs`, and `tests/{golden,robust}.rs` in the three parsers |
| Simulation at 60 Hz, bit-identical to the reference on every stage of the USA disc for the scripted runs tried: physics, collision, stage objects, camera, fly-in, goal, fall-out, time over, bonus stages | `mb-engine` | `tests/golden.rs` (every stagedef against hashes recorded where the comparison with the reference last passed), `tests/synthetic.rs`, `tests/robust.rs` ([ENGINE.md](ENGINE.md)) |
| Ball effects (sparks, collision stars, flash, post-goal sparkle) and goal confetti as display-only state on the reference's visual random stream; `Sim::hash` unchanged, `Sim::hash_visual` added | `mb-engine/src/effects.rs` | `effects_match_reference`, `tests/effects.rs` |
| The action button: fly-in at double speed on a first attempt, a fall-out cut short from its 60th tick | `mb-engine/src/sim.rs` (`Sim::step_with_buttons`), `mb-app/src/{input,play}.rs` | `intro_fast_forward_matches_reference`, `action_in_sequences_matches_reference`; `the_action_button_fast_forwards_the_first_fly_in_only`, `the_action_button_cuts_a_fall_out_short_at_the_same_cost` (`mb-app/tests/gameplay.rs`) |
| Courses (Beginner, Advanced, Expert, the Extra courses, Master), Practice, lives, score, continues, unlocks, settings; saves in two checksummed slots that survive a kill at any moment | `mb-game` | its unit tests and `tests/robust.rs`; `mb-app/tests/{rules,kill,pause}.rs` |
| Rendering of stages, backgrounds, ball, bananas, goals: GX materials as the reference draws them (world and view specular, texture matrices, blend factors), meshes merged per model | `mb-render` (`renderer.rs`, `material.rs`, `shader.wgsl`) | `mb-render/tests/materials.rs`; `every_course_stage_loads_and_renders` ([GMA.md](GMA.md)) |
| Particles, confetti and the ball's shadow in the game | `mb-render/src/{instances,quads,effects}.rs`, `mb-app/src/{particles,play_scene}.rs` | `mb-render/tests/effects.rs`; `particles_and_the_shadow_are_drawn_after_a_collision_and_a_goal`, `the_shadow_darkens_the_floor_beside_the_ball_from_every_side` |
| Backgrounds that move: night windows, storm fires, sunset clouds; stage 92's waved floor | `mb-app/src/{backdrop,stage_scene}.rs`, `mb-render/src/wave.rs` | `night_windows_flip_through_their_frames_with_the_stage_timer`, `storm_fires_flip_and_face_the_camera`, `sunset_clouds_drift_and_hurry_with_the_clock`, `the_bonus_wave_floor_ripples_under_the_ball` |
| Mirror floors on the 37 course stages the reference mirrors; Options -> Reflections, on by default | `mb-app/src/mirror.rs` (which stage, which plane), `stage_scene.rs`, `play_scene.rs`; `mb-render/src/mirror.rs`, `mirror.wgsl` | `mb-app/tests/mirrors.rs`, five mirror tests in `gameplay.rs`, `mirror_floors_load_from_the_pack`; `mb-render/tests/mirror.rs` |
| HUD as `hud.ts`: timer, score, bananas, speed, floor, the banners, `JUMP TO FLOOR n`, `WARP BONUS X n`, the `GOAL` flash | `mb-app/src/{hud,text}.rs` | unit tests in `hud.rs`; `a_green_goal_announces_the_floor_it_jumps_to` |
| Title menu, Course Play, Practice, Options, pause and in-place retry; a stage that fails to load returns to the menu | `mb-app/src/{menu,loading,play,widgets,ui}.rs` | `mb-app/tests/{gameplay,pack,pause}.rs` ([GAMEPLAY.md](GAMEPLAY.md)) |
| Sound effects and stage music from the disc's own banks and streams; cpal on the desktop, Web Audio in the browser | `mb-audio`, `mb-app/src/{audio,audio_cpal}.rs`, `mb-web/src/audio.rs` | `mb-audio`'s tests, `mb-app/tests/audio.rs` ([AUDIO.md](AUDIO.md)) |
| Keyboard, gamepad, mouse, touch (on-screen stick, touch-only menus), device tilt | `mb-app/src/{input,ui,tilt}.rs`, the drivers `mb-desktop/src/app.rs` and `mb-web/src/shell.rs` | `analog_stick_and_pointer_drive_the_same_states`; `scripts/web-e2e.ts` (touch at 412 x 915, an emulated sensor) |
| The static site; storage the user can see and erase | `mb-web`, `web/`, `xtask` | `scripts/web-e2e.ts`, `cargo test -p xtask` ([WEB.md](WEB.md)) |

Last run of everything, 2026-10-13 on the branch described under "State", with the USA disc (Windows 11):
`cargo xtask check` all gates, `cargo test --workspace --release` 547 passed, 0 failed, 5 ignored (0.2.0:
559; 0.1.0: 406; fewer since the tests against the reference and of removed code went);
`deno run -A scripts/web-e2e.ts` 93 checks in Chrome 154 with WebGPU available (0.2.0: 97, four were the
single file's), about eight minutes. Without the disc the disc-backed tests skip.

## Against the reference

The parity work is finished. This section is what it found, kept as the record of where the game is and is
not what WebMonkeyBall is. Compared by reading the reference's `game.ts`, `camera.ts`, `effects.ts`, `hud.ts`, `audio.ts`,
`noclip/SuperMonkeyBall/{World,Background,Material}.ts` and `noclip/Render.ts` against the port. The
reference was not run in a browser beside it.

### Left, in the agreed scope

Nothing. The one item that was left, the instant replays after a fall-out and after a goal, was dropped by
the owner on 2026-10-13 ([PARITY_PLAN.md](PARITY_PLAN.md), "Dropped: result replays") and is now under
"Out of scope by decision". Nothing else was found: `hud.ts`, `audio.ts`, `Background.ts` (only the night, storm and sunset classes do
anything) and `World.ts` have no SMB1, single-player behaviour without a counterpart here. The one line of
the reference's HUD that is not drawn is its netplay round-trip time, which is multiplayer.

### Deliberate differences

Places where the port does not do what the reference does, on purpose. Each is described once, with its
reason, in the file named; this is the index.

| Difference | Reason given in |
|---|---|
| Stage 92's mirror has no distortion offset; stage 103's mirror plane is the plane of its mirror model; stage 74 has a mirror; the plane of every mirror comes from its models; Reflections is an option | [GAMEPLAY.md](GAMEPLAY.md), "Mirror floors" |
| The mirror's cut at the plane, target size and format, the wave floor in its own reflection, no capture when no mirror model is in view | [GMA.md](GMA.md), "Mirror floors" |
| Particles are batched and drawn after the sorted objects; the sparkle uses the disc's `CROSS_LIGHT` texture; streak tails follow simulated time; 512 streaks and 928 instances a frame; the collision flash ported as it is, under the floor; no shadow on a fragment its material discards | [GAMEPLAY.md](GAMEPLAY.md), "Shadow and particles" |
| The effect pool is capped at 1024; effect ids restart with every `Sim` | [ENGINE.md](ENGINE.md), "Particles (display only)" |
| A fall-out and a goal are shown once, without the reference's instant replay; the action cannot end a goal sequence; the confetti lives only for the goal sequence | [PARITY_PLAN.md](PARITY_PLAN.md), "Dropped: result replays"; [ENGINE.md](ENGINE.md), "Not modelled" |
| Sunset clouds scroll visibly, from a seeded generator, hurried by the stage's own clock; flipbook windows drawn first among the blended meshes | [GAMEPLAY.md](GAMEPLAY.md), "Backgrounds and the bonus wave" |
| `JUMP TO FLOOR n` names the destination floor; the line and its trails use their opacities; no jump line in practice; `FLOOR SCORE` on a warp goal shows a multiplier nobody awards (as in the reference) | [GAMEPLAY.md](GAMEPLAY.md), "HUD" |
| The ball's shells are sorted by a thousandth of the reference's depth offsets; a stage model's blended meshes are sorted | [GAMEPLAY.md](GAMEPLAY.md), "Material switches" |
| Vertex-colour materials take the object's alpha and colour; the NaomiLib V coordinate is `1 - t` (`-t` only under a mirrored wrap); opaque meshes sorted by state and merged per model | [GMA.md](GMA.md), "How `mb-render` draws the materials" |
| Lives are shown, the HUD is anchored to the screen's edges, tall screens get a taller field of view, bumpers show the simulation's squash | [GAMEPLAY.md](GAMEPLAY.md), "Not ported from the reference" |
| Every sound is the disc's own sample, found through the executable's tables; the announcer's `GO` has one take | [AUDIO.md](AUDIO.md) |

### In the original game, not in the reference: outside parity

The monkey inside the ball and its voices; the softest impact tier, the time-over explosion, the party-ball
and goal-replay sounds; 1UP, continue and game-over lines, jingles, menu music; the envelopes of the MusyX
macros; the timing of the `FALL OUT` call; awarding the warp bonus; the minigames. The sound items are
listed with their ids in [AUDIO.md](AUDIO.md), "Known gaps".

The port already has a few things of the original that the reference lacks, because a game without them
cannot be played through: the clock tick and the menu sounds ([AUDIO.md](AUDIO.md)), and game over, five
continues and a 1UP per 100 bananas (`mb-game/src/run.rs`; see "Continue rule" under "Open items").

### Out of scope by decision

Multiplayer, netplay, lobby, rollback, spectating and the chained mode; the replay-file player (`replay.ts`);
the instant replays after a fall-out and a goal (`startResultReplay`, the `FALLOUT_REPLAY` and `GOAL_REPLAY`
cameras), with the goal-sequence skip and the rest of the confetti's life that only exist with them;
the map viewer and its free camera; SMB2 (stagedefs, courses, switches, wormholes, texture scroll, fog).

## Unverified

Nothing below has been done; each is a statement about where the evidence stops.

* **A person playing 0.2.0.** How the effects, mirrors and materials look, how it sounds, a course played to
  its end: the project owner tried the release candidate by hand before the tag; no written record of what
  was looked at exists.
* **Devices and browsers.** A real phone, an Android web view, iOS, Firefox, Safari, a real gamepad. Only
  Chrome 154 on Windows 11 (ANGLE on Direct3D 11, an RTX 4080) and the Windows desktop program were run.
* **Linux and macOS** desktop builds, and both CI workflows, have never run.
* **Android.** The game library compiles for `aarch64-linux-android` and that is all: no driver exists,
  nothing was linked, packaged or run. The site in a WebView was never run either.
* **Another architecture.** The simulation has run on x86-64 and wasm32. That ARM computes the same bits is
  expected and unverified ([ARCHITECTURE.md](ARCHITECTURE.md), "What will cause trouble").
* **The Europe and Japan discs** are accepted by id and untested.
* **A 60 Hz display.** The frame-time limits of the browser check were exercised at 165 Hz (0.1.0) and
  240 Hz (0.2.0); they scale with the display's frame, which was not seen doing so at 60 Hz.
* **A phone's GPU.** The CPU throttle slows the page's main thread only; what filling the mirror's second
  target costs a phone is not measured.

## Open items

Small things known to be unfinished or resting on thin evidence. None blocks play on the tested setups.

| Item | State |
|---|---|
| Sound has never been listened to | Verified by signal measurement only (levels, no clipping, the right requests at the right times). Balance, and whether each sample is the right one, need ears. |
| Material fidelity has no reference picture | The reference needs a browser and the game data, so the materials of 0.2.0 are held by unit tests of the mapping (`mb-render/tests/materials.rs`) and by screenshots no person has compared with the original. |
| WebGL2 fences set to finish | `GlFenceBehavior::AutoFinish` (`mb_app::gpu::instance_descriptor`) is what keeps a load's memory down. Verified in Chrome 154 on Windows only (no driver warning, pacing unchanged); a blocking finish may cost or be reported differently in Firefox, Safari and Android WebView ([WEB.md](WEB.md), "What the browser check sees of the new drawing"). |
| Merged meshes are not pixel-exact | Against the commit before the merge, 16 of 112 compared frames differ in one to four pixels of 230 400, where two unblended meshes of one model lie in one plane ([GMA.md](GMA.md), "Meshes, uploads and memory"). Also, an object faded with `set_alpha` blends its otherwise unblended meshes, and the triangles of a merged mesh are then drawn in file order rather than sorted among themselves; no picture of a faded background object was examined for it. |
| Stage 110's two mirror levels | The switch between the floors is tested with cameras placed above and between them (`a_stage_with_two_mirror_floors_mirrors_in_the_one_under_the_camera`), not by a ball falling from one to the other in play. |
| Timing limits in the browser check | Exercised at 165 and 240 Hz only (see "Unverified"). Long-frame counts are small numbers and are held to three times 0.1.0's share for that reason. |
| Engine parity, parts not compared | The bonus-stage branch of the fall-out rule and the bonus clear (the headless reference has no course), and confetti from a goal on a moving anim group. `tests/synthetic.rs` covers the first two without the reference ([ENGINE.md](ENGINE.md), "Tests"). |
| wasm size | 4 011 275 bytes after the slimming since 0.2.0 (4 016 134 then); 116 023 bytes of the budget remain. |
| Licence | `Cargo.toml` declares `MIT OR Apache-2.0`; there is no `LICENSE` file, and the reference this code was ported from carries no licence statement. For the owner to settle before publishing more widely. |
| `rust-version` | 1.88 is what clippy's MSRV lint accepts; only the current stable toolchain has built the code. |
| Figures not taken again for 0.2.0 | JS heap, the browser's memory during the import, stage load times in the browser, sound effect bytes ([WEB.md](WEB.md), "Measurements": the rows that say so). |
| Phones | Nothing has run on a real phone. The project owner tried the 0.1.0 site on their own phone (Firefox for Android among the browsers); the touch, tilt and file-chooser fixes came from those reports and were not confirmed there afterwards. |
| File chooser on phones | The input asks for `application/octet-stream` so Firefox for Android stops asking for camera and "record audio" permission (`web/main.js`, "picking the file"). Unconfirmed on a device; a file manager that reports the image under another type would grey it out, hence the "show all files" button. |
| Tilt steering | Recentres at stage load, at GO and from the on-screen crosshair button; the stick is hidden while tilt steers. If the device never delivers sensor readings (permission denied on iOS) there is no stick to fall back on: the player has to switch Options -> Controls back. Showing the stick again when no readings arrive would fix it (`mb-app/src/play.rs` `tilt_steering`, `mb-web` tilt feed). |
| Continue rule | "Continue resets the score to 0 and keeps the banana counter" (`mb-game/src/run.rs`) comes from memory of the original, not from a source. Game over, 5 continues and a 1UP per 100 bananas likewise: the reference has none of them. |
| Largest music track | About 54 MB of float samples in the browser (computed, not measured). 16-bit or segment-streamed music would cut it; not done because a sample-exact loop could not be verified. |
| Desktop: dropping the image on the window | Not implemented (dialog and command line only). |
| Pack version | `mb_assets::gamepack::VERSION` must be raised when the list of copied files changes. `tests/pack.rs` fails when the count or the bytes change, which is the reminder; nothing checks that the version was raised with them. |
| CI workflows | `.github/workflows/ci.yml` (manual only) runs `cargo xtask check`. `release.yml` starts when a release is published on GitHub: it checks that the tag is `v` + the version in `Cargo.toml`, runs `cargo xtask check`, then attaches the Linux, Windows and Apple-silicon executables and the site as an archive to the release. Neither has ever run, and their Linux package list is unverified; the first released Linux and macOS executables are unverified (the macOS one is also unsigned). |
| 32-bit overflow fixes in the parsers | Made by inspection; no 32-bit test runner was available (the browser check exercises wasm32 on valid data only). |

## Decisions (recorded)

- **Standalone (2026-10-13):** the reference is dropped as a requirement. Parity was reached and recorded
  (`mb-engine/tests/golden.rs`, the golden tests of the parsers); from here a change of behaviour is a
  decision of this project. The credits stay.
- **One crate per platform (2026-10-13):** `mb-app` is the game as a library with no window; `mb-desktop`
  and `mb-web` are the drivers; the next platform is another crate beside them. Shared code is
  compile-checked for Android in the gates. Rust edition 2024.
- **Scope:** everything WebMonkeyBall has for Super Monkey Ball 1, from and only from SMB1. Not multiplayer /
  netplay / lobby, not the replay-file player, not the map viewer, not SMB2, and (decided 2026-10-13) not
  the instant replays after a fall-out and a goal: the game is in a good enough state without them. What only the original has
  (the monkey, its voices, jingles, menu music) is not added.
- **Versions:** 0.2.0 is phases 1 to 4 of [PARITY_PLAN.md](PARITY_PLAN.md) (effects, small items, materials,
  mirrors), and the end of that plan; phase 5 (result replays) was dropped.
- **Release gate:** no release tag and no push before the project owner has tested the release candidate by
  hand and confirmed. Until then the version is "the 0.2.0 release candidate".
- **Targets:** the desktop program and the static site now; Android later, in a WebView and natively, which
  current work must not block (see "Targets"). Nobody starts a port without the owner asking.
- **Reflections are on by default on every platform.** A player whose device is too slow turns them off
  (Options -> Reflections); the choice is saved.
- **Budgets are measured constants enforced by tests.** They may be raised slightly when necessary, but
  only with a measurement and on purpose; none was raised for 0.2.0.
- **Graphics:** `wgpu`, one API for desktop and browser; nothing beyond `downlevel_webgl2_defaults`.
- **Windowing / input:** `winit` for the desktop window and the browser canvas. Two drivers on top of it,
  `mb-desktop/src/app.rs` and `mb-web/src/shell.rs`; they differ in pacing, GPU setup and devices and share
  everything from the state stack up (`shell.rs` explains the split). Whether a third (Android) joins them
  or all three become one is open ([ARCHITECTURE.md](ARCHITECTURE.md), "What will cause trouble", 2).
- **Disc reading:** the project's own readers in `mb-assets` on every target: `rvz.rs` (RVZ, none and
  Zstandard via `ruzstd`) and `disc.rs` (GameCube header, file table, `main.dol`), plain ISO/GCM included.
  What they return for the USA disc is held by recorded hashes (`mb-assets/tests/disc.rs`). Other containers are refused with
  a message that says what the file is and what is supported.
- **Browser:** `wasm-bindgen` + `web-sys`, built with `wasm-bindgen-cli --target no-modules` (classic
  scripts, so the same glue runs in the page and in the import worker). The disc
  image is never held in memory: a worker copies the files the game uses into one pack in the Origin Private
  File System (or serves them from the picked file where that is unavailable); the game prepares files
  asynchronously before each state that needs them ([WEB.md](WEB.md)). rustc's default WebAssembly feature
  set is used as it is; the browser floor is Chrome / WebView 97, Safari 16, Firefox 79.
- **No fallback without workers:** a browser without dedicated workers or `FileReaderSync` gets an
  "unsupported" message; a second, main-thread reader is not kept for it.
- **Saves:** two checksummed slots per value (`mb_game::SlotStorage`) over files (desktop), the private file
  system or `localStorage` (browser): crash-safe without relying on an atomic rename.
- **Hosting:** any static server; the reference is `python -m http.server`. There is no project web server.
- **Parsers** (`mb-tpl`, `mb-gma`, `mb-stage`) and `mb-audio`, `mb-game`, `mb-engine` are std-only.
- **Audio:** `mb-audio` decodes and mixes with no output dependency; device output is `cpal` on the desktop
  and Web Audio in the browser, where the browser mixes and music is decoded straight into its buffers
  ([AUDIO.md](AUDIO.md)).
- **Effects never write into the simulation.** They draw from the reference's separate visual random stream
  and are outside `Sim::hash`.
- **Developer support stays out of the product:** the windowless driver, the autopilot and test doubles are
  behind `mb-app`'s `dev` feature, which only `cargo test` and an explicit `--features dev` switch on.
- **Strictly offline:** no multiplayer, lobbies, netplay, rollback, telemetry, update checks or online
  leaderboards, and no networking crates. Deterministic simulation and state hashing stay (tests).

## Candidates for simplification

What 0.2.0 added, read against goal 4. Nothing here is a fault in what the player sees; each is machinery
that is larger than its effect, duplicates something, or serves nothing on this disc. It is for the owner to
decide. Done since the list was written (the rows are gone from the table): fog, the wavy mirror's
distortion, additive sprites and the world-specular switch removed; the tests' disc finder shared within
`mb-app`. Outside this list the single-file page, the parity harness against the reference and the `nod`
dependency went too.
[ARCHITECTURE.md](ARCHITECTURE.md), "The elements, by importance", ranks everything in the repository, not
only what 0.2.0 added. Sizes are lines of the file as 0.2.0 had it.

| Candidate | Where | What could go, and what that costs |
|---|---|---|
| Two kinds of mirror surface | `Renderer::set_mirror_surface` and `set_mirror_surface_only` (`renderer.rs`, 2998) | The first (a surface drawn with its own material and the reflection over it) has one user, stage 92's wave floor; the 36 other stages use the second. And a third way hides a surface: stage 110 switches its two levels with `set_visible` (`play_scene.rs`, `pose_mirror`). One flag with a "draw own material" bit would do. |
| Two ways to order blended objects | `Renderer::set_sort_bias` (ball shells, goal bags) and `set_depth_offset` (background and foreground objects, flipbook windows) | Both add a number to a sort distance; they differ in whether opaque meshes move to the sorted pass and whether the object's or the mesh's origin is used. One call with those two switches would carry both. |
| An instanced path beside the object path | `mb-render/src/instances.rs` (174), `effects.rs` (264), the instance pipelines in `renderer.rs`; against objects with a shared uniform buffer | Instances exist for the confetti (160 pieces), stars, glows and flashes. They are what keeps particles at 11 draws, so they earn their place on WebGL2; the cost is a second vertex layout, a second set of pipelines and the specular difference noted in [GMA.md](GMA.md). Listed for completeness, not as a recommendation. |
| Renderer calls that only tests use | `set_mirror_capture_whole` (one test in `mb-app/tests/gameplay.rs`), `set_mirror_scale` (the game always uses the default 0.5), `set_color_mul` and `set_tex_matrix` (the game calls `set_tex_offset`), `mirror_size`, `object_capacity` | Each is small. `set_mirror_capture_whole` exists to prove the scissored capture changes no pixel; it could be a `#[cfg]`-gated hook or the comparison could be dropped once trusted. |
| A texture matrix where an offset is used | `set_tex_matrix`, `set_tex_matrix_every_layer`, the `ENABLE_UV_SCROLL` branch of `shader.wgsl` (430) | No layer on the disc has the flag ([GMA.md](GMA.md)); the only user is the sunset clouds, through `set_tex_offset` and the every-layer switch. A two-float offset per object would be enough. |
| Four draw-count tables | `crates/mb-app/tests/budgets.rs` (737): `DRAWS_0_1_0`, `DRAWS_ADDED`, `DRAWS`, `MIRROR_DRAWS` | `DRAWS` is what is asserted exactly; the first two only prove it never exceeds 0.1.0, which the merged meshes put 15 to 50 % out of reach. They could be reduced to `DRAWS` and `MIRROR_DRAWS` once 0.1.0 stops being the baseline. |
| Accessors for tests in product code | `hud.rs` (`jump_text`, the warp factor), `play_scene.rs` (shadow drawn, cloud layers, mirror level), `menu.rs` (the run in progress); marked "(tests)" in their comments | A few lines each; they could sit behind the `dev` feature like the rest of the test support. |
| Frame counters in the shipped wasm | `mb-app/src/perf.rs` (239), `perf_*` exports of `mb-web/src/lib.rs`, `window.mbPerf` in `web/main.js` | They are what the browser check reads and cost nothing until enabled, but they are developer support inside the product, which `AGENTS.md` otherwise forbids. Kept because the check must measure the build that ships. |
| Developer helpers that grew | `mb-app/src/headless.rs` (365), `autopilot.rs` (222), `image.rs` (120), all behind `dev`; `mb-app/tests/gameplay.rs` (1975), `mb-render/tests/mirror.rs` (1542) | Not in the product. The test files are now longer than most of what they test; starting a stage is still set up separately in `budgets.rs`, `memory.rs`, `mirrors.rs`, `gameplay.rs` and `pack.rs` (finding the disc is shared since, `tests/common/mod.rs`). |
| `renderer.rs` itself | 2998 lines (0.1.0: 1633) | One file holds uploads, mesh merging, objects, sorting, culling, the mirror passes and the frame. Splitting changes no behaviour; listed because it is where the next feature will be hardest to add simply. |
| Mirror planes derived instead of looked up | `mb-app/src/mirror.rs` (675) | The reference has one short function of fixed choices (`getMirrorPlaneMatrix`); the port derives every plane from the mirror models (flatness, shared planes over the animation loop, levels). It is why stages 103 and 74 are right, and it is the largest piece of logic the mirrors added to `mb-app`. `tests/mirrors.rs` already records the resulting plane of every stage to check the rule; reading the planes from such a table would be much shorter, at the price of numbers that nothing derives any more. |
