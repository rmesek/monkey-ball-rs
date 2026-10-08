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
`cargo build -p mb-web --target wasm32-unknown-unknown --release` and then
`wasm-bindgen --target web --no-typescript --out-dir web/pkg`. `-Dev` / `--dev` builds unoptimised.
`CARGO_TARGET_DIR` is honoured. The release wasm is about 5.8 MB (1.8 MB gzipped); `wasm-opt` is not used.

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

Page parameters: `?fps=1` shows frame rate, canvas size and graphics adapter; `?gl=1` forces WebGL2 even where
WebGPU is available; `window.mbStats` holds load timings.

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
capped size.

Input: keyboard through winit (the canvas takes focus on start and on click; held keys are released when the
tab loses focus), touch (on-screen stick and buttons), mouse (left button works the touch controls, right
drag looks around), and the Gamepad API (first connected pad, polled every frame; it only writes axes and
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

- WebGPU: menu, course select, Beginner floor 1 played to the goal with the keyboard, floor 2 loads; steady
  120 fps (display rate) at 1024x768, longest frame 17 ms during play.
- WebGL2 (`?gl=1`, ANGLE/D3D11): same content renders the same; 120 fps, longest frame 13 ms.
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
