# mb-engine: deterministic simulation core

Port of WebMonkeyBall `math.ts`, `collision.ts`, `physics.ts`, the runtime half of `stage.ts`, `animation.ts`,
`rng.ts`, `determinism.ts`, `sim_hash.ts`, the SMB1 gameplay camera and the per-tick flow of `game.ts`. Offline
only: the state hash exists for replays and tests, there is no network code.

## API
- `Sim::new(&mb_stage::Stage, SimConfig)`, then `sim.step(QuantizedInput{x,y})` once per 60 Hz tick (stick -127..127).
  Public state: `ball`, `world`, `camera`, `runtime` (anim groups, bumpers, jamabars, goals, bag/tape, bananas),
  `phase` (`Intro | Playing | GoalSequence | FallOut | TimeOver | Finished(Outcome)`), timers, `score`.
  `sim.hash()` = `hashSimState`. `Sim` is `Clone`.
- `Tables::from_dol(main.dol bytes)` + `install_tables()` before the first `Sim`: the S16 sin/atan tables are game
  data (not equal to `fround(sin)`), read at runtime from the user's own disc. Without them a computed
  approximation is used (not bit-exact).
- Modules `math`, `collision`, `ball`, `stage_runtime`, `camera`, `world`, `geom`, `hash` are public for renderers.

## Numerics
JS doubles are `f64`; `f32` rounding only where the reference has it (matrices, tables, `fround`, custom
`sqrt`/`rsqrt`). Same operation order, so results are bit-identical. The one libm use (banana fly easing cosine)
is a 33-entry table.

## Not modelled
SMB2 (wormholes, switches, seesaws), multiplayer, result replays (the reference starts one 30 ticks into a
fall-out and at goal end; `Sim` just continues/finishes), confetti/effects, audio, course progression. The goal
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
and corrupts every field with NaN / infinity / extreme values.

## Tests
`cargo test -p mb-engine --release`: unit tests (math, collision, rng, keyframes, quantization), synthetic-stage
sim tests, and disc-backed tests that skip cleanly without a disc image / Deno / reference checkout
(`MB_DISC`, `DENO`, `MB_REFERENCE`). `tests/parity.rs` runs the real reference `Game` headless under Deno
(`tests/ref/`, stubs for DOM/gl-matrix/fflate) with scripted stick feeds and compares per tick: ball
position/velocity (raw f64 bits), state, flags, rotations, camera yaw, timers, tilt and the sim hash.
Knobs: `MB_PARITY_STAGES`, `MB_PARITY_SCRIPTS`, `MB_PARITY_FRAMES`, `MB_PARITY_AUTOPILOT`.

Result: bit-identical on every USA-disc stage (1..200) for the scripted runs tried, incl. goal runs
(banana pickup, tape/bag, goal sequence), fall-out and time-over. The headless reference has no course, so
bonus-stage rules (stages 91-95) are ported but only checked with `bonus_stage = false`.
