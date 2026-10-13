# monkey-ball-rs

Super Monkey Ball (Nintendo GameCube, 2001) in Rust, for the desktop and the web browser. Fully offline:
there is no network or multiplayer code. This repository contains code only; you play with your own copy of
the game.

It began as a port of the TypeScript project WebMonkeyBall and plays everything that project plays for
Super Monkey Ball 1: the 109 course stages, Course Play and Practice, with the game's own models, effects,
sounds and music.

## What you need

To build:

- [Rust](https://rustup.rs) (stable). Nothing else: the first `cargo` command installs the compiler, the
  browser target and the tools named in `rust-toolchain.toml`. No Node, no scripts.
- On Linux, the development packages of ALSA, udev and GTK 3 for the desktop program.

To play:

- A disc image of **Super Monkey Ball** for the GameCube: an RVZ, ISO or GCM file, for example
  `Super Monkey Ball (USA).rvz`. The USA disc (`GMBE8P`) is the tested one; Europe and Japan are accepted.
  It is needed once (see "What is kept, and where").
- Desktop: the one executable, `target/release/monkey-ball`. Web: the folder `target/site`, a browser, and
  any static file server.

## Commands

| | |
|---|---|
| `cargo run --release` | build and start the desktop program |
| `cargo xtask build-web` | build the web site into `target/site/` |
| `cargo xtask ci` | formatting, lints for every target and all tests |

`cargo xtask` runs the small build tool in `xtask/` (an alias in `.cargo/config.toml`, the usual
convention); a web build needs it because cargo alone stops at the `.wasm` file. The commands are the same
in every shell on Windows, Linux and macOS.

### Desktop

```
cargo run --release
```

The first start asks for the disc image in a file dialog. Options go after `--`:

| | |
|---|---|
| `cargo run --release -- "path/to/image.rvz"` | name the image instead of using the dialog |
| `-- --pick` | ask for an image and import it again |
| `-- --stage <number>` | start practice on that stage |
| `-- --course <name>` | start a course (beginner, advanced, expert, ...) |
| `-- --fullscreen` | start in fullscreen |
| `-- --mute` | open no audio device |
| `-- --help` | the list |

### Web browser

```
cargo xtask build-web
python -m http.server -d target/site 8080
```

and open `http://localhost:8080`. The build makes plain files and serves nothing; Python is only one way to
host them, and any static file server works, from any path. The site loads nothing from the internet.

A browser keeps the game data only on a page it trusts: `https`, or `http://localhost`. Opened from another
device as `http://<address>:8080`, or in a private window, the game still plays, from the file you pick, and
keeps nothing; tilt control needs a trusted page too. To try a phone over USB:
`adb reverse tcp:8080 tcp:8080`, then `http://localhost:8080` on the phone.

The first time, the game asks for the disc image and copies the files it uses (about 145 MB) into its own
storage: the program's data folder on the desktop, the site's private storage in the browser. After that it
starts without the image.

## Controls

| | Keyboard | Gamepad | Touch |
|---|---|---|---|
| Steer | WASD or arrow keys | left stick | on-screen stick, or tilt the device (Options -> Controls) |
| Menus | arrows or WASD, Enter | stick or d-pad, A | tap a row, drag to scroll |
| Back / pause | Esc, P | B, Start | on-screen button, the browser's back gesture |
| Action (hold) | Space or Z | A | hold the play area |
| Fullscreen | F11 or Alt+Enter (desktop) | | Options -> Fullscreen (browser) |

Holding the action speeds up the camera's fly-in over a new stage and cuts the wait after a fall-out.

## What is kept, and where

| | Desktop | Browser |
|---|---|---|
| Game data | `game.pack` in the data folder | the site's private storage |
| Saves and settings | `profile.a`, `profile.b` in the same folder (progress and settings are one save) | next to the game data |
| Folder | `%APPDATA%\monkey-ball-rs`, `~/Library/Application Support/monkey-ball-rs`, or `~/.local/share/monkey-ball-rs`; `MB_SAVE_DIR` overrides | the browser's storage for the site |
| Removing it | Options -> Remove game data, Erase save data; or delete the folder | the same options; or clear the site's data |

Progress is saved when a stage ends. Closing the window or the tab, or a crash, at any moment leaves the
previous save or the new one; an import that is interrupted leaves nothing behind.

## Tests

```
cargo xtask ci
```

runs formatting, lints for every target and all tests. Tests that need the game look for your disc image in
the `MB_DISC` environment variable, or for `Super Monkey Ball*.rvz` (`.iso`, `.gcm`) in the folder that
contains this repository, and skip without it. The browser test, `deno run -A scripts/web-e2e.ts`, drives
the built site in Chrome and needs [Deno](https://deno.com), Chrome and Python 3; nothing else in the
project uses any of the three.

## What it is built on

| | |
|---|---|
| Everywhere | [wgpu](https://wgpu.rs) (graphics: Vulkan, Metal, DirectX 12, OpenGL ES, WebGPU, WebGL2), `glam`, `bytemuck`, `ruzstd` (RVZ images) |
| Desktop | `winit` (window), `cpal` (sound), `gilrs` (gamepads), `rfd` (file dialog) |
| Browser | `winit`, `wasm-bindgen`, `web-sys`, `js-sys` |

Nothing that talks to a network. The file formats, the simulation and the rules of the game use the standard
library only. Android is the next target and not started: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
"Adding Android".

## Where to read on

| | |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | how the program is put together, which part is needed for what, how to add a platform |
| [docs/PLAN.md](docs/PLAN.md) | the state of the project: what works, what is unverified, what is open, what was decided |
| [AGENTS.md](AGENTS.md) | the rules for changing the code |
| [CHANGELOG.md](CHANGELOG.md) | what each version contains |
| `docs/` | notes on single subjects: the simulation, the game states, sound, the web target, the file formats |

## Credits and licence

Original game: Amusement Vision. SMB1 decompilation: camthesaxman. Web port (WebMonkeyBall): TwilightPB.
Renderer: ComplexPlane. Community tools and resources: SMB Custom Level Community.

The code in this repository is declared `MIT OR Apache-2.0` in `Cargo.toml`. The game itself is not part of
it and is not covered.
