# monkey-ball-rs

A cross-platform (desktop, Android, browser/WASM), fully offline Super Monkey Ball engine written in Rust.
It is a parser plus game engine that mirrors the working of [WebMonkeyBall](../WebMonkeyBall) and the web build
it powers. SMB1 comes first; SMB2 and the other sources follow.

## Offline only
No network or multiplayer features. UI is designed for touch, mobile and browser as well as desktop.

## Status
Stage 1 (playable on desktop and in the browser) is done; Stage 2 is the Android APK. See [docs/PLAN.md](docs/PLAN.md).

## Game content is not included
This repository contains **code only**. It never contains, downloads, or links to original game data.
You supply your own disc image, `Super Monkey Ball (USA).rvz`; files are read from it on demand.

## Play
Desktop (no argument opens a file dialog and remembers the choice):
```bash
cargo run -p mb-app --release -- "path/to/Super Monkey Ball (USA).rvz"
```
Browser (build, serve, open http://localhost:8080 and drop the RVZ on the page):
```bash
scripts/build-web.sh
cargo run -p xtask -- serve-web 8080
```
Controls: WASD or arrows / left stick to tilt, Enter to confirm, Esc to go back, P to pause, F11 fullscreen.
See [docs/GAMEPLAY.md](docs/GAMEPLAY.md) and [docs/WEB.md](docs/WEB.md).

## Build and check
```bash
scripts/check.sh
```
## Credits (from the reference project)
Original game: Amusement Vision. SMB1 decompilation: camthesaxman. Web port: TwilightPB. Renderer: ComplexPlane.
Community tools and resources: SMB Custom Level Community.
