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
  the one bit there is, `BUTTON_PRIMARY_ACTION` (1), fast-forwards the intro and ends a fall-out sequence
  (both below). `step(input)` is `step_with_buttons(input, 0)`.
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

## The action button after the intro
`game.ts` asks `isPrimaryActionDown` in four places. All four read the level (is it down during this tick);
none looks for a press, and nothing has to be released first.

| Use in `game.ts` | With result replays off (what `Sim` is) | Here |
|---|---|---|
| `fastForwardIntro` (`update`) | as above | ported |
| `canSkip` in `updateRingout` | ends the fall-out sequence | ported |
| `canSkipGoal` (`update`, goal timer) | can never be true: see below | not ported, nothing to port |
| `canSkip` in `stepResultReplay` | exists only inside a result replay | not ported (result replays were dropped) |

**Fall-out.** `beginFalloutSequence` sets `ringoutTimerFrames` to 270 and `ringoutSkipTimerFrames` to 60
(`Sim::ringout_timer`, `Sim::ringout_skip_timer`); on a bonus stage 120 and 0. `updateRingout` runs on every
later tick: both timers go down by one (the second stops at 0), then, unless the stage is a bonus stage, a
skip timer at 0 and the action down set the ring-out timer to 0, and a ring-out timer at 0 ends the attempt.
So with T the tick on which the ball fell out:

- the first tick on which the action counts is T + 60; a tick T + k with k >= 60 and the action down is the
  sequence's last, `Phase::Finished(Outcome::FellOut)`, exactly as tick T + 270 is without it;
- the action on ticks T + 1 .. T + 59 does nothing and does not have to be released: one that is down from
  before the fall, from play or from the intro it fast-forwarded, ends the sequence on T + 60;
- the delay is not started again by letting go;
- on a bonus stage the action does nothing and the sequence takes its 120 ticks;
- nothing else changes: until its last tick a shortened sequence is tick for tick the unshortened one, and
  no `SimEvent` marks the end.

With result replays on, the reference cuts to the replay at T + 30 (before the action can count) and the
action then skips the replay, from its 30th frame, which resolves the fall-out at once; without replays the
60-tick delay is all there is.

**Goal.** `canSkipGoal` needs `goalSkipTimerFrames <= 0`, the launch flag (`FLAG_09`) and the action. The
skip timer starts at 210 (`beginGoalSequence`, `beginBonusClearSequence`) and goes down by one a tick with the
goal timer. The ball launches 90 ticks into the sequence and the goal timer is then cut to 60, so a goal
sequence ends 151 ticks after the ball entered the goal with the skip timer at 59; a bonus clear lasts 180
ticks with the skip timer at 30. The rule can therefore only apply after a goal replay, which hands the
sequence back with `goalSkipTimerFrames = min(itself, the frames left)`. Without replays the action does
nothing during a goal sequence or a bonus clear, before or after the launch; `Sim` has no goal skip timer.

**Time over, play.** No use of the action; `updateTimeover` counts 120 ticks.

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
SMB2 (wormholes, switches, seesaws) and multiplayer: out of scope. Audio and course progression: other
crates. Result replays: dropped by decision. The reference
starts one 30 ticks into a fall-out and at goal end; `Sim` just continues/finishes, as the reference does
with replays switched off, so there is no skipping of a replay and no leaving of the goal sequence after
one, and in a `Sim` the confetti is never older than the goal sequence, about 150 ticks of its 210 to 269.
The goal
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
same simulation with particles on and off), and disc-backed tests that skip cleanly without a disc image
(`MB_DISC`).

`tests/golden.rs` is the guard of the simulation on the disc's stages: it runs each of the 200 stagedefs through a
wandering first attempt (random directions held for random times, the action held now and then, so the
fast-forwarded fly-in and the shortened fall-out are in it) and a retry with the stick held forward, and four
autopilot goal runs, folding `Sim::hash`, `Sim::hash_visual` and the event count of every tick into one hash
per stage, compared with a table. The scripted inputs are
integer arithmetic; the values are the same in debug and release builds. A failure names the stages that
changed and prints the new table.

The table was first recorded from a port that a harness had proved equal to the reference, tick for tick
and bit for bit, on every stage of the disc (the harness is in the history up to the tag `v0.2.0`). Not
covered by that proof and held by `tests/synthetic.rs` alone: the bonus-stage branch of the fall-out rule
and the bonus clear.
