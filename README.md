# monkey-ball-rs

Super Monkey Ball (Nintendo GameCube, 2001) in Rust, for the desktop and the web browser, and as a library
for an app on another platform. Fully offline: there is no network or multiplayer code. This repository
contains code only; you play with your own copy of the game.

This is a fan project. It is not affiliated with or endorsed by SEGA or Nintendo. "Super Monkey Ball" is a
trademark of SEGA, and "Nintendo GameCube" one of Nintendo.

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

The first start asks for the disc image in a file dialog and shows the import in the window. Options go
after `--`:

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

## Using the game from another project

The game is a library, and an app on another platform (the Android app, `monkey-ball-android`) uses this
repository as a git submodule at a tagged version and depends on its crates by path:

```toml
[dependencies]
# The game: states, assets, the seams (input, host, content, storage).
mb-app = { path = "monkey-ball-rs/crates/mb-app" }
# The event loop, the GPU surface and the frame; your platform implements `mb_driver::Platform`.
mb-driver = { path = "monkey-ball-rs/crates/mb-driver" }
# The disc reader, for an image the platform opens itself (`FileStream::from(file)`).
mb-assets = { path = "monkey-ball-rs/crates/mb-assets" }
# The same winit as the driver (`mb_driver::winit` names it too), with the backend your platform needs,
# for example `android-native-activity`.
winit = { version = "0.30", features = ["android-native-activity"] }
```

What to know:

- **Nothing else to set:** no crate has default features, and `dev` (test support) stays off.
- **A workspace around the submodule:** if your `Cargo.toml` is a workspace whose folder contains the
  submodule, add it to `exclude = ["monkey-ball-rs"]`. Otherwise cargo takes these crates for members of
  your workspace, and they find no `workspace.package` there for their version and edition.
- **Your profiles apply, not this repository's:** build the game with `--release`, or give your `dev`
  profile `opt-level = 1` and `[profile.dev.package."*"] opt-level = 3` as here; an unoptimised build is too
  slow to play.
- **Toolchain:** edition 2024; `rust-version` says 1.88, but only the current stable toolchain has built it.
- **What to write, and where it is done already:** [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), "The six
  things a platform supplies" and "Adding Android"; `crates/mb-desktop` is a complete example.

## What it is built on

| | |
|---|---|
| Everywhere | [wgpu](https://wgpu.rs) (graphics: Vulkan, Metal, DirectX 12, OpenGL ES, WebGPU, WebGL2), `winit` (window and events), `glam`, `bytemuck`, `ruzstd` (RVZ images) |
| Native targets | `cpal` (sound), `pollster` |
| Desktop | `gilrs` (gamepads), `rfd` (file dialog) |
| Browser | `wasm-bindgen`, `web-sys`, `js-sys` |

Nothing that talks to a network. The file formats, the simulation and the rules of the game use the standard
library only.

## Android

The Android app is written in its own repository, `monkey-ball-android`, on top of version 1.0.0 of this
one. It can show the game in two ways, and this repository gives it both:

- **In a WebView:** the built `target/site`, unchanged; [docs/ANDROID_WEBVIEW.md](docs/ANDROID_WEBVIEW.md)
  says how to host it.
- **Natively, through Vulkan or OpenGL ES:** the crates above as a library (see "Using the game from another
  project"). The driver (`mb-driver`) runs the event loop and keeps the GPU device when Android takes the
  window away; the game has touch controls, a back button, tilt steering from whatever sensor the app reads,
  sound through `cpal`, saves in a folder, and an import screen that copies the game data out of the disc
  image the player picks. What the app writes is listed in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
  "Adding Android".

Nothing in this repository has run on a phone yet.

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

The code in this repository is licensed under either of [LICENSE-MIT](LICENSE-MIT) or
[LICENSE-APACHE](LICENSE-APACHE) (Apache License 2.0), at your option. Both come without any warranty, and
neither makes the authors liable for what the code does. The licence covers this repository's own work: it
began as a port of WebMonkeyBall, which carries no licence statement, and it grants nothing over the game,
its data or its trademarks, none of which are part of it.
