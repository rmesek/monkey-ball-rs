# Architecture and porting

How the project is put together, what each part is worth, what a new platform has to supply and where it
will meet trouble. Written 2026-10-13 after the split into one crate per platform. The state of the game
itself (what works, budgets, open items, decisions) is in [PLAN.md](PLAN.md).

No port was started for this document. Everything said about Android comes from reading the code
and from one thing that was run: the game library compiles for `aarch64-linux-android`
(`cargo xtask check` holds that). Nothing has run on a phone.

## The shape

```
                  ┌─────────────── one crate per platform ───────────────┐
                  │  mb-desktop          mb-web + web/        (later)    │
                  │  window, gamepads,   canvas, import       Android /  │
                  │  file dialog, CLI    worker, Web Audio    iOS driver │
                  └──────────┬───────────────┬──────────────────┬────────┘
                             │   the driver contract (below)    │
                  ┌──────────┴───────────────┴──────────────────┴────────┐
   the same on    │  mb-app     the game: states, scenes, HUD, seams     │
   every platform │  mb-render  drawing on wgpu, no window               │
                  │  mb-engine  simulation      mb-game  rules and saves │
                  │  mb-assets, mb-stage, mb-tpl, mb-gma, mb-audio       │
                  │             files and formats, std only              │
                  └──────────────────────────────────────────────────────┘
```

| Crate | Lines (src / tests) | Depends on (third party) | What it is |
|---|---|---|---|
| `mb-assets` | 2 400 / 160 | `ruzstd` | disc image and RVZ readers, LZSS, the game pack format |
| `mb-stage`, `mb-tpl`, `mb-gma` | 3 700 / 2 550 | none | parsers: stagedef, textures, models and display lists |
| `mb-audio` | 2 800 / 290 | none | ADP and MusyX decoders, the PCM mixer |
| `mb-engine` | 8 000 / 4 500 | none | the deterministic 60 Hz simulation |
| `mb-game` | 3 700 / 490 | none | courses, lives, score, unlocks, settings, the `Storage` trait and the slot saves |
| `mb-render` | 7 200 / 3 750 | `wgpu`, `glam`, `bytemuck`, `web-time` | GX materials, passes, particles, mirrors; takes a device and a target view |
| `mb-app` | 15 750 / 4 850 | `wgpu`, `glam`, `web-time`; `cpal`, `pollster` off the web; `winit` only as an optional feature for key codes | the game as a library |
| `mb-desktop` | 980 / 95 | `winit`, `wgpu`, `rfd`, `gilrs` | the desktop program (executable `mb-app`) |
| `mb-web` + `web/` | 2 550 Rust, 1 360 JS and HTML | `winit`, `wgpu` (`webgl`), `wasm-bindgen`, `web-sys`, `js-sys`, `wasm-bindgen-futures`, `web-time`, `console_error_panic_hook` | the browser target |
| `xtask` | 1 540 | none | build tool: the site bundle (with its own gzip), the gates |

Rules the layering rests on, each held by the build:

* Nothing below the platform crates names a window, an event loop or an operating system. `mb-app` has no
  `winit` in its default features; the format, simulation and rules crates have no dependency at all except
  `ruzstd`.
* "Desktop" is a crate, not a `cfg`. Before the split it was spelled `cfg(not(target_arch = "wasm32"))`
  inside `mb-app`, which would have handed the file dialog and the gamepad crate to any other native target.
  The only `cfg(target_arch)` left in shared code separates the browser from native targets where they
  really differ: blocking on the GPU (`gpu::request_gpu`, the windowless test driver, `OffscreenTarget`'s
  read-back) and `cpal`.
* `cargo xtask check` lints `mb-app` and everything under it for `aarch64-linux-android`. It needs the
  target's standard library only (`rust-toolchain.toml` installs it), no NDK and no SDK. It proves that
  shared code compiles there, wgpu and cpal included. It proves nothing about running.

## The driver contract

A platform driver is the only code that knows its platform. It does six things; both existing drivers do
exactly these (`crates/mb-desktop/src/{lib,app}.rs`, `crates/mb-web/src/{lib,shell}.rs`).

| # | The driver supplies | Through | Desktop | Browser |
|---|---|---|---|---|
| 1 | **A surface and a device.** Create the window, the `wgpu` surface, adapter and device; make the `Renderer`; reconfigure on resize | `mb_app::gpu` (`instance_descriptor`, `required_limits`, `pick_surface_format`, `request_gpu` for blocking targets), `mb_render::Renderer::new(device, queue, format)` | blocks with `pollster`; vsync (`Fifo`) | awaits; WebGPU or WebGL2 |
| 2 | **Game files.** A `Content` | `content::open_disc_image(path)`, `content::open_disc_stream(stream)`, or `Content::deferred(source)` for storage that can only be read asynchronously | the image, read in place | a pack in the private file system, prepared per state |
| 3 | **A place for saves.** A `Box<dyn mb_game::Storage>`, wrapped in `SlotStorage` | `mb_game::{Storage, SlotStorage, FileStorage, MemoryStorage}` | files in the per-user directory | private file system or `localStorage` |
| 4 | **Sound output.** An `AudioBackend` | `mb_app::audio::{Audio, AudioBackend}` | `audio_cpal::CpalBackend` (in `mb-app`, any native target) | Web Audio (`mb-web/src/audio.rs`) |
| 5 | **Input.** Write actions, axes, the pointer and touches | `mb_app::input::InputState`, `ui::TouchControls`, `tilt::TiltSteering`, `input::action_for_key` (feature `winit`) | keyboard, mouse, `gilrs` pads, touch | keyboard, mouse as touch, Gamepad API, touch, device orientation, the back gesture |
| 6 | **What only the platform can do or know.** Say what it offers, carry out what states ask | `mb_app::host`: `set_caps`, `set_info`, `drain` -> `Command::{RemoveGameData, SetFullscreen, Tilt, CalibrateTilt}` | `forget_image`, the storage lines | fullscreen, tilt, remove data |

Then, once per frame: build a `GameCtx` (time step, input, renderer, content, size, `ui::Layout` with the
safe-area insets and the scale), call `GameStack::update`, `InputState::end_frame`, take the top state's
camera, and draw (`Renderer::render`, or `prepare` + `draw` to skip a frame that would look the same).

Everything under the contract is shared and is tested without a window by the windowless driver
(`mb_app::headless`, behind the `dev` feature): `mb-app`'s integration tests run the real menu, loading,
play and pause states on an offscreen target.

## What each platform needs

"Shared" means the code is used as it is. "Driver" means the platform's crate has it or would need it.

| Concern | Desktop (works) | Browser (works) | Android in a WebView | Android native (Vulkan / GLES) |
|---|---|---|---|---|
| Entry point, packaging | `main()`, a plain executable | `wasm-bindgen` exports, `web/` page, `cargo xtask build-web` | the built `web/dist` as assets of a host app that somebody writes ([ANDROID_WEBVIEW.md](ANDROID_WEBVIEW.md)); no Rust changes | **new crate**: a `cdylib` with `android_main`, winit's `android-native-activity` or `android-game-activity`, an APK build (cargo-apk, xbuild or Gradle with cargo-ndk), signing |
| Window and surface | once, at start | once, the canvas | the web view's | **differs**: the surface dies on every `suspended` and must be made again on `resumed`; `app.rs` creates window, device and renderer together and has no `suspended`. Device and renderer can stay |
| Graphics backend | DX12 / Vulkan / Metal, GL as a fallback | WebGPU, else WebGL2 | WebGL2 (WebGPU where the WebView has it) | Vulkan, GLES 3 as a fallback. `gpu::required_limits` already gives a GL adapter the WebGL2 baseline. Shared |
| Frame pacing | vsync, polling loop | `requestAnimationFrame`, no redraw of an unchanged frame | as the browser | either works; the browser driver's on-demand redraw is the one that saves battery |
| Game files | path from a dialog (`rfd`) or the command line; read in place | file input -> import worker -> pack | as the browser; the host app must implement the file chooser | **driver**: the system document picker through JNI; it yields a file descriptor, which `open_disc_stream(FileStream::from(file))` reads in place. Remembering the image means a persisted URI permission |
| Saves | `FileStorage::default_dir()` (`%APPDATA%`, XDG, `~/Library`) | page storage | as the browser | **driver**: `FileStorage::new(<the app's files directory>)`; `default_dir()` knows no Android. Shared otherwise |
| Sound | `cpal` (WASAPI / ALSA / CoreAudio) | Web Audio | as the browser; starts at the first touch unless the host app allows autoplay | `cpal`'s Android backend compiles; never heard. `set_suspended` exists for pause |
| Keyboard | winit key codes | winit key codes | as the browser | winit key codes (hardware keyboards) |
| Touch | winit touches -> `TouchControls` | the same | the same | the same code as `app.rs` has today |
| Gamepads | `gilrs` | Gamepad API | Gamepad API | **gap**: `gilrs` has no Android backend, and winit 0.30 is not known to report stick axes there (to be confirmed on a device). Needs its own source |
| Tilt | none | `deviceorientation` -> `TiltSteering` | as the browser (needs a secure context) | **driver**: the sensor through the NDK; `TiltSteering::sample` takes the browser's angles (beta, gamma, screen angle), so the readings must be converted to those |
| Safe area | none | CSS `env(safe-area-inset-*)` | as the browser | **driver**: cut-outs and bars are not in winit; from the activity (content rect) or JNI into `ui::Insets` |
| Back button | Escape | history `popstate` -> `Action::Cancel` while `Game::nav_depth() > 0` | the host app forwards back to the page's history | **driver**: the back key as `Action::Cancel` under the same `nav_depth` rule (in `shell.rs` today, not in `app.rs`) |
| Leaving the app | window hidden: no frames, audio suspended | page hidden: no frames, auto-pause | as the browser | `suspended`: drop the surface, pause, suspend audio. Saves are already written when they happen, never at exit |
| Device lost | not handled | reported, the page says to reload | as the browser | should be handled as the browser driver does |
| Fullscreen | F11 / Alt+Enter, a window matter | `Command::SetFullscreen` | the host app's immersive mode | the activity's immersive mode |
| Log output | `eprintln!` in the driver | the console | the console (`WebChromeClient`) | `eprintln!` goes nowhere; the driver needs a logger. The shared crates print nothing |
| Threads | game thread + cpal's callback | page + import worker, no shared memory | as the browser | as the desktop |

## What will cause trouble

Ordered by how much of a port they are likely to cost.

1. **The Android surface life cycle.** Not a detail of the driver but its main loop: every `GameCtx`
   borrows the renderer, the renderer holds textures and buffers made while a stage loaded ("nothing is
   created on the GPU during play"), and a surface that comes back with another size or format must not
   invalidate them. The renderer already takes its target view per frame and survives a resize, and its
   format is fixed at `Renderer::new`; what is missing is a driver that separates "device and renderer"
   from "window and surface". Neither existing driver does (`WindowState` and `Live` hold all five).
2. **Two winit drivers, soon three.** `app.rs` (590 lines) and `shell.rs` (640) share their structure and
   differ in pacing, GPU setup and devices. An Android driver is closer to `shell.rs` (insets, back,
   auto-pause, redraw on demand, device loss) with `app.rs`'s blocking GPU setup. `shell.rs` was written
   around a `Platform` trait with neutral defaults and could become the one winit driver for all three,
   with `Platform` implemented per crate; that would take the desktop off its polling loop and give it the
   on-demand redraw. It is a real change to how the desktop paces frames and was not made here. Decide it
   before writing the third.
3. **Picking the game file on Android.** There is no file dialog crate; the document picker is Java, the
   result is a `content://` URI, and only a file descriptor crosses into Rust. Reading in place then works
   (`open_disc_stream`). The alternative is the browser's model: import once into the app's own storage
   with `mb_app::import::Importer`. That needs a pack reader that does not hold the pack in memory:
   `gamepack::PackBytes` keeps all 144 MB, and the file-backed readers are in `mb-web` (Blob slices) only.
4. **Gamepads and sensors on Android** have no crate that covers them the way `gilrs` and the browser do.
5. **WebGL2 on phones** is the tightest target the renderer has (a WebView, or GLES 3 natively). The
   budgets were measured in desktop Chrome with a CPU throttle; a phone's GPU, its fill rate for the
   mirror's second target and its drivers were never measured. `GlFenceBehavior::AutoFinish`
   (`gpu::instance_descriptor`) was verified in Chrome on Windows only.
6. **Determinism across architectures is unverified.** The simulation is IEEE `f32` / `f64` arithmetic on
   the game's own trig tables and should be bit-identical on ARM, but it has only run on x86-64 and wasm32.
   One call can reach the platform's `cos` (`math::cos_quarter_turn`, off its table) and the tables fall
   back to computed ones when `main.dol` cannot be read. `crates/mb-engine/tests/golden.rs` is the test
   that would show a difference; nothing depends on cross-device determinism today (no netplay, no replay
   files).
7. **32-bit targets** (`armv7-linux-androideabi`, wasm32 is one already): the parsers' overflow fixes were
   made by inspection; only wasm32 exercises them, and only on valid data.
8. **Memory in the browser** never shrinks (wasm linear memory). The load path is built around that
   (portioned uploads, prepared files released after parsing, music decoded into Web Audio's buffers). A
   native target does not have the constraint and loses nothing by keeping the code.
9. **Linux and macOS desktop** builds are expected to work and have never been built; both CI workflows
   have never run.

## The elements, by importance

**Necessary** cannot be removed without losing the game or a target. **Supporting** holds a promise the
project makes (crash safety, budgets, parity) and could in principle go, at the price named.
**Removable** serves nothing a player can reach, or only history.

| Element | Where | Importance | Platform-specific? | Note |
|---|---|---|---|---|
| Disc and RVZ readers, LZSS | `mb-assets/src/{disc,rvz,lzss}.rs` | necessary | no | the only way in for game data on every target |
| Game pack format, importer | `mb-assets/src/gamepack.rs`, `mb-app/src/import.rs` | necessary for the browser | used by the browser; any platform that cannot read the image in place needs it | atomic import, checksums |
| Parsers | `mb-stage`, `mb-tpl`, `mb-gma` | necessary | no | hostile input never panics (`tests/robust.rs`) |
| Audio decoders, mixer | `mb-audio` | necessary | no (the browser mixes in Web Audio and uses only the decoders) | |
| Simulation | `mb-engine` | necessary | no | the part with the most to lose from a careless change; held by `tests/golden.rs` |
| Particles in the engine | `mb-engine/src/effects.rs` | necessary (display) | no | outside `Sim::hash`, on its own random stream |
| Rules, profile, saves | `mb-game` | necessary | `FileStorage::default_dir` is desktop conventions | two-slot saves are what makes a kill harmless on any backend |
| Renderer | `mb-render` | necessary | no; `offscreen.rs` read-back is native only (tests) | nothing beyond WebGL2's limits |
| States, scenes, HUD | `mb-app/src/{menu,loading,play,play_scene,stage_scene,hud,text,widgets,particles,backdrop,mirror,anim,stage_info}.rs` | necessary | no | two thirds of `mb-app` |
| The seams | `mb-app/src/{host,input,ui,tilt,content,gpu,game}.rs`, `audio::AudioBackend` | necessary | they are the boundary | about 2 400 lines; the contract above |
| Native audio output | `mb-app/src/audio_cpal.rs` | necessary off the web | native targets | kept in `mb-app` so an Android driver gets it for nothing |
| Desktop driver | `mb-desktop` | necessary for the desktop | all of it | |
| Browser driver and glue | `mb-web`, `web/{index.html,main.js,worker.js}` | necessary for the browser | all of it | |
| `web/guard.js` | | supporting | browser | says "unsupported" instead of hanging in an old browser |
| `web/sw.js`, `manifest.webmanifest`, icons | | supporting | browser, hosted | offline start and installation; the page works without them and a WebView does not use them |
| The single-file page | `xtask/src/{bundle,gzip}.rs` (1 200 lines, its own gzip) | supporting | browser | "open one file from disk" is a stated goal; it is also most of the build tool |
| Budget tests | `mb-app/tests/{budgets,memory}.rs`, `mb-engine/tests/effects.rs`, `WASM_BUDGET_BYTES` | supporting | the figures are WebGL2's | they are why the port got faster than 0.1.0 instead of slower |
| Browser check | `scripts/web-e2e.ts` (920 lines, Deno + Chrome) | supporting | browser | the only test of the page; eight minutes |
| Frame statistics in the product | `mb-app/src/perf.rs`, `perf_*` in `mb-web`, `window.mbPerf` | supporting | browser | developer support inside the shipped wasm, kept so the check measures what ships |
| Windowless driver, autopilot, PNG writer | `mb-app/src/{headless,autopilot,image}.rs` (`dev` feature) | supporting | native only | not in any product build; every state test stands on it |
| Golden hashes | `mb-engine/tests/golden.rs`, `tests/golden.rs` of the parsers | supporting | no | what replaces the reference as the guard |
| Parity harness | `mb-engine/tests/{parity.rs,ref/}`, `mb-gma/tests/ref/`, `mb-tpl/tests/ref_hash.ts` (1 590 Rust + TypeScript drivers) | **removable**, see below | needs Deno and a checkout of WebMonkeyBall | skips without them |
| `nod` | dev-dependency of `mb-assets` | removable | | a second disc reader to compare the first with; one test |
| The wavy mirror's distortion | `mb-render/src/{mirror.rs,mirror.wgsl}`, `Distortion`, `Mirror::wavy`; one test | removable | no | the game sets `distortion: None` on the one stage it was for. About 150 lines, a texture binding and a shader branch |
| Renderer calls only tests use | `set_mirror_capture_whole`, `set_mirror_scale`, `set_color_mul`, `set_tex_matrix`, `SpriteBlend::Additive`, `mirror_size`, `object_capacity` | removable with their tests | no | each small |
| Switches with no effect on this disc | `set_world_specular_disabled` + `stage_scene::bg_light_group`; the `ENABLE_UV_SCROLL` branch | removable | no | the reference's rules, ported, and met by no data |
| 0.1.0 baseline tables | `DRAWS_0_1_0`, `DRAWS_ADDED` in `budgets.rs`; `WASM_BYTES_0_1_0`; the 0.1.0 columns of the docs | removable once 0.2.0 is the baseline | no | `DRAWS` and `MIRROR_DRAWS` are what is asserted exactly |
| `docs/PARITY_PLAN.md` | | removable (history) | | the budget tests and docs cite it; fold what is still true into PLAN.md first |
| Local branches `parity/*`, tags `checkpoint/*` | git | removable (history) | | merged; `v0.1.0` and `v0.2.0` are the releases |

Removed in this pass: fog (`Renderer::fog`, never switched on), seven identical copies of the tests' disc
finder. The longer list of what 0.2.0 added beyond its effect, with what each removal would cost, is in
[PLAN.md](PLAN.md), "Candidates for simplification".

## Standing without the reference

The project began as a line-by-line port of WebMonkeyBall, and until 0.2.0 "correct" meant "equal to the
reference". Whether it still needs it:

| Use of the reference | Needed to build or run? | Needed to develop? | Now |
|---|---|---|---|
| The game, the site, the desktop program | no, never was | | nothing at build or run time reads `../WebMonkeyBall` |
| `cargo xtask check`, the browser check | no | | the seven tests that want the reference skip without it (run with the checkout hidden: all skip, nothing fails) |
| Simulation parity (`mb-engine/tests/parity.rs`: 5 tests, Deno) | | until now the only guard of the simulation on the disc's stages | **replaced** by `tests/golden.rs`: all 200 stagedefs through fixed inputs, plus four goal runs, against hashes recorded where parity last passed. The parity tests were run once more on the final code of this pass, and passed |
| Parser parity (`mb-gma`, `mb-tpl` against the TypeScript parsers; `mb-stage`'s table) | | | already doubled by `tests/golden.rs` in each parser (hashes over every file on the disc) |
| Renderer, HUD, audio | | never compared by machine: read and reimplemented | unit tests and pixels of this renderer only |
| Understanding why the code is the way it is | | useful | the porting notes ([ENGINE.md](ENGINE.md), [GMA.md](GMA.md), [GAMEPLAY.md](GAMEPLAY.md)) carry the reasoning; comments name the reference's functions |

**Verdict: the reference can be dropped.** Development can continue with this repository and a disc image
alone. With the result replays dropped there is nothing left to port from it. What changes when it goes:

* "Correct" becomes "what the golden tables and the tests say". A change of behaviour is a decision recorded
  in a commit, no longer a divergence to be explained.
* The parity harness can be deleted whenever convenient (the "removable" row above). Until the first
  deliberate change to the simulation it costs nothing and still works for anyone who has the checkout; after
  it, the harness would report the change as a failure and should go.
* Comments and docs keep naming reference functions. They are provenance, not instructions.
* One thing is not a technical question: this code is a port, a derived work of WebMonkeyBall and through it
  of the SMB decompilation, whatever happens to the checkout. The reference's repository carries no licence
  file and no `license` field; this repository declares `MIT OR Apache-2.0` in `Cargo.toml` and has no
  `LICENSE` file either. The credits in the README must stay, and the owner should settle the licence
  question before publishing more widely. That is for a person to decide.

## Adding a platform

1. A new crate `crates/mb-<platform>`, depending on `mb-app` (with `features = ["winit"]` if it uses winit),
   `mb-render`, `mb-game` and its platform crates. Nothing in `mb-app` should need to change; if something
   does, it is a seam that is missing, and it belongs in `mb-app` for every driver.
2. Implement the six rows of the contract. Start from the driver that is closest (see "What will cause
   trouble", 2).
3. Add its product build to the gates in `xtask/src/main.rs`, the way `clippy -p mb-desktop` and the wasm
   clippy are there.
4. Whatever the platform cannot do becomes a `host::Caps` flag the menus read, never a `cfg` in a state.
5. Verify by running, and write down what was run on what device. A compile check is not a port.
