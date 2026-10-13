# Plan: parity with WebMonkeyBall (SMB1)

Written 2026-10-08 against commit `a1f1575` (version 0.1.0); brought in line with what happened on
2026-10-13, when phases 1 to 4 were done. This file is the record of the plan: its phases, limits, budgets
and checkpoints. The current state, what is left and the direction are in [PLAN.md](PLAN.md) and are not
repeated here.

## Goal

Everything WebMonkeyBall shows and plays for Super Monkey Ball 1 is in this port: all effects, sounds and
levels. Still out: multiplayer, the replay-file player, the map viewer, SMB2.

The targets of this work are the two that exist and worked in 0.1.0: the desktop program and the static web
site (which a phone's browser can open). Neither may regress. An Android app is a later goal and is not
built here; this plan only makes sure nothing done now stands in its way (see "Keeping Android possible").

Levels, physics and sound were at parity in 0.1.0. The work is visual (phases 1 to 4) and one gameplay
item (phase 5).

| Phase | What | State | Checkpoint |
|---|---|---|---|
| 0 | Baseline of 0.1.0 | done | `checkpoint/p0` (`88e43e3`) |
| 1 | Particles, confetti, shadow; the budget tests | done | `checkpoint/p1` (`b2f2f5e`) |
| 2 | The small items | done | `checkpoint/p2` (`7d2a8de`) |
| 3 | Material fidelity | done | `checkpoint/p3` (the same commit as p2) |
| 4 | Mirror floors | done | `checkpoint/p4` (`1417840`) |
| | Version 0.2.0: phases 1 to 4 | **released 2026-10-13**, after the owner's manual test | `v0.2.0` |
| 5 | Result replays | not started | version 0.3.0, `checkpoint/p5` |

## Decisions this plan rests on

| | Question | Decided |
|---|---|---|
| D1 | Port the fall-out and goal instant replays? | yes, in 0.3.0 (phase 5) |
| D2 | Add what the original has and the reference lacks (monkey in the ball, its voices, jingles, menu music)? | no; out of scope |
| D3 | Push branches and tags to `origin`? | no, local only; `v0.2.0` is made and anything pushed only after the owner has tested the release candidate by hand and confirmed |
| D4 | Which version is which? | 0.2.0 is phases 1 to 4; phase 5 is 0.3.0 |
| D5 | Reflections on by default? | yes, on every platform; Options -> Reflections turns them off |
| D6 | May a budget be raised? | slightly, when necessary, and only with a measurement so that it does not creep; none was raised |

## No regressions

Version 0.1.0 is the baseline, tagged `v0.1.0` at `a1f1575`. Phase 0 recorded what it does (test count,
browser checks, draw calls, frame times, wasm size, memory) and every later phase was compared with that.

* All gates of `AGENTS.md` run before every merge into `main`: `cargo xtask check`,
  `cargo xtask build-web --single`, `deno run -A scripts/web-e2e.ts`, with the disc image.
* No existing test is weakened, skipped or deleted. A test that has to change because behaviour changed on
  purpose is changed in the same commit, and the commit message says why.
* The engine parity tests against the reference stay bit-identical. Effects never write into the simulation.
* Every new feature adds a test that drives the real states on both paths: the desktop path
  (`crates/mb-app/tests/gameplay.rs`) and the pack path the browser uses (`tests/pack.rs`), plus a check in
  `scripts/web-e2e.ts` where the browser can observe it.
* Every new visual can be absent without breaking a stage: a missing model or texture costs that decoration
  only, as the goal tape and clock digits do.

### Regression budgets as tests

Counted, not timed, wherever a count exists, so the tests do not flicker with the load of the machine.
Added in phase 1 against the 0.1.0 figures and extended by each phase. The column "As planned" is what
this file said on 2026-10-08; where the outcome differs the last column says how.

| What | As planned | Enforced by | Outcome in 0.2.0 |
|---|---|---|---|
| Draw calls | no stage draws more than its 0.1.0 count outside effects; effects and confetti add at most 12; mirrors only on stages 92, 102, 103 | `crates/mb-app/tests/budgets.rs`: `every_stage_draws_what_it_did_in_0_1_0_outside_the_particles` (tables `DRAWS_0_1_0`, `DRAWS_ADDED`, `DRAWS`, `MIRROR_DRAWS`), `particles_add_at_most_twelve_draws_at_their_peak` | Every stage draws fewer calls than in 0.1.0 (meshes merged per model): 25 517 / 23 343 over the 109 stages at ticks 60 and 420, against 38 960 / 35 048. Particles add at most 11. 22 stages draw something new outside the particles (the wave floor, night windows, storm fires), recorded in `DRAWS_ADDED`. **Mirrors are on 37 stages, not 3:** the reference mirrors every stage whose stagedef names mirror models; a mirror's draws are counted apart and never double a frame |
| Per-frame GPU work | 0 buffers, textures, pipelines or bind groups created in a play frame after the first | `no_gpu_object_is_created_while_a_stage_is_played`, `no_gpu_object_is_created_while_a_mirror_stage_is_played_or_after_a_resize` | 0, from the first frame of a stage (`Renderer::reserve_objects`) |
| Tick cost | 0 heap allocations per tick in steady play, effects included | `crates/mb-engine/tests/effects.rs`: `stepping_with_particles_allocates_nothing` | 0 |
| Frame time | reported; fails only beyond 1.5x the 0.1.0 figure | `scripts/web-e2e.ts`, WebGL2, 6x CPU throttle | Beginner 1: 3.2 to 3.4 ms of update and render against 5.3 ms. Added: the mirror stages 89, 76 and 21 must fit a 16.6 ms frame at the 99th percentile with Reflections on and off (9.4 to 12.5 ms on) |
| Memory | wasm memory after playing Beginner 1 and Expert 50 at most 2 MB over 0.1.0 (23 and 28 MB) | `scripts/web-e2e.ts`; natively `crates/mb-app/tests/memory.rs` (`loading_and_playing_a_stage_peaks_within_two_megabytes_of_0_1_0`) | 19.1 and 23.5 MB, below 0.1.0. The native test was added when the first mirror builds went over (up to 36.6 MB in the browser); uploads are now submitted in portions of `Renderer::STAGED_LIMIT` |
| Memory: GPU | texture bytes of the heaviest stage at most 1 MB over 0.1.0 (16.6 MB), mirror target excluded and reported | `the_heaviest_stage_uploads_no_more_texture_bytes_than_its_budget`, `flipbook_stages_upload_less_than_the_heaviest_stage_may`, `with_reflections_off_the_mirror_stages_cost_what_they_did` | 16.7 MB (65 532 bytes of particle textures). The mirror target is 2 bytes per pixel of the frame, reported by `Renderer::mirror_bytes` |
| Storage: game pack | 144.1 MB / 382 files; any growth is a deliberate, documented number with `gamepack::VERSION` raised | `crates/mb-app/tests/pack.rs`: `the_import_copies_exactly_what_the_game_reads` | unchanged: 382 files, 144 084 942 bytes. No file was added to the lists |
| Storage: site | wasm module at most 250 KB over 3 877 298 bytes | `WASM_BUDGET_BYTES` in `xtask/src/main.rs`; `cargo xtask build-web` fails above it | 4 016 134 bytes: +138 836, 111 164 left |
| Storage: saves | format unchanged; a 0.1.0 save still loads | `a_save_from_0_1_0_loads_with_reflections_on` (`crates/mb-game/src/progress.rs`); a 0.1.0 save in `scripts/web-e2e.ts` | one new `setting` line (`reflections`); a 0.1.0 save loads with it on, 0.1.0 skips the line |

## Keeping Android possible

The later Android app will most likely show the static site in a WebView (`docs/ANDROID_WEBVIEW.md`), or
be a native build of the same crates. Either works only if every phase keeps to the following; they are
already rules of `AGENTS.md`, restated because new rendering work is where they are easiest to break.

* WebGL2 stays a first-class backend: nothing beyond `downlevel_webgl2_defaults`, no WebGPU-only feature,
  no compute, no float render targets, no threads.
* The site stays plain relative files with no network request, and the single-file build keeps working.
* Platform access stays behind `mb_app::host` and the input seam; no state calls a browser or desktop API.
* The format, simulation and rules crates stay free of GPU, windowing and browser code.
* New files the game reads go through the file lists in `content.rs`, so an import stays complete.
* Touch-only use keeps working for every new screen or option.

Phases 1 to 4 kept to all six. What a native Android build would need beyond them is in
[PLAN.md](PLAN.md), "Targets".

## Limits every phase works inside

These come from the targets; WebGL2 in a phone's browser is the tightest of them.

| Limit | Rule |
|---|---|
| Graphics API | nothing beyond `wgpu::Limits::downlevel_webgl2_defaults` |
| Draw calls | the cost on WebGL2 is per call (baseline: 275 draws, 5.1 ms render at 6x CPU throttle, Beginner 1). Particles and confetti are instanced or batched, never one draw per particle: together they add at most 12 draws at their peak, and the shadow adds none (a term in the stage shader, not the reference's second geometry pass). A mirror adds one reduced scene pass, on the stages that have one and only while a mirror model is in view |
| Per-frame work | no buffer, texture, pipeline or bind-group creation during play; dynamic geometry goes into one preallocated buffer written once per frame; no heap allocation per tick or per frame in the effect code |
| Memory | fixed-capacity effect pools; wasm memory never shrinks, so nothing is decoded "just in case" and a load never stages more than about a megabyte of uploads at once. Budget: wasm memory in play grows by at most 2 MB over the 0.1.0 figure for the same stage |
| Threads | none (no `SharedArrayBuffer` under a plain static server or in the app) |
| Size | the wasm module (3 877 298 bytes in 0.1.0) grows by at most 250 000 bytes over all phases |
| Determinism | effects use the reference's separate visual RNG and stay out of `Sim::hash`; the parity tests keep passing bit for bit |
| Storage | a loader that reads a new file adds it to the lists in `content.rs` and raises `gamepack::VERSION` |
| Offline | no networking crate, no request outside the bundle |

Each phase ends by re-measuring draws, update and render time and wasm memory the way `docs/WEB.md`
describes, and records the numbers there. A phase that breaks a budget is not merged.

## Git checkpoints and workspaces

* `main` stays releasable. One branch per phase, `parity/p1-effects` and so on, merged into `main` with
  `--no-ff` when the gates are green, then an annotated tag `checkpoint/p1`. One logical change per commit.
* `CHANGELOG.md` and the matching `docs/` file are updated in the phase that changes them.
* Version 0.2.0 is phases 1 to 4. Phase 5 is left for 0.3.0. With phase 4 merged the version is set and
  the release candidate is handed to the project owner to try by hand; the tag `v0.2.0` is made, and
  anything is pushed, only after the owner confirms.
* Cargo workspace: no new crate. Effect state goes into `mb-engine`, drawing into `mb-render`, assembly
  into `mb-app`, so the layering of `AGENTS.md` and the crate ownership of its subagent table hold.
* One checkout, as `AGENTS.md` says: subagents split by crate ownership, edit only what they own, run no
  state-changing git command; the lead re-runs the gates and commits. Git worktrees are not used: each
  would need its own multi-gigabyte `target/` and its own `MB_DISC`, and the crate split already keeps
  parallel agents out of each other's files.

## Subagents

* Every brief starts with the reading list: `CLAUDE.md`, `AGENTS.md` (all of it), `docs/PLAN.md`, this
  file, the `docs/` files of the crates the agent owns (`ENGINE.md`, `GMA.md`, `TPL.md`, `STAGEDEF.md`,
  `GAMEPLAY.md`, `AUDIO.md`, `WEB.md`, `REFERENCE_NOTES.md` as they apply) and the reference files of its
  task. The agent confirms in its report what it read.
* Model by complexity, as `AGENTS.md` sets it: `claude-opus-5-5` for ports of reference logic, rendering
  and cross-crate design; `claude-haiku-5-5` for mechanical work (lint cleanup, doc updates, searches).
* Shared types between two agents are fixed in both briefs before either starts.
* A report says what was built, what was verified and how, and what is open. The lead reads the diff,
  re-runs the gates and only then commits.

## Phase 0: baseline (done)

Measured 2026-10-08 on `1ec223f` (Windows 11, USA disc):

| | |
|---|---|
| `cargo xtask check` | all gates passed; 406 tests passed, 0 failed, 2 ignored |
| `deno run -A scripts/web-e2e.ts` | all 60 checks passed |
| `pkg/mb_web_bg.wasm` | 3 877 298 bytes; single file 2 107 204 bytes |
| Draw calls, frame times, wasm memory | the figures of `docs/WEB.md` for 0.1.0 (275 draws, update 0.16 ms + render 5.1 ms at 6x CPU throttle on WebGL2, Beginner 1; 22 to 28 MB wasm memory). Later phases measured a build of the `v0.1.0` tag again, per stage, for the tables in `tests/budgets.rs`, `tests/memory.rs` and `docs/WEB.md` |

## Phase 1: particles, confetti, shadow (done)

Reference: `effects.ts`, `stage.ts` (confetti), `physics.ts` (spawn calls), `World.ts` `drawEffects`,
`drawConfetti`, `drawProjectedShadow`.

| Step | Crate | What | Verified by |
|---|---|---|---|
| 1a | `mb-render` | instanced drawing of one model at many transforms and colours (`instances.rs`); one dynamic quad batch (`quads.rs`, `quad.wgsl`); the projected ball shadow as a term of the stage shader | `crates/mb-render/tests/effects.rs` |
| 1b | `mb-engine` | port of `effects.ts` and of the confetti (`effects.rs`); visual RNG seeded as the reference; fixed pools; not hashed | `effects_match_reference` (the reference's `includeVisual` hash and a hash over every field), `tests/effects.rs` |
| 1c | `mb-app` | `particles.rs`, `play_scene.rs`: effects, confetti and the shadow drawn, blended between ticks; no file added to the lists | `particles_and_the_shadow_are_drawn_after_a_collision_and_a_goal`, `particles_and_the_shadow_are_drawn_from_the_pack` |
| 1d | `mb-app`, `xtask` | the budget tests | `crates/mb-app/tests/budgets.rs` |

The intro fast-forward (planned as 2e) and the renderer side of phase 3 were done in this phase as well.

## Phase 2: the small items (done)

| Step | What | Where | Verified by |
|---|---|---|---|
| 2a | Night window and Storm fire flipbooks (`Background.ts`) | `mb-app/src/backdrop.rs` | `night_windows_flip_through_their_frames_with_the_stage_timer`, `storm_fires_flip_and_face_the_camera` |
| 2b | Sunset cloud scrolling, faster from "hurry up" | `backdrop.rs`, `Renderer::set_tex_offset` | `sunset_clouds_drift_and_hurry_with_the_clock` |
| 2c | Bonus-wave vertex animation on stage 92, matching the collision ripple; in the vertex shader | `mb-render/src/wave.rs`, `stage_scene.rs` | `the_bonus_wave_floor_ripples_under_the_ball` |
| 2d | HUD: `JUMP TO FLOOR n`, the warp bonus line, the flash on `GOAL` | `mb-app/src/hud.rs`, `SpriteBlend::Solid` | `a_green_goal_announces_the_floor_it_jumps_to`, unit tests in `hud.rs` |
| 2e | Intro fast-forward (`fastForwardIntro`) | `mb-engine/src/sim.rs` | `intro_fast_forward_matches_reference` |
| added | The action ends a fall-out from its 60th tick (`updateRingout`'s `canSkip`) | `mb-engine/src/sim.rs` | `action_in_sequences_matches_reference` |

Where the outcome differs from the plan: the `GOAL` flash is the reference's solid white silhouette, not an
additive blend, and `GO` has no flash in the reference (a tint, there since 0.1.0). The goal-sequence skip
was looked at with the fall-out skip and left for phase 5, because without result replays it cannot
trigger in the reference.

## Phase 3: material fidelity (done)

Reference: `Material.ts`, `TevLayer.ts`. World-specular layers, per-vertex view specular, texture matrices,
blend factors on opaque-alpha meshes, depth offsets, no specular on the inner ball shells. All of it as
uniforms on the existing pipelines. There are no reference images to compare against (the reference needs a
browser and the game data), so this phase is verified by unit tests of the material mapping
(`crates/mb-render/tests/materials.rs`) and by screenshots; no person has compared them with the original
yet. The UV-scroll flag turned out to be set on no layer of the SMB1 disc (`docs/GMA.md`).

## Phase 4: mirror floors (done)

Reference: `World.ts` `getMirrorMode`, `getMirrorPlaneMatrix`, `prepareToRenderMirrors`,
`prepareToRenderWavyDistort`; `Render.ts`. The plan named three stages (92, 102, 103). Reading the
reference showed that it mirrors every stage whose stagedef names mirror models: **37 of the 109 course
stages** on the USA disc (`crates/mb-app/tests/mirrors.rs` holds the list). One extra pass into a
half-resolution colour and depth target with the reflected camera, scissored and culled to the mirror
models in view; stages without a mirror pay nothing; Options -> Reflections turns the pass off.

The first builds broke two budgets (wasm memory up to 36.6 MB; stage 89 at 16.9 to 17.2 ms). They were
brought back inside by merging meshes per model, submitting uploads in portions and cutting the capture to
the mirror (`docs/GMA.md`, "Meshes, uploads and memory"), not by raising the budgets. Three differences
from the reference were chosen on purpose (stage 92 without the distortion offset, the planes of stages
103 and 74): `docs/GAMEPLAY.md`, "Mirror floors".

## Phase 5: result replays (version 0.3.0, not started)

The one piece of the agreed scope that is left. What it needs, read from the reference:

**What the player sees.** 30 ticks into a fall-out (`FALLOUT_RESULT_REPLAY_START_DELAY_FRAMES`) the game
cuts to a replay of the last 120 ticks before the fall, from a camera placed beside the ball's path; 90
ticks in it cuts once more, to a closer camera on the last 90 ticks. At the end of a goal sequence it
replays the approach to the goal (150 to 240 ticks, more for a faster entry) from one of four cameras
chosen by where the ball came from and how fast. The action button ends either replay from its 30th tick. Not on
bonus stages for the fall-out; never with more than one player.

| Piece | In the reference | Here |
|---|---|---|
| History | `pushResultReplayHistoryFrame`, `shouldCaptureResultReplayHistory`: a rollback state, the input and the camera yaw per tick, the last 720 ticks (`RESULT_REPLAY_HISTORY_FRAMES`), only during play and the armed tail of a goal sequence | `mb-engine`: `Sim` is `Clone`, but each carries its 320 KB particle pool, so 720 clones are out of the question inside the memory budget. Keep one snapshot and the inputs since, and re-simulate (the simulation is deterministic), or a ring of the fields a replay needs. Fixed capacity, no allocation per tick |
| Starting | `startResultReplay('fallout' / 'goal')`: needs at least 30 ticks of history, rewinds to the first state, keeps a resume state | a replay state beside `Sim`, in `mb-engine` |
| Stepping | `consumeResultReplayInputFrame`, `stepResultReplay`: feeds the recorded inputs, the fall-out's second cut at tick 90, the hand-back (a goal replay returns the rest of its `totalFrames` to the goal sequence, with `goalSkipTimerFrames = min(itself, the frames left)`), the skip | `mb-engine`; `play.rs` passes the action as it does today |
| Cameras | `camera.ts`: `initGoalReplay` / `updateGoalReplay` (modes 0 to 3, the goal followed on its anim group), `updateFalloutReplay`; the fall-out camera is seeded in `game.ts` `initResultReplayCamera` | `mb-engine/src/camera.rs`: two new states beside the four there |
| Goal skip | `canSkipGoal` in `update`: the action ends the goal sequence once its skip timer is at 0 and the ball has launched | `Sim` gets the goal skip timer it does not have (`docs/ENGINE.md`, "The action button after the intro") |
| Confetti | lives on through the replay and the rest of the sequence | nothing to add: the pieces already carry their life; the sequence just lasts longer |
| HUD | `hud.ts` starts no `GOAL` / `FALL OUT` banner while a replay runs or in the tick it ends | `mb-app/src/hud.rs` |
| Sound | `suppressAudioEffects` and the event flags: sounds of replayed ticks | to be read before porting: which events sound again in a replay |
| Drawing | the replayed ticks are ordinary ticks | nothing new in `mb-render`; particles are simulated again from the rewound state |

Things to settle before the port, each found while reading:

* **The replay cameras use `Math.random()`** (nine calls in `camera.ts`, three more in
  `initResultReplayCamera`), not one of the reference's seeded generators. A bit-exact comparison needs the
  harness (`crates/mb-engine/tests/ref/`) to replace `Math.random` with a scripted source, and the port
  needs a generator of its own for them, as the sunset clouds have; it must not touch the simulation's or
  the visual stream.
* **A replay re-runs the simulation**, so the hash, the events and the particles of replayed ticks must not
  leak into scoring, saves or sounds: the resume state is what the stage continues from.
* **Budgets.** 111 164 bytes of the wasm budget and the 2 MB of memory are what the history and the two
  cameras have to fit in; the allocation test (`stepping_with_particles_allocates_nothing`) must cover a
  tick that records.
* **Pause, retry and quit during a replay** have no counterpart in the reference's menu; `play.rs` has to
  say what each does (`crates/mb-app/tests/pause.rs` has the tests for the sequences as they are).

Verified by: a parity test for the replay flow and both cameras (`tests/parity.rs`, with the reference's
result replays switched on, which `action_in_sequences_matches_reference` already does up to the cut);
`tests/gameplay.rs` and `tests/pack.rs` for the flow in the real states; a check in `scripts/web-e2e.ts`.
Then `checkpoint/p5`.

## Order and parallelism

```
P0 -> P1 (1a || 1b -> 1c) -> P2 -> P3 -> P4 -> [0.2.0 release candidate, owner's test] -> P5
```

## Not verified by this plan

Real phones, other browsers than Chrome, the Europe and Japan discs, and how the result looks and sounds
to a person: screenshots and the audio renders need a human at the end of each phase. The full list is in
[PLAN.md](PLAN.md), "Unverified".
