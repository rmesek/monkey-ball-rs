# monkey-ball-rs

A fully offline Super Monkey Ball engine written in Rust, for the desktop and for the browser (WebAssembly).
It is a parser plus game engine that mirrors the working of [WebMonkeyBall](../WebMonkeyBall) and the web build
it powers. SMB1 comes first; SMB2 and the other sources follow.

## Offline only
No network or multiplayer features. UI is designed for touch, mobile and browser as well as desktop.

## Status
Playable on desktop and in the browser. See [docs/PLAN.md](docs/PLAN.md).

## Game content is not included
This repository contains **code only**. It never contains, downloads, or links to original game data.
You supply your own disc image, `Super Monkey Ball (USA).rvz`. The desktop program reads it in place; the
browser page copies the files the game uses into its own storage on your device.

## Play
Two targets: a desktop program and a static web site. Both ask for your own game file: a disc image of
Super Monkey Ball (Nintendo GameCube, 2001), as RVZ (uncompressed or Zstandard) or ISO/GCM, typically
`Super Monkey Ball (USA).rvz`, about 475 MB. Disc ids `GMBE8P` (USA, the tested one), `GMBP8P` (Europe) and
`GMBJ8P` (Japan) are accepted. The tested RVZ file has SHA-1 `AFAC5395934DF849552396C43EDDA0BA064B9E3C` and
SHA-256 `CE2D2DFA75EE4CED285538233B62CD73FB9561DD8F7C109AA7789692DE6271DC`; another dump or compression of
the same disc has another hash and works the same (the game checks the disc id and what it reads, not the
whole file).

**Desktop** (no argument: reopens the image used last time, or shows a file dialog):
```bash
cargo run -p mb-app --release -- "path/to/Super Monkey Ball (USA).rvz"
```
The image is read in place, nothing is copied. Saves and the remembered path live in the per-user data
directory (Options shows it, and can forget the image or erase the saves).

**Browser** (build the site into `web/dist/`, serve it with any static server, open it and choose the file):
```bash
scripts/build-web.sh            # or scripts\build-web.ps1; add --single / -Single for one self-contained .html
cd web/dist && python -m http.server 8080      # then open http://localhost:8080/
```
The page copies the 144 MB of files the game uses into the browser's private storage once and starts without
asking again afterwards; Options shows what is kept and removes it. `python -m http.server 8080 --bind 0.0.0.0`
makes it reachable from a phone on the same network (plain http there is not a secure context: the page then
plays from the picked file each time, on WebGL2). `web/dist/monkey-ball.html` (with `--single`) is the whole
page in one file that can be opened from disk. Details: [docs/WEB.md](docs/WEB.md).

Controls: WASD or arrows / left stick / touch stick / device tilt to steer, Enter to confirm, Esc to go back,
P to pause, F11 fullscreen. Menus work with touch alone. See [docs/GAMEPLAY.md](docs/GAMEPLAY.md).

## Build and check
```bash
scripts/check.sh                      # fmt, clippy, cargo test --workspace
deno run -A scripts/web-e2e.ts        # the built site in the installed Chrome, through the real file picker
```
`cargo test --workspace` covers the logic (disc and RVZ readers against hostile input, the game pack, the
import and save protocols under a crash at every step, the file lists of every course stage, touch gestures,
the game flow). Tests that need the disc image use `MB_DISC` or `../Super Monkey Ball (USA).rvz` and skip
without it. The browser check needs the bundle, Chrome, Python 3 and the image, and says what is missing.

## Credits (from the reference project)
Original game: Amusement Vision. SMB1 decompilation: camthesaxman. Web port: TwilightPB. Renderer: ComplexPlane.
Community tools and resources: SMB Custom Level Community.
