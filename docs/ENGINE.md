# mb-engine: deterministic simulation core

Port of WebMonkeyBall `math.ts`, `collision.ts`, `physics.ts`, the runtime half of `stage.ts`, `animation.ts`,
`rng.ts`, `determinism.ts`, `sim_hash.ts`, `effects.ts`, the SMB1 gameplay camera and the per-tick flow of
`game.ts`. Offline only: the state hash exists for replays and tests, there is no network code.

## API
- `Sim::new(&mb_stage::Stage, SimConfig)`, then `sim.step(QuantizedInput{x,y})` once per 60 Hz tick (stick -127..127).
  Public state: `ball`, `world`, `camera`, `runtime` (anim groups, bumpers, jamabars, goals, bag/tape, bananas),
  `phase` (`Intro | Playing | GoalSequence | FallOut | TimeOver | Finished(Outcome)`), timers, `score`.
  `sim.hash()` = `hashSimState`, `sim.hash_visual()` = the same with `includeVisual` (below). `Sim` is
  `Clone`.
- `sim.step_with_buttons(QuantizedInput{x,y}, buttons)` is the same tick with the reference's button bits;
  the one bit there is, `BUTTON_PRIMARY_ACTION` (1), fast-forwards the intro (below). `step(input)` is
  `step_with_buttons(input, 0)`.
- `Tables::from_dol(main.dol bytes)` + `install_tables()` before the first `Sim`: the S16 sin/atan tables are game
  data (not equal to `fround(sin)`), read at runtime from the user's own disc. Without them a computed
  approximation is used (not bit-exact).
- Modules `math`, `collision`, `ball`, `stage_runtime`, `camera`, `world`, `geom`, `hash`, `effects` are public
  for renderers.

## Numerics
JS doubles are `f64`; `f32` rounding only where the reference has it (matrices, tables, `fround`, custom
`sqrt`/`rsqrt`). Same operation order, so results are bit-identical. The one libm use (banana fly easing cosine)
is a 33-entry table.

## Intro fast-forward
`fastForwardIntro` of `game.ts`. On a tick that starts with more than 120 intro frames left, with
`SimConfig::intro_fast_forward` set and `BUTTON_PRIMARY_ACTION` in the tick's buttons, `intro_timer` and the
fly-in camera's `timer_curr` go down by two instead of one. Nothing else is advanced twice: the stage clock is
held at 0 until 120 frames are left anyway, the ball is not there yet, and bananas and bags do one tick. So a
first intro takes between 240 and 360 ticks, and the last 120 frames (READY, the ball drop at 24, GO at 0)
always take 120 ticks. A fast tick shows the camera where two plain ticks would have put it.

Who decides what:

| Condition of the reference | Here |
|---|---|
| `introTimerFrames > 120` | the engine, every tick |
| the primary action is held (`Input.isPrimaryActionDown`: Space, Enter, Z, the touch action, gamepad button 0) | the caller, as bit 0 of `buttons` |
| `stageAttempts === 1` | the caller, through `SimConfig::intro_fast_forward`: `first_attempt()` sets it, `retry()` clears it |
| `players.length <= 1` | always true here |

Behaviour ported as it is in the reference:

- The timer is not clamped at 120. From an odd count it goes 121 -> 119; that happens when the action is first
  pressed after an odd number of plain ticks. READY is therefore a crossing (`prev > 120 && now <= 120`, as
  the reference's announcer and this port's HUD and audio test it), never `== 120`.
- The ball drop and the end of the intro are tests for equality (`=== dropFrames`, `=== 0`) on that timer. A
  double step cannot pass either, because it needs more than 120 frames left and so lands on 119 at the
  lowest. The same holds for the stage clock override `max(0, 120 - intro)`: it is 0 for every frame above
  119, so play starts from the same clock whatever was skipped.
- The camera guards its timer only before the first of the two steps, so a fly-in standing at 1 on a fast
  tick ends at -1 and stays. With the game's lengths (intro 360, fly-in 350) the fly-in timer is always the
  intro timer minus 11 and is at 110 or more before a fast tick; only a `SimConfig` with a fly-in much shorter
  than its intro gets there.
- A retry's intro is 120 frames, so it would never fast-forward even with the flag set; the flag is the
  reference's attempt counter, kept for a configuration with other lengths.
- In the reference the game loop asks the input device on every tick; the `buttons` of its input feed and
  replays are recorded but not read back for this. Here the buttons are an argument of the tick, so a
  recorded run replays its fast-forward. `QuantizedInput` stays the stick alone (the reference's
  `QuantizedStick`).
- `SimEvent`s are unchanged; none is added.

Not ported: the other two uses of the primary action in `game.ts`, cutting the fall-out sequence short
(`ringoutSkipTimerFrames`) and leaving the goal sequence once the ball has launched, and skipping a result
replay. Buttons do nothing outside the intro.

## Particles (display only)
`effects.rs` ports `effects.ts` and the confetti of `stage.ts`. The state is public and read by the renderer;
the simulation never reads it.

- `sim.runtime.effects: Vec<BallEffect>`, in spawn order. `kind` is `Coli` (spark: `spawnMovementSparks` when
  the ball is on the ground faster than 0.3 per tick, `trunc(5 * speed)` a tick; and half of a collision
  burst), `ColiStar` and `ColiFlash` (`spawnCollisionStars` when a collision is harder than 0.06: one flash and
  up to 16 stars and 16 sparks), `Levitate` (`spawnPostGoalSparkle`, every other tick of the goal float).
  Every field of the reference is there: `pos` / `prev_pos`, `rot_*` / `prev_rot_*` (to blend between ticks),
  `scale`, `alpha`, `color_*`, `life`, `age`, `life_ratio()`, the ground glow (`has_ground`, `glow_pos`,
  `glow_rot_x/y`, `glow_dist`), `surface_normal` for a flash, and `id`.
- `sim.runtime.confetti: Vec<ConfettiParticle>`: 160 pieces (`CONFETTI_PER_GOAL`) the first time a goal bag
  opens, `model_index` 0..5, with `pos` / `prev_pos`, `rot_*` / `prev_rot_*`, `scale`, `life`.
- `sim.runtime.visual_rng`: the reference's `visualRng`, seeded `stage id ^ 0x9e3779b9`; particles draw from
  nothing else. A new `Sim` (first attempt or retry, as a new `StageRuntime` in the reference) starts with
  empty pools, the stream at its seed and effect ids from 1.
- Update order is the reference's: inside `StageRuntime::update_objects` after the goal bags (confetti, then
  effects, newest first), and the ball's spawns in `Ball::step` after the stage collision. Particles are not
  advanced during time over (the stage is paused) or once the `Sim` is finished.
- `SimConfig::visual_effects = false` is the reference's `suppressVisualEffects`: no effect is spawned and
  nothing is updated. As there, an opening bag still throws its confetti, which then never moves.
- `Sim::hash()` does not see particles. `Sim::hash_visual()` / `hash_sim_state_visual` appends the confetti and
  then the effects (count, then position, velocity and the three angles of each).

Pools: `runtime.effects` is allocated once with `MAX_BALL_EFFECTS` = 1024 entries (320 KB) and never
grows; `runtime.confetti` is reserved for the one bag that opens per attempt. A tick allocates nothing
(`tests/effects.rs` counts). The reference's effect array has no limit: sparks per tick grow with the ball's
speed and live at most 59 ticks, a collision adds at most 33. 1024 live effects need a ground speed above 5
units per tick held for a second; the highest count seen on the 200 retail stages with the stick held in each
of six directions for 40 s is 212 (stage 151). Spawns that find the pool full are dropped without drawing
random numbers, which is the one place the port can differ from the reference, and what stops a ball with an
infinite speed from spawning forever as it would there.

Differences from the reference, none of which a valid stage reaches: the pool limit above, and effect ids,
which the reference counts across stage loads.

## Not modelled
SMB2 (wormholes, switches, seesaws), multiplayer, result replays (the reference starts one 30 ticks into a
fall-out and at goal end; `Sim` just continues/finishes, so in a `Sim` the confetti is never older than the
goal sequence, about 150 ticks of its 210 to 269), audio, course progression. The goal
tape anchor height uses the reference's fallback constant unless `SimConfig::goal_tape_anchor_y` is set; the
fly-in bounds come from collision triangles unless `SimConfig::bound_sphere` is set (camera fly-in only).

## Broken stages
`Sim` takes whatever `mb-stage` parsed. It never panics or loops on it: grid cells that name missing
triangles are dropped when the stage is converted (`StageGeom::from_stage`; the parser cannot produce them),
empty stages, missing start positions, degenerate triangles, zero-keyframe or zero-length animations and
non-finite numbers are all simulated as they are. Two rules are not in the reference and cannot trigger on a
valid stage: a ball whose position is not a finite number has fallen out (it would otherwise never cross the
fall-out plane), and it cannot enter a goal. A stage whose fall-out height is NaN simply has no fall-out
plane; the stage clock ends the attempt. `tests/robust.rs` builds stages in code, encodes them as stagedefs
and corrupts every field with NaN / infinity / extreme values. The particles run in all of it, and two tests
poison every float they carry and drive the ball at speeds up to infinity: counts stay within the pools and
every particle still ends with its life, which is counted in ticks.

## Tests
`cargo test -p mb-engine --release`: unit tests (math, collision, rng, keyframes, quantization), synthetic-stage
sim tests (`tests/effects.rs` for the particles: spawn thresholds, lives, the pool limit, no allocation, the
same simulation with particles on and off), and disc-backed tests that skip cleanly without a disc image /
Deno / reference checkout
(`MB_DISC`, `DENO`, `MB_REFERENCE`). `tests/parity.rs` runs the real reference `Game` headless under Deno
(`tests/ref/`, stubs for DOM/gl-matrix/fflate) with scripted stick feeds and compares per tick: ball
position/velocity (raw f64 bits), state, flags, rotations, camera yaw, timers, tilt and the sim hash, and for
the particles the `includeVisual` hash, both counts, a hash over every field of every effect and confetti
piece (computed by the harness from the reference's objects) and the visual random state, and for the camera
its eye and look-at point (raw f64 bits), the other two angles and the fly-in timer.
Knobs: `MB_PARITY_STAGES`, `MB_PARITY_SCRIPTS`, `MB_PARITY_FRAMES`, `MB_PARITY_AUTOPILOT`,
`MB_PARITY_FX_STAGES`, `MB_PARITY_FX_SCRIPTS`, `MB_PARITY_FX_FAST`, `MB_PARITY_FX_GOALS`, `MB_PARITY_FX_FRAMES`,
and `MB_PARITY_FF_STAGES`, `MB_PARITY_FF_RETRY`.

`intro_fast_forward_matches_reference` gives the reference a scripted primary action: an inputs line with a
third column makes the harness answer `input.isPrimaryActionDown()` from it, tick by tick (the game code is
unchanged; only the device is scripted). The action is held during intros only, because afterwards the
reference also uses it to cut sequences short (above). Next to the comparison the test checks that the
reference's intro took exactly as many ticks as the rule says for the script, that a retry's took 120, and
that some run took the timer from 121 to 119.

`effects_match_reference` runs the reference with result replays off (`run.ts ... noreplay`), so that the
particles can be followed where the game would cut to a replay: a fall-out runs its 270 ticks and reloads the
stage, the harness awaits the reload and the comparison goes on into the retry against a fresh `Sim`; after a
goal sequence the stage runtime alone is advanced 400 more ticks on both sides until the last confetti piece
is gone. The test fails unless its runs reached every part of the particle code (each kind, rolling sparks,
ground under effects and confetti, moving ground, a star's bounce, expiry, a retry).

Result: bit-identical on every USA-disc stage (1..200) for the scripted runs tried, incl. goal runs
(banana pickup, tape/bag, goal sequence), fall-out and time-over. The headless reference has no course, so
bonus-stage rules (stages 91-95) are ported but only checked with `bonus_stage = false`.

Particles: bit-identical in every field on the default runs of both tests (stages 1, 2, 10, 50 with four
stick scripts; 36, 77, 95, 126, 133, 151, 197 with the stick held forward; goal runs on 1, 3, 11, 17 and 95;
16 retries after a fall-out; 55,000 ticks), and on one run of `effects_match_reference` over all 200 stages
(`MB_PARITY_FX_STAGES` and `MB_PARITY_FX_FAST` = 1..200, scripts 0 to 3: 1004 runs, 1.9 million ticks, 1997
retries). Not compared: confetti from a goal on a moving group, and anything during a result replay (not
ported).

Intro fast-forward: bit-identical in every column on 7 stages (1, 2, 10, 36, 50, 77, 126) with 8 action
scripts each, 900 ticks a run with a stick script after the intro: held from the first tick to the last of
the intro; from the first tick to 60 frames before its end (across READY); from the second tick (121 -> 119);
from tick 100 through the ball drop; released with 121 frames left; released and pressed again six times;
every other tick; random taps. And on 6 stages (2, 10, 36, 50, 77, 126) with result replays off, the stick
held forward and the action held for the first 110 ticks of every attempt, 2400 ticks a run: the first intro
takes 250 ticks, the 18 intros after a fall-out take 120. 62 runs, 49,323 ticks. `tests/synthetic.rs` has the
same rules without a disc (twice the speed above 120, nothing at 120 or below, nothing on a retry or with the
flag off, the drop at 24 and the end at 0 for even, odd and tapped timers, the camera overshoot).

One thing the retry comparison showed about the simulation itself: the reference reuses its ball object
across a reload and `initBallForStage` leaves `ball.speed` as the last step set it, so its hash carries a
stale speed until the ball next steps (the end of the intro). A fresh `Sim` has 0 there. Nothing but the
hash reads the value; the test carries it over so that the rest of the retry can be compared.
