# Browser build

The game runs in a browser tab as WebAssembly (`crates/mb-web`) behind a small static page (`web/`).
The page contains the engine only. The user opens their own disc image with the file picker or by dropping it
on the page; it is read inside the tab and never leaves the device. The page's Content-Security-Policy allows
connections to its own origin only.

## Build

Needs the wasm target and the wasm-bindgen CLI, at exactly the version of the `wasm-bindgen` crate in
`Cargo.lock` (pinned in `crates/mb-web/Cargo.toml`, currently 0.2.129):

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
```

One command builds `web/pkg/` (git-ignored: `mb_web.js` + `mb_web_bg.wasm`):

```
scripts\build-web.ps1        # Windows PowerShell
scripts/build-web.sh         # bash
```

Both check the CLI version against `Cargo.lock`, run
`cargo build -p mb-web --target wasm32-unknown-unknown --profile web` and then
`wasm-bindgen --target web --no-typescript --out-dir web/pkg`. `-Dev` / `--dev` builds unoptimised.
`CARGO_TARGET_DIR` is honoured.

The `web` profile (workspace `Cargo.toml`) is `release` plus fat LTO, one codegen unit and `panic = "abort"`.
Measured against plain `--release` on the same sources: 5.20 MB instead of 6.00 MB (1.71 MB gzipped), and
12 % less render CPU time per frame under 6x CPU throttling (4.5 ms to 4.0 ms on a 250-draw stage, WebGL2).
`wasm-opt` is optional: it is not required and the scripts do not run it. With binaryen installed,
`wasm-opt -O3 web/pkg/mb_web_bg.wasm -o web/pkg/mb_web_bg.wasm` can be run after the build; its effect was
not measured here (the tool is not installed on the build machine).

To deploy, copy `web/index.html`, `web/main.js` and `web/pkg/` to any static host that serves `.wasm` as
`application/wasm`. No server-side code, no other files.

## Serve locally

```
cargo run -p xtask -- serve-web [port] [--disc <disc image>] [--root <dir>]
```

A static file server made of `std::net` only: binds to 127.0.0.1 (default port 8080), serves `web/` with the
right MIME types, `Cache-Control: no-store`, GET/HEAD and byte ranges. Open `http://localhost:8080/`.
(Opening `index.html` from disk does not work: ES modules and wasm need http.)

### Developer disc path

File dialogs cannot be driven by browser automation, so the dev server can expose the developer's own image:

```
cargo run -p xtask -- serve-web 8080 --disc "..\Super Monkey Ball (USA).rvz"
```

and `http://localhost:8080/?dev-disc=1` loads it from `/dev-disc` with range requests, through the same code
as the picker and drag-and-drop (`loadDisc` in `web/main.js`; only the byte source differs). The page uses
this only when its hostname is `localhost`/`127.0.0.1`, and only this dev server has the route. Nothing from
the disc is written anywhere.

Page parameters: `?gl=1` forces WebGL2 even where WebGPU is available; `?scale=0.5` renders at that fraction
of the device resolution (0.25 to 1); `?autoscale=0` turns the automatic resolution fallback off;
`?fps=1` shows the frame timing overlay. `window.mbStats` holds disc load timings.

### Frame timing (`?fps=1`, `window.mbPerf`)

The engine records every frame of play (not menus or the pause screen) into a ring of 1024 frames
(`mb_app::perf`): time since the previous frame, CPU time of the game update and of rendering (acquire,
encode, submit, present), simulation ticks run, draw calls, state changes and bytes uploaded. Recording is off
unless `?fps=1` is given or `mbPerf.enable()` is called. The overlay shows, twice a second: average / p99 /
worst frame interval and how many frames were longer than 1.5 times the median, update and render time,
the ticks-per-frame distribution, draws, binds, upload size, event-loop wake-ups per frame and a histogram of
frame intervals. From the console: `mbPerf.summary()`, `mbPerf.frames()` (raw rows), `mbPerf.reset()`,
`mbPerf.log()`, and `mbPerf.lastLoad()` for where the time of the last stage load went (disc read, LZSS,
parsing, texture decode, texture upload, meshes and pipelines).

## Browser requirements

- WebAssembly and ES modules.
- WebGPU, or WebGL2 as the automatic fallback. `mb-app/src/shell.rs` builds the wgpu instance with
  `wgpu::util::new_instance_with_webgpu_detection` over `BROWSER_WEBGPU | GL`, which asks the browser for a
  WebGPU adapter and drops to WebGL2 when there is none (having `navigator.gpu` is not enough to know). On a
  GL adapter the device is requested with `Limits::downlevel_webgl2_defaults()` raised to the adapter's
  texture size; the renderer stays within those limits.
- Enough memory for the image (see below).

The canvas fills the window and follows resizes and orientation changes. The render resolution is capped at
2 device pixels per CSS pixel (`MAX_PIXEL_RATIO` in `crates/mb-web/src/lib.rs`); pointer positions, the HUD
layout scale and the safe-area insets (`--sai-*` CSS variables from `env(safe-area-inset-*)`) follow the
capped size. The cap is applied to the ratio the canvas really has (winit's physical size over the canvas'
CSS size), not to `devicePixelRatio`: the two disagree under device emulation, where capping by the reported
ratio shrank the picture to a third.

Render scale: `?scale=` multiplies the capped resolution. Unless `?autoscale=0` is given, the game lowers it by
itself when at least 60 % of the frames of a two-second stretch of play take longer than 21 ms: steps of 0.8
down to 0.5, never back up (no oscillation). The HUD layout follows the new size.

## Frame pacing and power

- The event loop sleeps between frames (`ControlFlow::Wait`); each frame requests the next one, which is
  `requestAnimationFrame` in the browser. It used to poll, which ran the browser's event loop about 800 times
  per frame and made Chrome drop 3 to 4 % of the frames during play (see "What was verified").
- Nothing runs while the tab is hidden. Hiding the tab or taking focus from the canvas pauses a stage that is
  being played, and the time away is not simulated when the player returns.
- A frame identical to the one on screen (a still menu, the pause screen) is not drawn or presented again.
- The browser is asked for the low-power GPU.
- `webglcontextlost` is handled (with `preventDefault`) by a message asking to reload, as is a lost WebGPU
  device; the engine does not rebuild its GPU state in place. Progress is saved at every stage.
- The gamepad is polled only while one is connected (`gamepadconnected` / `gamepaddisconnected`).

Input: keyboard through winit (the canvas takes focus on start and on click; held keys are released when the
tab loses focus), touch (on-screen stick and buttons), mouse (left button works the touch controls, right
drag looks around), and the Gamepad API (first connected pad, polled every frame while one is connected; it only writes axes and
buttons when they change, so an idle pad does not override keyboard or touch, and a pad that disconnects is
released).

## Disc formats

| Container | Browser | Desktop |
|---|---|---|
| RVZ, Zstandard (Dolphin's default) | yes | yes |
| RVZ, no compression | yes | yes |
| ISO / GCM, CISO, WBFS, TGC | yes (through nod; only RVZ was exercised in a browser) | yes |
| RVZ with bzip2, LZMA or LZMA2 | no, clear message | yes |
| WIA, GCZ | no, clear message | yes |

Unsupported containers are recognised from the first 80 bytes, before the file is read, and the page says what
the file is and how to convert it in Dolphin (Convert File, RVZ, Zstandard).

Why the browser has its own RVZ reader (`crates/mb-assets/src/rvz.rs`): nod's WIA/RVZ reader calls
`std::time::Instant::now()`, which panics on `wasm32-unknown-unknown`, and nod decodes Zstandard, bzip2 and
LZMA with C libraries that need a C toolchain for wasm (none is required to build this project). The built-in
reader handles GameCube RVZ with "none" or Zstandard (pure-Rust `ruzstd`), including RVZ-packed junk data, and
hands nod a plain ISO stream, which nod reads without either problem. It does not check the SHA-1 hashes in
the RVZ headers. Desktop builds use nod for everything. `cargo test -p mb-assets` compares the two readers on
a sample of the disc when an image is available, and

```
cargo run --release -p xtask -- check-rvz <image.rvz>
```

compares the whole disc (both the "none" and a Zstandard conversion of the USA image are byte-identical over
all 1,459,978,240 bytes).

## Memory behaviour

The game loads files synchronously and nod's `DiscStream` is synchronous, but a browser's main thread can
only read a `File` asynchronously. So the whole image is read once, up front, and kept while the game runs.

It is kept on the JavaScript heap as 8 MiB `Uint8Array` pieces (`DiscImage` in `crates/mb-web`), not in wasm
memory. Reads copy just the requested range into wasm (`ChunkStream`). Compared with copying the file into
one wasm `Vec` (the earlier design):

- wasm32 linear memory is limited to 4 GiB, never shrinks, and has to grow as one contiguous block; a single
  0.5 to 1.4 GB allocation is what fails first on phones. Many 8 MiB buffers do not need contiguous space and
  leave wasm memory for the game itself.
- The cost is the same total: the tab holds the file size in RAM (about 0.5 GB for the uncompressed RVZ,
  0.3 GB for a Zstandard RVZ, 1.46 GB for an ISO). Measured: 509 MB JS heap with the 475 MB RVZ loaded.
  Zstandard RVZ is the best choice for memory. A decoded-group cache of 4 MiB sits on top.
- If the browser cannot provide the memory, the load stops with a message and the page stays usable.

Not done: keeping only the `File` handle and reading ranges on demand. That needs synchronous file reads,
which exist only in workers (`FileReaderSync`), so the whole game loop would have to move to a worker with an
`OffscreenCanvas`, or all game loading would have to become asynchronous. Either is a larger change than
Stage 1 needs; it is the way to go if phones with little memory become a target.

## What was verified

In the Claude desktop app's built-in browser (Chromium, Windows 11, NVIDIA RTX 4080), served by
`xtask serve-web`:

- WebGPU: menu, course select, Beginner floor 1 played to the goal with the keyboard, floor 2 loads.
- WebGL2 (`?gl=1`, ANGLE/D3D11): same content renders the same.
- Frame pacing, measured in Chrome 154 (same machine, 165 Hz display, 1084x805 canvas) with the engine's own
  recorder over 30 s of steering per run, stage loads excluded. "Long" = frames over 1.5 times the median
  interval; update / render = CPU ms per frame; 6x = Chrome's CPU throttling.

  | Stage, backend | Build | Long frames | p99 / worst interval | Update | Render | Draws |
  |---|---|---|---|---|---|---|
  | Beginner 1, WebGL2 | before | 166 of 4857 | 14.2 / 17.7 ms | 0.03 | 0.90 | 314 |
  | | after | 0 of 5707 | 6.5 / 7.0 ms | 0.03 | 0.53 | 130-220 |
  | Beginner 1, WebGPU | before | 178 of 4865 | 14.2 / 18.1 ms | 0.03 | 0.19 | 314 |
  | | after | 0 of 5783 | 6.6 / 7.2 ms | 0.03 | 0.13 | 170-230 |
  | Beginner 10, WebGL2 | before | 158 of 4899 | 13.8 / 16.6 ms | 0.03 | 1.26 | 437 |
  | | after | 1 of 5706 | 6.6 / 11.2 ms | 0.03 | 0.66 | 130-270 |
  | Beginner 10, WebGL2, 6x | before | 196 of 2379 (80 fps) | 18.6 / 61.2 ms | 0.31 | 9.86 | 428 |
  | | after | 58 of 4936 (172 fps) | 8.7 / 13.5 ms | 0.15 | 4.24 | 250 |
  | Expert 50, WebGL2 | before | 144 of 4864 | 13.6 / 16.7 ms | 0.03 | 1.50 | 572 |
  | | after | 2 of 4963 | 6.5 / 17.1 ms | 0.02 | 0.39 | 217 |

  The few long frames left are the first frames after a stage (re)load in that run. The old build with only
  the event-loop change (`Poll` to `Wait`) already measured 0 long frames on both backends, which is what
  identifies polling as the cause of the dropped frames; the rest is CPU headroom for slower devices.
- Uncompressed RVZ (497,668,128 bytes) and a Zstandard conversion of it (307,854,016 bytes) both load and
  play; an LZMA2 conversion is refused immediately with the message above; a non-disc file reports a format
  error and the picker stays usable.
- Drop path: a `File` dropped on the page (synthetic `drop` event) loads and starts; reading 475 MB from the
  `File` took 0.12 s (in-memory blob), opening the disc 0.1 to 0.8 s (1.7 s for Zstandard), first frame about
  1.1 s after that. Entering a stage freezes for roughly one second while it loads.
- Touch stick and left-mouse drag move the ball (synthetic pointer events); resizing between 375x812 and
  1024x768 reconfigures the canvas.
- No wgpu validation errors or panics in the console.

Not verified: a real phone (memory limits, the pixel-ratio cap, safe-area insets), a real gamepad, Firefox
and Safari, the native file dialog itself, ISO/CISO/WBFS images in a browser.
