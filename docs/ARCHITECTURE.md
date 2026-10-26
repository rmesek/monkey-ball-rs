# Architecture

How the program is put together, which part is needed for what, and what a new platform has to supply.
The state of the project (what works, what is open) is in [PLAN.md](PLAN.md).

## The shape

```
 one crate per platform    mb-desktop                    mb-web + web/               the Android app
                           window, gamepads,             canvas, import worker,
                           file dialog, command line     page storage, Web Audio     (own repository)
                                 │                              │                          │
                                 └────────── the six things a platform supplies ───────────┘
 one driver                mb-driver   winit event loop, GPU surface and its life cycle, frame, touch
 the same on every         mb-app      the game: menu, loading, play, HUD, scenes, and the seams
 platform                  mb-render   drawing on wgpu; takes a device and a target, knows no window
                           mb-engine   the 60 Hz simulation          mb-game   courses, score, saves
                           mb-assets   disc image reader, game pack  mb-stage, mb-tpl, mb-gma, mb-audio
                                                                     the game's file formats
 tooling                   xtask (builds the site, runs the gates)   scripts/web-e2e.ts (browser test)
```

Dependencies point downwards only. Nothing below the platform crates names an operating system or a
browser API; nothing below `mb-driver` names a window or an event loop.

## Which part is needed for what

| The game has to | Code | Lines | Notes |
|---|---|---|---|
| **1. Read the disc image and keep the game's files on the device** | | | |
| read RVZ / ISO / GCM, find files in it | `mb-assets/src/{disc,rvz,lzss}.rs` | 1 800 | only third-party piece: `ruzstd` (Zstandard) |
| copy the 382 files the game uses into one pack, atomically | `mb-assets/src/gamepack.rs`, `mb-app/src/import.rs` | 700 | header written last; every file has a checksum |
| show the import on a native platform, the copying on a thread of its own | `mb-app/src/import_screen.rs` | 370 | the image is the platform's to pick and open; its own fonts draw the screen |
| know which files those are | `mb-app/src/content.rs` (the file lists) | 730 | the only place a disc path is spelled |
| keep the pack: a file (desktop, Android), the browser's private file system | `mb-desktop/src/lib.rs`, `mb-web/src/lib.rs`, `web/worker.js` | | the platform's part |
| **2. Play the game** | | | |
| parse stages, textures, models, sound | `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio` | 6 500 | no dependencies; hostile input never panics |
| simulate: physics, collision, stage objects, camera, particles | `mb-engine` | 8 000 | deterministic; held by recorded hashes |
| rules: courses, lives, score, unlocks, settings | `mb-game` | 3 700 | |
| draw: GX materials, particles, shadow, mirror floors | `mb-render` | 7 000 | one renderer on `wgpu`, within WebGL2's limits |
| the states: title, course and stage select, options, loading, play, pause, HUD | `mb-app/src/{menu,loading,play,hud,text,widgets}.rs` | 6 800 | |
| build a stage into a scene and animate it | `mb-app/src/{stage_scene,play_scene,particles,backdrop,mirror,anim,stage_info}.rs` | 4 400 | |
| decide what plays when, mix it | `mb-app/src/audio.rs`, `audio_cpal.rs` (native output) | 2 000 | the browser mixes in Web Audio (`mb-web/src/audio.rs`) |
| take input: keys, stick, pointer, touch, tilt | `mb-app/src/{input,ui,tilt}.rs` | 1 200 | states read actions and axes, never a device |
| run it in a window: event loop, surface and its life cycle, frame pacing, window input | `mb-driver` | 890 | one driver for every platform; the platform's part is the `Platform` trait |
| **3. Survive a sudden exit** | | | |
| saves in two checksummed slots, written when they happen | `mb-game/src/{storage,progress}.rs` | 1 100 | a kill leaves the previous save or the new one |
| an import that is all there or not there | `import::write_pack_file` (temp file, rename); a marker file in the browser | | |
| **4. Not get worse unnoticed** | | | |
| recorded hashes: the simulation of every stage, every parsed file, the disc reader | `tests/golden.rs` in `mb-engine`, `mb-stage`, `mb-tpl`, `mb-gma`; `mb-assets/tests/disc.rs` | 550 | |
| the real states on an offscreen target, by scripted input | `mb-app/tests/{gameplay,pack,pause,rules,mirrors,audio}.rs` on `mb-app/src/headless.rs` | 4 000 | needs the `dev` feature, which the tests switch on |
| budgets: draw calls, GPU objects, heap, texture and pack bytes, wasm size | `mb-app/tests/{budgets,memory}.rs`, `mb-engine/tests/effects.rs`, `xtask` | 1 000 | counted, not timed |
| pixels of the renderer on every backend | `mb-render/tests/*.rs` | 3 700 | |
| damaged files never crash | `tests/robust.rs` in the parsers, `mb-engine`, `mb-game` | 2 500 | |
| the program killed mid-run | `mb-desktop/tests/kill.rs` | 110 | |
| the built site in a real browser | `scripts/web-e2e.ts` | 910 | Deno and Chrome; the only test of the page |

Third-party code: `wgpu`, `glam`, `bytemuck`, `web-time`, `ruzstd` in the shared crates; `cpal` and
`pollster` on native targets; `winit` in the driver and the platform crates; `rfd` and `gilrs` on the desktop;
`wasm-bindgen`, `web-sys`, `js-sys` in the browser. The build tool links `wasm-bindgen-cli-support`.

## The six things a platform supplies

A platform crate is the only code that knows its platform. It implements `mb_driver::Platform` and hands
the driver its event loop, its `Content` and the first state; both existing ones do exactly this
(`crates/mb-desktop/src/{lib,app}.rs`, `crates/mb-web/src/lib.rs`).

| # | Supplies | Through | Desktop | Browser |
|---|---|---|---|---|
| 1 | a window | `Platform::window_attributes`; the driver makes surface, device and renderer | a sized window, fullscreen | the page's canvas, WebGPU or WebGL2 |
| 2 | the game's files, as a `Content` | `content::open_pack_file`, or the import screen (`import_screen::ImportScreen` on a `content::LatePack`) when there is no pack yet, or `Content::deferred` where storage is asynchronous | pack file in the data folder, imported on the game's import screen | pack in the private file system, imported by the page, prepared per state |
| 3 | a place for saves, a `Storage` | `mb_game::{SlotStorage, FileStorage}` | files in the data folder | private file system or `localStorage` |
| 4 | sound output, an `AudioBackend` | `mb_app::audio` | `audio_cpal` (works on any native target) | Web Audio |
| 5 | input | `mb_app::input::InputState`, `ui::TouchControls`, `tilt::TiltSteering` | keyboard, mouse, `gilrs` pads, touch | keyboard, mouse, Gamepad API, touch, device orientation, the back gesture |
| 6 | what only the platform can do | `mb_app::host::Host`, in every `GameCtx` and in `Platform::poll_input`: capabilities and info in, commands out (`RemoveGameData`, `SetFullscreen`, `Tilt`, `CalibrateTilt`, `PickImage`, `GameDataReady`) | remove data, pick an image | remove data, fullscreen, tilt |

The driver does the rest once per frame: build a `GameCtx`, call `GameStack::update` and
`InputState::end_frame`, draw with the top state's camera, present. It keeps the device and the renderer
when the platform takes the surface away (`suspended`) and makes a new surface at `resumed`; it pauses the
stage and lets go of held input when the window is hidden, draws nothing for a frame that would look like
the last, and ends the run with a message when the device is lost.

Rules that keep this true:

* Code for one platform lives in that platform's crate. No `cfg(target_os)` in shared code; the
  `cfg(target_arch = "wasm32")` there separates "may block" from "must await", and in `mb-driver` also
  what the browser does differently (the surface it keeps, the animation frame that paces it).
* What a platform cannot do is a `host::Caps` flag the menus read, never a `cfg` in a state.
* `cargo xtask ci` compiles `mb-app` and everything under it for `aarch64-linux-android`, so shared code
  cannot quietly become desktop-only. It is a compile check; nothing has run on a phone.

## Adding Android

The Android app lives in a repository of its own (`monkey-ball-android`) and takes this one as a git
submodule at a tagged version; none of its code is here, and nothing here has run on a phone. What the
crates give it, in both ways the app can run the game:

| | In a WebView | Native (Vulkan, GLES as fallback) |
|---|---|---|
| What it is | the built `target/site` shown by a small host app | a `cdylib` crate in the app's repository: a `Platform` of `mb-driver` |
| Rust to write | none | the entry point, the `Platform`, the data directory |
| Game data | the page's import, as in a browser | `ImportScreen` copies the image into `game.pack` in the app's files directory, from the file descriptor the document picker hands over (`FileStream::from(File)`, `Importer::open`) |
| Saves | page storage | `SlotStorage::new(FileStorage::new(<files directory>))` |
| Sound | Web Audio | `audio_cpal` (AAudio; compiles for Android, never heard) |
| Touch, back button | as in the browser | the driver's; the back button arrives as `Cancel` |
| Leaving the screen | the page's | the driver's: a new surface at `resumed`, device, renderer and states kept |
| Tilt (gyro) | `deviceorientation`, already there | the sensor through the NDK into `TiltSteering`; the seam is ready, the source is missing |
| Gamepads | Gamepad API | missing: `gilrs` has no Android backend |
| Guide | [ANDROID_WEBVIEW.md](ANDROID_WEBVIEW.md) | the steps below |

**A native app, step by step.** Not compiled here (there is no Android NDK on the machine this was
written on); each step names the code that already does the same on the desktop.

1. Depend on `mb-driver`, `mb-app` and `mb-assets` by path, and on `winit` with its
   `android-native-activity` feature ([README](../README.md), "Using the game from another project").
2. In `android_main(app)`, build the event loop with `EventLoop::builder().with_android_app(app)`; the data
   directory is `app.internal_data_path()`.
3. Say what the platform offers in a `Host`: `remove_data`, not `quit` (the system closes apps), and the
   info lines that name where the data is kept.
4. With a complete `game.pack` (`content::open_pack_file`), run `mb_driver::run` on it with the title menu.
   Without one, run it on a `LatePack` with an `ImportScreen` in front of the menu: `start` in
   `mb-desktop/src/lib.rs`.
5. In `Platform::poll_input`, on `Command::PickImage` start the document picker (Java). The picked file's
   descriptor comes back on another thread: send it through a channel, and at the next `poll_input` open
   it with `Importer::open` and offer it to the `ImageSlot`, or say why it cannot be used (a toast) and
   start the picker again. On `Command::RemoveGameData`, end the run and delete the pack once it is closed,
   as the desktop does.
6. In `Platform::on_visible(false)`, suspend the `Audio`.

**Where a native app keeps the game.** The image is needed once, at first setup. The import screen copies
the 145 MB the game uses out of it into `game.pack` in the app's internal files directory, where the saves
go too. From then on the app opens that pack and never asks for the image, which the user may delete.
Internal storage belongs to the app: it needs no permission, no other app can read it, and it lasts until
the user clears the app's data or uninstalls it, which is exactly when the game should ask again. No
permission on the original file has to be kept. The pack should be left out of the system's cloud backup
(it is far over the size limit and can be imported again); the saves can stay in. This is the desktop's
model with another directory, and the code for it is the code the desktop already runs.

What nobody knows until it runs on a phone: the surface made again at `resumed` (no platform here takes
the surface away), the speed of phone GPUs (the budgets come from desktop Chrome with a CPU throttle),
`cpal`'s output, and determinism on ARM (`mb-engine/tests/golden.rs` is where a difference would show).

Two traps for whoever writes it: the engine's trig tables are installed once per process, and an Android
process outlives its activity; the Android check in the gates compiles `mb-app` only, because `winit`'s
Android backend and the link need the NDK. Threads are no trap: the driver calls every `Platform` hook on
the game's thread and hands it the `Host` there, so what a Java callback reports goes to the platform first
(a channel, an atomic) and into the `Host` at the next `poll_input`; nothing in the game is global.

## Review: where the code is harder to change than it should be

Findings of a read-through that are not acted on yet (the two drivers, the global platform state and the import before the window were, for 1.0.0). None is a fault a player sees.

| Finding | Where | What would fix it |
|---|---|---|
| One file holds uploads, mesh merging, objects, sorting, culling, mirror passes and the frame | `mb-render/src/renderer.rs` (2 950 lines) | split by concern; no behaviour change |
| One state is the title, the course flow, stage select and the options screen | `mb-app/src/menu.rs` (1 400 lines) | options into a state of its own |
| The options screen is addressed by row number | `mb-app/src/menu.rs` and its tests, `mb-app/tests/gameplay.rs`, tap positions in `scripts/web-e2e.ts` | rows found by name in tests; removing three options for 0.3.0 meant edits in all three |
| The browser crate and the driver have no Rust tests of their own (1 900 and 890 lines) | `mb-web`, `mb-driver` | `scripts/web-e2e.ts` covers the browser and the driver there, the kill test and timed runs the driver on the desktop; the surface made again at `resumed` has not run anywhere |
| The engine's trig tables are installed once per process | `mb-engine/src/tables.rs`, `Content::install_trig_tables` | fine for one game per run; a pack of another region imported without a restart keeps the first one's tables |
| Frame statistics for the browser test ship in the product | `mb-app/src/perf.rs`, `mb-web` | kept so the test measures the real build |
| Calls on the renderer that only tests use; a material flag no file on the disc sets | `set_mirror_capture_whole`, `set_mirror_scale`, `set_color_mul`, `set_tex_matrix`; `LayerInfo::uv_scroll` | remove with their tests |
| Some budget constants are still named after version 0.1.0 | `mb-app/tests/memory.rs`, `scripts/web-e2e.ts` | rename; the numbers are the budgets |
| The shared game library depends on `cpal` (native sound) | `mb-app/Cargo.toml` | a small native-support crate; kept together because Android wants it too (`pollster` is only for the windowless test driver now) |
