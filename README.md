# monkey-ball-rs

A port of Super Monkey Ball (Nintendo GameCube, 2001) to Rust, following the TypeScript project
WebMonkeyBall as closely as possible. It builds two things: a desktop program, and a static web site that runs
the game in a browser as WebAssembly. It is fully offline: there is no network or multiplayer code. This
repository contains code only. It never contains, downloads or links to game data; you play with your own
copy of the game.

## What you need to play

One file: a disc image of **Super Monkey Ball** for the Nintendo GameCube (2001).

| | |
|---|---|
| Formats | RVZ (uncompressed or Zstandard), or a plain ISO / GCM image |
| Typical file | `Super Monkey Ball (USA).rvz`, about 475 MB |

| Region | Disc id | |
|---|---|---|
| USA | `GMBE8P` | the tested one |
| Europe | `GMBP8P` | accepted, untested |
| Japan | `GMBJ8P` | accepted, untested |

The RVZ dump this was tested with has SHA-1 `AFAC5395934DF849552396C43EDDA0BA064B9E3C` and SHA-256
`CE2D2DFA75EE4CED285538233B62CD73FB9561DD8F7C109AA7789692DE6271DC`. Another dump or another compression of
the same disc has different file hashes and is accepted just the same: the game identifies the disc by its id
and validates what it reads, it does not hash the file. Any other file is refused with a message that says
what it is and what is supported.

## Prerequisites

| For | You need |
|---|---|
| Everything | Rust, stable, through [rustup](https://rustup.rs). `rust-toolchain.toml` makes rustup install the toolchain, `rustfmt`, `clippy` and the `wasm32-unknown-unknown` target on the first `cargo` command |
| The web site | the `wasm-bindgen` command at exactly the version of the `wasm-bindgen` crate in `Cargo.lock`: `cargo install wasm-bindgen-cli --version 0.2.129 --locked` (the build checks the version and prints this command when it is missing or different) |
| Serving the site locally | Python 3 (any static file server does) |
| The browser test | [Deno](https://deno.com) and Chrome |

On Linux the desktop program also needs the development packages of ALSA and udev (for sound and gamepads).

## Build and run: desktop

```
cargo run -p mb-app --release -- "path/to/Super Monkey Ball (USA).rvz"
```

With no argument the program reopens the image used last time, or shows a file dialog. `--help` lists the
options (`--pick`, `--fullscreen`, `--course <name>`, `--mute`). The executable is
`target/release/mb-app` (`mb-app.exe` on Windows).

## Build and run: web site

```
cargo xtask build-web --single
```

The site is written to `web/dist/`: plain files with relative paths (`index.html`, a few scripts, the 3.9 MB
WebAssembly module). `--single` adds `web/dist/monkey-ball.html` (2.1 MB), the
whole page in one file that can be copied anywhere and opened from disk.

Serve the folder with any static file server and open it:

```
cd web/dist
python -m http.server 8080
```

then go to `http://localhost:8080/`. To try it from a phone on the same network, use
`python -m http.server 8080 --bind 0.0.0.0` and open `http://<this computer's address>:8080/` on the phone.
Plain http on a network address is not a "secure context", so there the browser gives the page no private
storage: it plays from the picked file and asks for it again next time (https, or `localhost`, does not have
this limit).

Browsers: Chrome or Android System WebView 97, Safari 16, Firefox 79, or newer (only Chrome on Windows has
been run). Details, page parameters and measurements: [docs/WEB.md](docs/WEB.md). Showing the site inside an
Android app: [docs/ANDROID_WEBVIEW.md](docs/ANDROID_WEBVIEW.md).

## What happens when you start it

1. The program or page asks for the game file and says what it expects. You pick your own file once.
2. It checks the file and takes what it needs:

   | | Desktop | Browser |
   |---|---|---|
   | Game data | nothing is copied; the image is read where it is and its path is remembered | the 144 MB of files the game uses (of the 475 MB image) are copied once into the browser's private storage for this site (a second or two on a desktop PC) |
   | Saves | a few small files | a few small files next to the game data |
   | Where | `%APPDATA%\monkey-ball-rs` (Windows), `~/Library/Application Support/monkey-ball-rs` (macOS), `$XDG_DATA_HOME/monkey-ball-rs` or `~/.local/share/monkey-ball-rs` (Linux); `MB_SAVE_DIR` overrides | the browser's storage for the site (Origin Private File System); where that is unavailable nothing is copied and the page reads from the picked file |
   | Removing it | Options -> "Forget game image" and "Erase save data", or delete the folder | Options -> "Remove game data" and "Erase save data", or clear the site's data in the browser |

3. The game starts at the title menu. From then on it starts without asking. The options screen shows what is
   kept and where.

Progress is saved when a stage ends, in a way that survives closing the window or the tab, or a crash, at any
moment; an interrupted import leaves nothing behind.

## Controls

| | Keyboard | Gamepad | Touch |
|---|---|---|---|
| Steer | WASD or arrow keys | left stick | on-screen stick (appears where you touch, bottom left) |
| Menus | arrows or WASD, Enter | stick or d-pad (desktop), A | tap a row, drag to scroll |
| Back | Esc | B | Back button, or the browser's back gesture |
| Pause | Esc or P | Start | on-screen pause button |
| Fullscreen | F11 or Alt+Enter (desktop) | | Options -> Fullscreen (browser) |

The mouse works in menus (click rows, wheel scrolls). On a phone, Options -> Controls switches to steering by
tilting the device; the on-screen stick then gives way to a recentre button (the crosshair above pause)
that makes the way you hold the phone now the level position.

## Tests

```
cargo xtask check
```

runs the quality gates: `cargo fmt --all -- --check`, `cargo clippy` for the workspace, for the desktop
program as shipped and for the browser build (`wasm32-unknown-unknown`), all with `-D warnings`, and
`cargo test --workspace --release`.

Many tests read the real game. They look for your disc image in the `MB_DISC` environment variable, or for a
file named `Super Monkey Ball*.rvz` (or `.iso`, `.gcm`) in the directory that contains this repository, and
skip with a note when there is none (or when the machine has no GPU adapter, for the tests that draw).

The browser test drives the built site in Chrome through the real file picker:

```
deno run -A scripts/web-e2e.ts
```

It needs `web/dist` (build it first with `cargo xtask build-web --single`), Chrome, Python 3 and the
disc image, and says what is missing otherwise.

## Project layout

| Path | What |
|---|---|
| `crates/mb-assets` | disc image and RVZ readers, LZSS, the on-device game pack format |
| `crates/mb-stage` | stagedef parser (collision, goals, bananas, animations) |
| `crates/mb-tpl` | texture container parser and GX texture decoders |
| `crates/mb-gma` | GMA and NaomiLib model parsers, GX display lists |
| `crates/mb-audio` | music (DTK ADPCM) and sound effect (MusyX) decoders, software mixer |
| `crates/mb-engine` | deterministic simulation: physics, collision, stage objects, camera |
| `crates/mb-game` | courses, lives, score, unlocks, settings, crash-safe saves |
| `crates/mb-render` | renderer on wgpu (GX material emulation); no windowing |
| `crates/mb-app` | the game above all that: menus, loading, play, HUD, input, audio; the desktop program and the browser driver |
| `crates/mb-web` | the browser target: import worker side, game data and saves in the browser, Web Audio |
| `xtask` | the build tool behind `cargo xtask`: `build-web` and `check` (standard library only, the same command on every platform) |
| `web/` | the page: `index.html`, `main.js`, `worker.js`, `guard.js`, service worker, manifest, icons |
| `scripts/` | `web-e2e.ts`, the browser test |
| `docs/` | see below |

## Documentation

| | |
|---|---|
| [docs/PLAN.md](docs/PLAN.md) | current state, recorded decisions, possible next features |
| [docs/WEB.md](docs/WEB.md) | the web site: build, hosting, storage model, browser requirements, measurements |
| [docs/ANDROID_WEBVIEW.md](docs/ANDROID_WEBVIEW.md) | putting the site inside an Android app |
| [docs/GAMEPLAY.md](docs/GAMEPLAY.md) | states, HUD, menus, controls, what is not ported |
| [docs/ENGINE.md](docs/ENGINE.md) | the simulation and its parity tests against the reference |
| [docs/AUDIO.md](docs/AUDIO.md) | sound: formats, backends, what plays when |
| [docs/DISC_CONTENT.md](docs/DISC_CONTENT.md), [STAGEDEF.md](docs/STAGEDEF.md), [GMA.md](docs/GMA.md), [TPL.md](docs/TPL.md) | what is on the disc and the file formats |
| [docs/REFERENCE_NOTES.md](docs/REFERENCE_NOTES.md) | notes on the reference project |
| [AGENTS.md](AGENTS.md) | rules and workflow for contributors and AI agents |

## Known gaps against the original

Not there yet: the monkey inside the ball and its voices, the ball's shadow, goal confetti and other
effects, result replays, a few sounds (softest impacts, the time-over explosion, jingles, menu music), mirror
floors and per-background effects, the minigames, and Super Monkey Ball 2. The Europe and Japan discs are
accepted but untested, and nothing has been run on a real phone. The full lists are in
[docs/GAMEPLAY.md](docs/GAMEPLAY.md) and [docs/AUDIO.md](docs/AUDIO.md).

## Credits

Original game: Amusement Vision. SMB1 decompilation: camthesaxman. Web port (WebMonkeyBall): TwilightPB.
Renderer: ComplexPlane. Community tools and resources: SMB Custom Level Community.

## License

MIT OR Apache-2.0 for the code in this repository, as declared in `Cargo.toml`. The game itself is not part
of it and is not covered.
