# Changelog

## 0.3.0

Release candidate, not tagged: the owner tries it by hand first.

The same game, made smaller and easier to understand, port and change. Nothing new to play.

For a player:

- **One cargo command per target:** `cargo run --release` runs the desktop program, whose executable is
  now called `monkey-ball` (it was `mb-app`) and takes the stage to start on as `--stage <n>`;
  `cargo xtask build-web` builds the site into `target/site/` (it used to be `web/dist/`), to be hosted with any static file server such as
  `python -m http.server`. Nothing to install but Rust: the web build no longer needs the `wasm-bindgen`
  command.
- **The desktop imports the game once**, like the browser: the files the game uses are copied into
  `game.pack` in the data folder, and the disc image is not needed after that. Options has "Remove game
  data" in place of "Forget game image".
- **Two volumes**, Music and Sound (the announcer follows Sound), both 80 by default. 80 is the level the
  mix had before under the numbers 50 and 30; a save from an older version keeps its numbers and plays
  quieter until the dial is turned up.
- **A short landing page** in the browser: one line and the button.
- **The single-file page is gone.** The site has to be served; to play without a server there is the
  desktop program.
- **The instant replays** after a fall-out and a goal, once planned for this version, are dropped.

Under the hood:

- **One crate per platform:** `mb-app` is the game as a library without a window; `mb-desktop` and
  `mb-web` are the drivers. The library is compile-checked for Android in the gates. A disc image can be
  opened from an already-open file, and imported into a pack file (`import::import_to_file`), which is
  what an Android app will use.
- **Standalone:** the simulation of every stage, every parsed file and the disc reader are held by
  recorded hashes. The harness that compared the port with WebMonkeyBall, the `nod` dependency and the
  0.1.0 draw-call tables are gone.
- **Removed from the renderer:** fog, the wavy mirror's distortion, additive sprites, a specular switch
  no data used. No pixel changes.
- **Removed from the build tool:** its gzip compressor, SHA-256 and the single-file bundler.
- **Removed settings:** announcer volume, and two that were stored and never read. Old saves load.
- **Docs:** a short README, `docs/ARCHITECTURE.md` (which part is needed for what, what a platform
  supplies, a review of what is hard to change), `docs/PLAN.md` (state and open items). The plan of the
  port and the notes on the reference are deleted; the notes on single subjects stay.
- **Rust edition 2024.**
- **Size and tests:** the wasm module is 4 009 217 bytes (0.2.0: 4 016 134); 548 tests; the
  browser test makes 93 checks.

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
