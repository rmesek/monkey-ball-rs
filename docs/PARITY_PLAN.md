# Plan: parity with WebMonkeyBall (SMB1)

Written 2026-10-08 against commit `a1f1575` (version 0.1.0). It extends `docs/PLAN.md` and replaces its
"Possible next features" table.

## Goal

Everything WebMonkeyBall shows and plays for Super Monkey Ball 1 is in this port: all effects, sounds and
levels. Still out: multiplayer, the replay-file player, the map viewer, SMB2.

The targets of this work are the two that exist and work in 0.1.0: the desktop program and the static web
site (which a phone's browser can open). Neither may regress. An Android app is a later goal and is not
built here; this plan only makes sure nothing done now stands in its way (see "Keeping Android possible").

Levels, physics and sound are already at parity. The work is visual (Phases 1 to 4) and one optional
gameplay item (Phase 5).

## Decisions this plan assumes

| | Question | Default |
|---|---|---|
| D1 | Port the fall-out and goal instant replays? | yes, last (Phase 5) |
| D2 | Add what the original has and the reference lacks (monkey in the ball, its voices, jingles, menu music)? | no; out of scope |
| D3 | Push branches and tags to `origin`? | no, local only until told |

## No regressions

Version 0.1.0 is the baseline, tagged `v0.1.0` at `a1f1575`. Phase 0 records what it does (test count,
browser checks, draw calls, frame times, wasm size, memory) and every later phase is compared with that.

* All gates of `AGENTS.md` run before every merge into `main`: `cargo xtask check`,
  `cargo xtask build-web --single`, `deno run -A scripts/web-e2e.ts`, with the disc image.
* No existing test is weakened, skipped or deleted. A test that has to change because behaviour changed on
  purpose is changed in the same commit, and the commit message says why.
* The engine parity tests against the reference stay bit-identical. Effects never write into the simulation.
* Every new feature adds a test that drives the real states on both paths: the desktop path
  (`crates/mb-app/tests/gameplay.rs`) and the pack path the browser uses (`tests/pack.rs`), plus a check in
  `scripts/web-e2e.ts` where the browser can observe it.
* Every new visual can be absent without breaking a stage: a missing model or texture costs that decoration
  only, as the goal tape and clock digits do today.

## Keeping Android possible

The later Android app will most likely show the static site in a WebView (`docs/ANDROID_WEBVIEW.md`), or
be a native build of the same crates. Either works only if this phase keeps to the following; they are
already rules of `AGENTS.md`, restated because new rendering work is where they are easiest to break.

* WebGL2 stays a first-class backend: nothing beyond `downlevel_webgl2_defaults`, no WebGPU-only feature,
  no compute, no float render targets, no threads.
* The site stays plain relative files with no network request, and the single-file build keeps working.
* Platform access stays behind `mb_app::host` and the input seam; no state calls a browser or desktop API.
* The format, simulation and rules crates stay free of GPU, windowing and browser code.
* New files the game reads go through the file lists in `content.rs`, so an import stays complete.
* Touch-only use keeps working for every new screen or option.

## Limits every phase works inside

These come from the targets; WebGL2 in a phone's browser is the tightest of them.

| Limit | Rule |
|---|---|
| Graphics API | nothing beyond `wgpu::Limits::downlevel_webgl2_defaults` |
| Draw calls | the cost on WebGL2 is per call (baseline: 275 draws, 5.1 ms render at 6x CPU throttle, Beginner 1). Particles and confetti are instanced or batched, never one draw per particle: together they add at most 12 draws at their peak, and the shadow adds none (a term in the stage shader, not the reference's second geometry pass). Mirrors may add one reduced scene pass, on the three stages that have them only |
| Per-frame work | no buffer, texture, pipeline or bind-group creation during play; dynamic geometry goes into one preallocated buffer written once per frame; no heap allocation per tick or per frame in the effect code |
| Memory | fixed-capacity effect pools; wasm memory never shrinks, so nothing is decoded "just in case". Budget: wasm memory in play grows by at most 2 MB over the 28 MB baseline |
| Threads | none (no `SharedArrayBuffer` under a plain static server or in the app) |
| Size | the wasm module (3.9 MB) grows by at most 250 KB over all phases |
| Determinism | effects use the reference's separate visual RNG and stay out of `Sim::hash`; the parity tests keep passing bit for bit |
| Storage | a loader that reads a new file adds it to the lists in `content.rs` and raises `gamepack::VERSION` |
| Offline | no networking crate, no request outside the bundle |

Each phase ends by re-measuring draws, update and render time and wasm memory the way `docs/WEB.md`
describes, and records the numbers there. A phase that breaks a budget is not merged.

## Git checkpoints and workspaces

* `main` stays releasable. One branch per phase, `parity/p1-effects` and so on, merged into `main` with
  `--no-ff` when the gates are green, then an annotated tag `checkpoint/p1`. One logical change per commit.
* `CHANGELOG.md` and the matching `docs/` file are updated in the phase that changes them. Version bumps:
  0.2.0 after Phase 2, 0.3.0 after Phase 4.
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

## Phase 0: baseline (lead, on `main`)

| Step | Commit |
|---|---|
| Commit the pending edits (`AGENTS.md`, `docs/PLAN.md`, `release.yml`) as they are | "docs/CI: release workflow" |
| Run every gate with the disc; record test count, draw calls, frame times, wasm size and memory as the baseline | none (numbers go into this file) |
| `docs/PLAN.md`, `docs/GAMEPLAY.md`, `README.md`: correct the claim that the reference draws a monkey; link this plan | "docs: parity plan" |

### Baseline, measured 2026-10-08 on `1ec223f` (Windows 11, USA disc)

| | |
|---|---|
| `cargo xtask check` | all gates passed; 406 tests passed, 0 failed, 2 ignored |
| `deno run -A scripts/web-e2e.ts` | all 60 checks passed |
| `pkg/mb_web_bg.wasm` | 3 877 298 bytes; single file 2 107 204 bytes |
| Draw calls, frame times, wasm memory | not measured again; the figures of `docs/WEB.md` for 0.1.0 stand (275 draws, update 0.16 ms + render 5.1 ms at 6x CPU throttle on WebGL2, Beginner 1; 22 to 28 MB wasm memory) |

## Phase 1: particles, confetti, shadow

Reference: `effects.ts`, `stage.ts` (confetti), `physics.ts` (spawn calls), `World.ts` `drawEffects`,
`drawConfetti`, `drawProjectedShadow`.

| Step | Owner | What | Verified by |
|---|---|---|---|
| 1a | Rendering | `mb-render`: instanced drawing of one model at many transforms and colours (confetti pieces, star, flash, glow); one dynamic quad batch for streaks and sparkles (preallocated, written once per frame, additive and alpha, depth test without depth write); a projected ball-shadow term for stage objects | offscreen render tests; draw count asserted |
| 1b | Simulation | `mb-engine`: port of `effects.ts` (sparks, stars, flash, post-goal sparkle) and of the confetti; visual RNG seeded as the reference; fixed pools; not hashed | parity test extended with the reference's `includeVisual` hash; existing parity unchanged |
| 1c | Game | `play_scene.rs`: draws effects (streak, sparkle, flash, star, ground glow), confetti and the projected shadow, blended between ticks like the ball; any new texture or model file added to the file lists | `tests/gameplay.rs` (effects exist after a collision and a goal), screenshots from `--autoplay` |

1a and 1b run in parallel (different crates) once the render-state structs are agreed; 1c follows. Four or
five commits, then `checkpoint/p1`.

## Phase 2: the small items

Independent of each other; one agent in `mb-app` does a to c in sequence (they touch the same two files),
d and e can run beside it.

| Step | What | Note on cost |
|---|---|---|
| 2a | Night window and Storm fire flipbooks (`Background.ts`); the parser already reads the records | all frames are loaded once; a frame change toggles visibility, uploads nothing |
| 2b | Sunset cloud scrolling, faster from "hurry up" | a per-object texture offset uniform; the first part of Phase 3 |
| 2c | Bonus-wave vertex animation on stage 92 (`BonusWave.ts`), matching the collision ripple | done in the vertex shader from a time uniform, so no vertex buffer is rewritten |
| 2d | HUD: `JUMP TO FLOOR n`, the warp bonus line, additive flash on `GOAL` and `GO` | an additive blend mode in `sprite.rs` |
| 2e | Intro fast-forward (`fastForwardIntro` in `game.ts`) | simulation behaviour: ported in `mb-engine` with a parity script |

Then `checkpoint/p2`, version 0.2.0.

## Phase 3: material fidelity

Reference: `Material.ts`, `TevLayer.ts`. World-specular layers, the UV-scroll flag and texture matrices,
no specular on the inner ball shells. All of it as uniforms on the existing pipelines, so the pipeline
count does not grow. There are no reference images to compare against (the reference needs a browser and
the game data), so this phase is verified by unit tests of the material mapping and by side-by-side
screenshots that a person looks at. Shader cost is measured on WebGL2 before and after. `checkpoint/p3`.

## Phase 4: mirror floors

Reference: `World.ts` `prepareToRenderMirrors`, `prepareToRenderWavyDistort`. Flat mirrors on stages 102
and 103, the wavy one on stage 92. One extra pass into a half-resolution colour and depth target with the
reflected camera, skipping what the reference skips (mirror models, objects flagged out of mirror
captures) and all particles. Stages without a mirror pay nothing. An options row "Reflections" turns the
pass off on weak devices. Verified by an offscreen test on stage 102 and by the frame budget. Then
`checkpoint/p4`, version 0.3.0.

## Phase 5: result replays (if D1 stays yes)

Reference: `game.ts` (`startResultReplay`, `stepResultReplay`, the history frames), `camera.ts`
(`FALLOUT_REPLAY`, `GOAL_REPLAY`). A bounded history in `mb-engine`, the two replay cameras, and the flow
in `play.rs`. Parity test with the reference for the replay camera. `checkpoint/p5`.

## Order and parallelism

```
P0 -> P1 (1a || 1b -> 1c) -> P2 -> P3 -> P4 -> P5
```

## Not verified by this plan

Real phones, other browsers than Chrome, the Europe and Japan discs, and how the result looks and sounds
to a person: screenshots and the audio renders need a human at the end of each phase.
