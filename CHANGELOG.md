# Changelog

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

Not in this version, and what is unverified: see "Open items" and "Possible next features" in
[docs/PLAN.md](docs/PLAN.md).