# Changelog

## Unreleased

Nothing new for a player; no version number yet. A cleaning pass that makes the project a base for more
platforms ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)).

- **Scope:** the instant replays after a fall-out and a goal are dropped, by the owner's decision. The plan
  that had them as a version 0.3.0 is finished and kept as a record ([docs/PARITY_PLAN.md](docs/PARITY_PLAN.md)).
- **Standalone:** the simulation of every stage on the disc is recorded as golden hashes
  (`crates/mb-engine/tests/golden.rs`), taken where the comparison with WebMonkeyBall last passed. The
  reference is no longer needed to develop; the tests that use it still run when a checkout is there.
- **One crate per platform:** `mb-app` is the game as a library, without a window or an event loop. The
  desktop program moved to the new crate `mb-desktop` (build it with `cargo run -p mb-desktop --release`;
  the executable is still `mb-app`), the browser driver into `mb-web`.
- **Android groundwork, no app:** the game library is compile-checked for `aarch64-linux-android` by
  `cargo xtask check`; a disc image can be opened from an already-open file
  (`content::open_disc_stream`), which is what a platform with file descriptors instead of paths needs; a GL
  adapter gets the WebGL2 limits on every platform (`gpu::required_limits`).
- **Rust edition 2024**, resolver 3, `rust-version` 1.88.
- **Removed:** fog in the renderer (never switched on); seven copies of a test helper.
- **Size and tests:** the wasm module is 4 014 488 bytes (0.2.0: 4 016 134); 562 tests (0.2.0: 559); the
  browser test's 97 checks pass.

## 0.2.0

Released 2026-10-13, after the project owner tried the release candidate by hand and confirmed it.

Parity with the reference (WebMonkeyBall) in what is drawn: phases 1 to 4 of
[docs/PARITY_PLAN.md](docs/PARITY_PLAN.md). Both targets of 0.1.0, the desktop program and the static web
site, are kept.

New for a player:

- **Effects:** sparks while the ball rolls fast, stars and a flash at a hard landing, confetti at the goal
  and a sparkle around the floating ball, the ball's shadow on the stage.
- **Backgrounds that move:** the dancers in the night windows, the fires of the storm stages, the drifting
  clouds and haze of the sunset stages, the rippling floor of the bonus wave (stage 92).
- **Mirror floors** on the 37 course stages that have one, and **Options -> Reflections** to switch them
  off on a slow device. On by default on every platform; the choice is saved.
- **Materials as the reference draws them:** world and view specular highlights, additive and other
  blended surfaces, no highlight on the ball's inner shells.
- **The action button** (Space, Z, a touch on the play area; Enter and gamepad A count too): hold it to
  run the fly-in over a new stage at double speed, and to cut a fall-out short from its 60th tick.
- **HUD:** `JUMP TO FLOOR n` and the `WARP BONUS X n` line through a green or red goal, the white flash of
  the `GOAL` banner.
- Fixed: on a phone held upright the two lines of the options screen's storage note were drawn at
  different sizes.

Under the hood:

- **Simulation:** the particles are bit-identical to the reference in every field on every tick compared,
  on a random stream of their own; `Sim::hash` is unchanged and the simulation never reads them.
- **Drawing:** meshes of a model that share their state are one draw, so every stage draws fewer calls than
  in 0.1.0 (25 517 against 38 960 over the 109 stages in the fly-in); particles cost at most 11 draws,
  the shadow none; a mirror costs one more pass, cut to the mirror models in view.
- **Memory:** stage loads submit their uploads in portions, so the page needs 19.1 MB of wasm memory with a
  stage loaded where 0.1.0 needed 22 to 28 MB, mirrors included.
- **Size:** the wasm module is 4 016 134 bytes (0.1.0: 3 877 298); the game pack is unchanged at 382 files
  and 144 084 942 bytes, and a save written by 0.1.0 loads.
- **Budgets as tests:** draw calls per stage, no GPU object or heap allocation during play, texture bytes,
  heap during a load, the pack and the wasm module are recorded numbers that a test or the build holds
  ([docs/PLAN.md](docs/PLAN.md), "Budgets"). None was raised.
- **Tests:** 559 (0.1.0: 406); the browser test makes 97 checks (0.1.0: 60), among them memory and frame
  time at 6x CPU throttle on WebGL2 against 0.1.0, with Reflections on and off.

Different from the reference on purpose (for example stage 92's mirror without the distortion offset, the
mirror planes of stages 103 and 74, the floor named by `JUMP TO FLOOR n`): indexed in
[docs/PLAN.md](docs/PLAN.md), "Deliberate differences".

Not in this version: the replays after a fall-out and a goal, with the goal-sequence skip and the rest of
the confetti's life that depend on them. They are version 0.3.0.

Not verified: how it looks and sounds to a person; a real phone, an Android web view, iOS, Firefox, Safari,
a real gamepad; the Europe and Japan discs; a 60 Hz display for the frame-time limits; the WebGL2 fence
setting outside Chrome. See "Unverified" and "Open items" in [docs/PLAN.md](docs/PLAN.md).

## 0.1.0

First version: Super Monkey Ball 1 playable from the user's own disc image on two targets.

- **Targets:** a desktop program (`cargo run -p mb-app --release`) and a static web site built to `web/dist/`
  (`cargo xtask build-web --single`), which runs under any static file server and as one self-contained
  `.html` file.
- **Game:** title menu, Course Play (Beginner, Advanced, Expert, the Extra courses and Master), Practice,
  Options; deterministic 60 Hz simulation checked bit for bit against the reference; HUD, pause, lives, score,
  continues, unlocks; sound effects and stage music.
- **Input:** keyboard, gamepad, mouse, touch (on-screen stick and touch-only menus) and device tilt.
- **Game file:** RVZ (uncompressed or Zstandard) or ISO/GCM of the GameCube game; the USA disc `GMBE8P` is the
  tested one. The desktop reads it in place; the browser copies the 144 MB the game uses into its private
  storage once. Options shows what is kept and removes it.
- **Robustness:** crash-safe saves and import; parsers that reject corrupt input without panicking or
  over-allocating; a stage that fails to load returns to the menu.
- **Build and checks:** `cargo xtask build-web` and `cargo xtask check`, the same on every platform, standard
  library only; browser test `deno run -A scripts/web-e2e.ts`.

What was open and unverified when it was cut is in the version of `docs/PLAN.md` at the tag `v0.1.0`.
