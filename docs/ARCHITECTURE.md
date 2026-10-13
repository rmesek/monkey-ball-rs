# Architecture

How the program is put together, which part is needed for what, and what a new platform has to supply.
The state of the project (what works, what is open) is in [PLAN.md](PLAN.md).

## The shape

```
 one crate per platform    mb-desktop                    mb-web + web/               (later) Android
                           window, gamepads,             canvas, import worker,
                           file dialog, command line     page storage, Web Audio
                                 │                              │                          │
                                 └────────── the six things a platform supplies ───────────┘
 the same on every         mb-app      the game: menu, loading, play, HUD, scenes, and the seams
 platform                  mb-render   drawing on wgpu; takes a device and a target, knows no window
                           mb-engine   the 60 Hz simulation          mb-game   courses, score, saves
                           mb-assets   disc image reader, game pack  mb-stage, mb-tpl, mb-gma, mb-audio
                                                                     the game's file formats
 tooling                   xtask (cargo web, cargo xtask check)      scripts/web-e2e.ts (browser test)
```

Dependencies point downwards only. Nothing below the platform crates names a window, an event loop or an
operating system.

## Which part is needed for what

| The game has to | Code | Lines | Notes |
|---|---|---|---|
| **1. Read the disc image and keep the game's files on the device** | | | |
| read RVZ / ISO / GCM, find files in it | `mb-assets/src/{disc,rvz,lzss}.rs` | 1 800 | only third-party piece: `ruzstd` (Zstandard) |
| copy the 382 files the game uses into one pack, atomically | `mb-assets/src/gamepack.rs`, `mb-app/src/import.rs` | 700 | header written last; every file has a checksum |
| know which files those are | `mb-app/src/content.rs` (the file lists) | 730 | the only place a disc path is spelled |
| keep the pack: a file (desktop, later Android), the browser's private file system | `mb-desktop/src/lib.rs`, `mb-web/src/lib.rs`, `web/worker.js` | | the platform's part |
| **2. Play the game** | | | |
| parse stages, textures, models, sound | `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio` | 6 500 | no dependencies; hostile input never panics |
| simulate: physics, collision, stage objects, camera, particles | `mb-engine` | 8 000 | deterministic; held by recorded hashes |
| rules: courses, lives, score, unlocks, settings | `mb-game` | 3 700 | |
| draw: GX materials, particles, shadow, mirror floors | `mb-render` | 7 000 | one renderer on `wgpu`, within WebGL2's limits |
| the states: title, course and stage select, options, loading, play, pause, HUD | `mb-app/src/{menu,loading,play,hud,text,widgets}.rs` | 6 800 | |
| build a stage into a scene and animate it | `mb-app/src/{stage_scene,play_scene,particles,backdrop,mirror,anim,stage_info}.rs` | 4 400 | |
| decide what plays when, mix it | `mb-app/src/audio.rs`, `audio_cpal.rs` (native output) | 2 000 | the browser mixes in Web Audio (`mb-web/src/audio.rs`) |
| take input: keys, stick, pointer, touch, tilt | `mb-app/src/{input,ui,tilt}.rs` | 1 200 | states read actions and axes, never a device |
| **3. Survive a sudden exit** | | | |
| saves in two checksummed slots, written when they happen | `mb-game/src/{storage,progress}.rs` | 1 100 | a kill leaves the previous save or the new one |
| an import that is all there or not there | `import::import_to_file` (temp file, rename); a marker file in the browser | | |
| **4. Not get worse unnoticed** | | | |
| recorded hashes: the simulation of every stage, every parsed file, the disc reader | `tests/golden.rs` in `mb-engine`, `mb-stage`, `mb-tpl`, `mb-gma`; `mb-assets/tests/disc.rs` | 550 | |
| the real states on an offscreen target, by scripted input | `mb-app/tests/{gameplay,pack,pause,rules,mirrors,audio}.rs` on `mb-app/src/headless.rs` | 4 000 | needs the `dev` feature, which the tests switch on |
| budgets: draw calls, GPU objects, heap, texture and pack bytes, wasm size | `mb-app/tests/{budgets,memory}.rs`, `mb-engine/tests/effects.rs`, `xtask` | 1 000 | counted, not timed |
| pixels of the renderer on every backend | `mb-render/tests/*.rs` | 3 700 | |
| damaged files never crash | `tests/robust.rs` in the parsers, `mb-engine`, `mb-game` | 2 500 | |
| the program killed mid-run | `mb-desktop/tests/kill.rs` | 110 | |
| the built site in a real browser | `scripts/web-e2e.ts` | 910 | Deno and Chrome; the only test of the page |

Third-party code: `wgpu`, `glam`, `bytemuck`, `web-time`, `ruzstd` in the shared crates; `cpal` and
`pollster` on native targets; `winit` in the two platform crates; `rfd` and `gilrs` on the desktop;
`wasm-bindgen`, `web-sys`, `js-sys` in the browser. The build tool links `wasm-bindgen-cli-support`.

## The six things a platform supplies

A platform crate is the only code that knows its platform. Both existing ones do exactly this
(`crates/mb-desktop/src/{lib,app}.rs`, `crates/mb-web/src/{lib,shell}.rs`).

| # | Supplies | Through | Desktop | Browser |
|---|---|---|---|---|
| 1 | a surface and a GPU device | `mb_app::gpu`, `mb_render::Renderer::new` | winit window, blocking setup, vsync | winit canvas, awaited setup, WebGPU or WebGL2 |
| 2 | the game's files, as a `Content` | `import::import_to_file` + `content::open_pack_file`, or `Content::deferred` where storage is asynchronous | pack file in the data folder | pack in the private file system, prepared per state |
| 3 | a place for saves, a `Storage` | `mb_game::{SlotStorage, FileStorage}` | files in the data folder | private file system or `localStorage` |
| 4 | sound output, an `AudioBackend` | `mb_app::audio` | `audio_cpal` (works on any native target) | Web Audio |
| 5 | input | `mb_app::input::InputState`, `ui::TouchControls`, `tilt::TiltSteering` | keyboard, mouse, `gilrs` pads, touch | keyboard, mouse, Gamepad API, touch, device orientation, the back gesture |
| 6 | what only the platform can do | `mb_app::host`: capabilities in, commands out (`RemoveGameData`, `SetFullscreen`, `Tilt`, `CalibrateTilt`) | remove data | remove data, fullscreen, tilt |

Then once per frame: build a `GameCtx`, call `GameStack::update` and `InputState::end_frame`, draw with the
top state's camera.

Rules that keep this true:

* Code for one platform lives in that platform's crate. No `cfg(target_os)` in shared code; the
  `cfg(target_arch = "wasm32")` there separates "may block" from "must await" and nothing else.
* What a platform cannot do is a `host::Caps` flag the menus read, never a `cfg` in a state.
* `cargo xtask check` compiles `mb-app` and everything under it for `aarch64-linux-android`, so shared code
  cannot quietly become desktop-only. It is a compile check; nothing has run on a phone.

## Adding Android

Nothing is started. What exists and what is missing:

| | In a WebView | Native (Vulkan, GLES as fallback) |
|---|---|---|
| What it is | the built `web/dist` shown by a small host app | a new crate `mb-android` beside `mb-desktop` |
| Rust changes | none | the driver only |
| Game data | the page's import, as in a browser | `import_to_file` into the app's files directory, from a file descriptor the document picker hands over (`content::open_disc_stream` reads one) |
| Saves | page storage | `FileStorage::new(<files directory>)` |
| Sound | Web Audio | `audio_cpal` (compiles for Android; never heard) |
| Touch, keys | as in the browser | the same winit code the desktop driver has |
| Tilt (gyro) | `deviceorientation`, already there | the sensor through the NDK into `TiltSteering::sample`; the seam is ready, the source is missing |
| Gamepads | Gamepad API | missing: `gilrs` has no Android backend |
| Guide | [ANDROID_WEBVIEW.md](ANDROID_WEBVIEW.md) | this table |

What will cost the most in a native port, in order:

1. **The surface life cycle.** Android destroys the surface whenever the app leaves the screen. Both
   drivers create window, device and renderer together, once. The renderer already takes its target per
   frame and survives a resize; what is missing is a driver that keeps device and renderer and makes the
   surface again.
2. **Two winit drivers, soon three.** `app.rs` and `shell.rs` share their structure and differ in pacing,
   GPU setup and devices. An Android driver is closer to `shell.rs` (safe-area insets, back button,
   pause when hidden, redraw only when something changed). Decide before writing the third whether all
   three become one driver with a per-platform trait, as `shell.rs` already has.
3. **The file picker** is Java; only a file descriptor crosses into Rust.
4. **Phone GPUs** were never measured: the budgets come from desktop Chrome with a CPU throttle.
5. **Determinism on ARM** is expected and unverified; `mb-engine/tests/golden.rs` is where a difference
   would show.

## Review: where the code is harder to change than it should be

Findings of a read-through, not yet acted on. None is a fault a player sees.

| Finding | Where | What would fix it |
|---|---|---|
| One file holds uploads, mesh merging, objects, sorting, culling, mirror passes and the frame | `mb-render/src/renderer.rs` (2 950 lines) | split by concern; no behaviour change |
| One state is the title, the course flow, stage select and the options screen | `mb-app/src/menu.rs` (1 400 lines) | options into a state of its own |
| The platform seam is global state: capabilities, info lines and the command queue are thread-locals | `mb-app/src/host.rs` | carry them in `GameCtx`; tests then need no resetting |
| Two drivers repeat touch routing, frame set-up and surface errors | `mb-desktop/src/app.rs`, `mb-web/src/shell.rs` | see "Adding Android", 2 |
| The desktop import runs before the window opens; progress goes to the terminal only | `mb-desktop/src/lib.rs` | acceptable at a second or two; a loading state if it ever is not |
| The browser has a second way to read the game ("live": from the picked file, where the page may not store) | `web/worker.js`, `web/main.js`, `mb-web/src/lib.rs` | drop it if "needs storage" is an acceptable requirement; it is what makes plain-http addresses and private windows work |
| Frame statistics for the browser test ship in the product | `mb-app/src/perf.rs`, `mb-web` | kept so the test measures the real build |
| Calls on the renderer that only tests use; a material flag no file on the disc sets | `set_mirror_capture_whole`, `set_mirror_scale`, `set_color_mul`, `set_tex_matrix`; `LayerInfo::uv_scroll` | remove with their tests |
| Some budget constants are still named after version 0.1.0 | `mb-app/tests/memory.rs`, `xtask`, `scripts/web-e2e.ts` | rename; the numbers are the budgets |
| The shared game library depends on `cpal` and `pollster` | `mb-app/Cargo.toml` | a small native-support crate; kept together because Android will want both |
