# Browser build

The game runs in a browser as WebAssembly (`crates/mb-web`) behind a small static page (`web/`). The page
contains the engine only. The player supplies their own disc image through the file picker (or by dropping it
on the page); it is read on the device and never uploaded. The page makes no network requests except for its
own bundle files, and its Content-Security-Policy forbids anything else.

The output is a **static site**: plain files, relative paths only, no server-side component, no dependency on
a service worker, WebGL2 as a first-class backend. It can be hosted by any static file server, installed as a
PWA, or placed inside another application's web view (see [ANDROID_WEBVIEW.md](ANDROID_WEBVIEW.md)).

## Build

Needs Rust and nothing else: the `wasm32-unknown-unknown` target comes with the toolchain
(`rust-toolchain.toml` names it), and the build tool links wasm-bindgen's post-processing as a library at
the version of the `wasm-bindgen` crate (both pinned; a test compares them).

`cargo web` builds the site and serves it on `http://localhost:8080` (`--port N` for another port).
`cargo xtask build-web` only builds it, into `web/dist/` (git-ignored):

```
cargo xtask build-web [--dev]
```

It runs `cargo build -p mb-web --target wasm32-unknown-unknown --profile web` and wasm-bindgen's
`no-modules` output, holds the module to its size budget (below), and assembles the directory. The same command works in any shell on any
platform: it is a small Rust program (`xtask/`; `cargo xtask` is an alias in
`.cargo/config.toml`), so there are no shell scripts and nothing else to install:

| File | What |
|---|---|
| `index.html` | the page (stamped with a version hash of the bundle) |
| `guard.js` | a few lines of old-style script loaded first: says so when a later script cannot load or parse (a browser below the supported versions) |
| `main.js`, `worker.js` | the page's script and the import worker (classic scripts, no modules) |
| `pkg/mb_web.js`, `pkg/mb_web_bg.wasm` | the engine: 4.0 MB wasm (1.45 MB gzipped), 0.19 MB glue |
| `sw.js`, `manifest.webmanifest`, `icon*.{svg,png}` | offline start and installation (optional at run time) |

`-Dev` / `--dev` builds unoptimised and keeps the function names in the module. The `web` profile (workspace
`Cargo.toml`) is `release` plus fat LTO, one codegen unit and `panic = "abort"`; the optimised build also
drops the module's name section (1.1 MB; a panic still reports its message and source line). `wasm-opt` is
not used (not required; not installed on the build machine). The module is built with rustc's default
feature set for `wasm32-unknown-unknown` (see "Browser requirements").

**Size budget.** Every visitor downloads and compiles the module, so it may not grow by accident: the build
prints its size and fails, before `web/dist` is touched, when the optimised module is larger than
`WASM_BUDGET_BYTES` in `xtask/src/main.rs` (4 127 298 bytes). The comment on the constant says how to raise it on purpose. `--dev` builds are not
held to it.

| | 0.1.0 | 0.2.0 |
|---|---|---|
| `pkg/mb_web_bg.wasm` | 3 877 298 bytes (1.39 MB gzipped) | 4 016 134 bytes (1.45 MB gzipped): +138 836, 111 164 bytes of the budget left |
| `pkg/mb_web.js` | 192 362 bytes | 193 611 bytes |
| `monkey-ball.html` (the single file, removed after 0.2.0) | 2 107 204 bytes | 2 185 866 bytes |

## Run

Any static server works. The reference one is Python's:

```
cd web/dist
python -m http.server 8080
```

then open `http://localhost:8080/`. Add `--bind 0.0.0.0` to reach it from a phone on the same network at
`http://<this machine's address>:8080/`; plain http on a network address is not a secure context, so there the
browser offers no private storage, no service worker and no WebGPU, and the page plays from the picked file
for the session on WebGL2 (see below).

What `python -m http.server` implies, and how the page copes:

* `.wasm` is served as `application/wasm` by current Pythons; the page compiles with `compileStreaming` and
  falls back to compiling from the bytes when a server labels it otherwise.
* No cache headers: every script and the wasm are requested with the build's version (`?v=<hash>`), so an
  old script is never paired with a new engine, and the service worker's cache is named by the same hash.
* No COOP/COEP headers: nothing here needs `SharedArrayBuffer` or threads. No range requests are needed.

### Not from disk

The site has to be served. A page opened from a `file://` address may not fetch the module or start the
worker from the files next to it, and says so. Until 0.2.0 the build could also write the whole page into
one file that did open from disk (`monkey-ball.html`); it was removed with its 700 lines of build tool
(a gzip compressor among them). To play without a server, use the desktop program.

### Page parameters

| Parameter | Effect | For |
|---|---|---|
| `?gl=1` | use WebGL2 even where WebGPU is available | a machine whose WebGPU misbehaves; the browser check |
| `?scale=0.5` | render at that fraction of the device resolution (0.25 to 1) | weak GPUs |
| `?autoscale=0` | do not lower the resolution automatically when frames stay slow | measuring |
| `?fps=1` | show the frame timing overlay | measuring |
| `?nostore=1` | do not keep the game data (play from the picked file) | trying the page without leaving 144 MB behind; the browser check |
| `?tilt=1` | offer tilt control on a device without a touch screen | the browser check (emulated sensor) |

For the console and for automated checks: `window.mbStats` holds what happened at start-up (mode, import
time, pack size, save mode, why nothing is stored), `window.mbPerf` frame timings and load breakdowns
(`summary()`, `frames()`, `counts()`, `total()`, `lastLoad()`, `status()`, `preparedBytes()`,
`wasmMemoryBytes()`, `audio()`; nothing is recorded until `enable()` or `?fps=1`, and a row is only built when
one of them is called), and
`window.mbBack()` takes one step back inside the game (false at the title menu).

## The game file

The page asks for a disc image of **Super Monkey Ball** (Nintendo GameCube, 2001):

| | |
|---|---|
| Formats | RVZ with no compression or Zstandard; plain ISO / GCM |
| Discs | USA `GMBE8P` (the tested one), Europe `GMBP8P`, Japan `GMBJ8P` (accepted by id; not tested here) |
| Typical file | `Super Monkey Ball (USA).rvz`, about 475 MB |
| The tested RVZ file | 497,668,128 bytes, SHA-1 `AFAC5395934DF849552396C43EDDA0BA064B9E3C`, SHA-256 `CE2D2DFA75EE4CED285538233B62CD73FB9561DD8F7C109AA7789692DE6271DC` |

A different dump or compression of the same disc has a different file hash and is accepted just the same: the
game identifies the disc by its id and validates the headers and tables it reads; it never hashes the whole
file (that would cost a phone time and battery for nothing). Every other file gets a message that says what it
is and what is supported (other RVZ compressions, WIA, GCZ, CISO, WBFS, Wii discs, other games, empty,
truncated or damaged files), and the picker stays usable.

Reading is done by the project's own code on every target (`crates/mb-assets`): `rvz.rs` (the RVZ container)
and `disc.rs` (GameCube boot header, file table, `main.dol`). Sizes and offsets from the file are checked
against the format and the file before anything is allocated or read; tests feed truncated, bit-flipped and
hostile-size inputs under an allocation-counting allocator. `crates/mb-assets/tests/disc.rs` holds what the
readers return for the USA disc by recorded hashes: a sample in every test run, and the whole image with
`cargo test --release -p mb-assets --test disc -- --ignored the_whole_image_reads_as_recorded`
(1,459,978,240 disc bytes, 897 files, `main.dol`). The hashes were recorded in a run in which a second
reader, the `nod` crate, returned the same bytes for the whole image; `nod` is no longer a dependency.

## Storage model

### What is kept, and where

| | Browser | Desktop |
|---|---|---|
| Game data | one **pack** file in the page's Origin Private File System: `monkey-ball/game.pack` plus the marker `monkey-ball/game.json` | the same pack as one file, `game.pack`, in the per-user data directory, written under another name and renamed when complete |
| Saves | files in `monkey-ball/saves/` next to the pack; where the private file system is unavailable, `localStorage`; where neither works, memory for the session, and every save says so in the menu. Exactly one of the three, never two. | the per-user data directory (`%APPDATA%\monkey-ball-rs`, `~/Library/Application Support/monkey-ball-rs`, `$XDG_DATA_HOME/monkey-ball-rs`; `MB_SAVE_DIR` overrides) |

The pack holds only what the game reads, 144.1 MB of the 475 MB image (382 of 897 files, USA disc):

| Category | Size |
|---|---|
| Stages (stagedef, models, textures of the 109 course stages) | 70.4 MB |
| Music (the 10 tracks the courses use, intro + loop) | 45.5 MB |
| Backgrounds | 18.4 MB |
| Sound effects (the two banks the main game uses) | 4.5 MB |
| HUD and fonts | 2.0 MB |
| `main.dol` (trig tables, sound table) | 2.0 MB |
| Shared models (ball, bananas, goal, clock digits, particles) | 1.2 MB |

Not copied: minigames, the other sound banks, unused music, previews, replay data, character models. The list
is one function, `mb_app::content::all_files`, built from the same per-stage file list the stage loader reads
through; a test checks that the pack contains exactly that and that every stage of every course loads from it
without a single unprepared read.

### Rules

* **Availability.** The private file system needs a secure context (`https`, `http://localhost`) and a
  browser that has it; private windows and some web views do not. Then, or when the quota is too small or a
  write fails, the page does not store anything: the import worker stays alive and hands the game single
  files straight from the picked image for the session (still never holding the image in memory), and the
  file is asked for again next time.
* **Space.** About 145 MB; `navigator.storage.estimate()` is checked first (160 MB must be free where the
  browser reports a quota), and a write that still comes up short ends the import cleanly. Either way the
  page then plays from the picked file, and the options screen says why nothing is kept.
* **No duplicates.** There is one pack. Importing again, the same disc or another region, replaces it.
* **Expiry.** A browser may clear a site's storage under pressure. After an import the page asks for
  persistence (`navigator.storage.persist()`); the landing page and the options screen say whether the data is
  "kept" or "may be cleared". If the data is gone or incomplete, the page says so and asks for the image again.
  The desktop program's data never expires.
* **Versioning.** The marker records the pack format version (`mb_assets::gamepack::VERSION`). A page with
  another version deletes the old pack and asks for the image once more.
* **Reclaiming.** The page says before the import that it keeps game data and saves on the device. In the
  game, Options shows what is kept and where, with "Remove game data" (deletes the pack, returns to the
  import screen, keeps the saves) and "Erase save data", each asking for a second tap. The landing page and the
  error dialog also offer the removal. On desktop, Options shows the data folder with the same two rows; "Remove game
  data" closes the program, which asks for the image at its next start.

### Sudden exit

Closing the tab or killing the process at any moment leaves the old complete state or the new complete one.

* **Import.** The pack's header is written last and the marker after that; a pack without a valid marker is
  an interrupted import and is deleted at the next start. The worker reads the whole pack back and compares
  checksums before writing the marker. Every file in the pack carries a CRC-32 that is checked again when it
  is read during play.
* **Saves.** `mb_game::SlotStorage` keeps every value in two slots, each record with a generation number and
  a checksum; a write always replaces the older slot. A read takes the newest complete record, so a write cut
  short costs nothing but itself, on any backend. Below it, the desktop writes a temp file, syncs it and
  renames it into place; the browser uses `createWritable()` (write, then swap) or one `localStorage` item.
  Saves are written when they happen (after each stage result, on every options change), not at exit.
* **Play.** Nothing is written while a stage runs; a stage that ends writes its result once.

Tests: a unit test kills the save backend at every operation with every length of torn write (over 800
crash points) and checks the value read afterwards is the old or the new one; a test replays the importer's writes
and cuts them off after every one, checking the result is never accepted as a pack; `crates/mb-desktop/tests/kill.rs` kills the
real desktop program mid-run three times and reads its saves; the browser check closes the tab mid-play and
mid-import and restarts.

Two tabs of the game on one origin share the saves without coordination: each works on its own copy and the
tab that saves last wins.

## How the browser build works

```
picked File --> import worker (worker.js + the same wasm module)
                  reads with FileReaderSync, a megabyte or one RVZ group at a time
                  |-- "store": writes game.pack with a sync access handle, verifies, writes game.json
                  `-- "live":  keeps the File, serves single files on request
page (main.js + wasm)
   GameData = pack Blob + its index   |   live file service
   state needs files -> prepare (async slices of the Blob / a message to the worker) -> loading screen
        -> the usual synchronous loaders read from memory -> files released
```

* The worker runs the same wasm module as the page (compiled once, posted to the worker), so there is one
  artifact and one copy of the readers. It is the only place with synchronous file reads.
* The game loop stays on the main thread. A state that needs files asks for them first
  (`Content::prepare`, polled once per frame), shows a loading screen meanwhile, loads, and releases them
  (`Content::release`). Reading a file that was not prepared is a loud error and is recorded
  (`mbPerf.misses()`, always empty).
* **Stage loading** (`mb_app::loading`): fetch, parse, music, build, one phase per frame so the loading
  screen is up before the work starts. Only textures the stage's models use are decoded and uploaded
  (stage 1: 68 instead of 124; Expert 50: 105 instead of 292). The shared files and the last background stay
  parsed between stages (about 1.2 MB + 0.5 to 3.5 MB). A stage that cannot be fetched or parsed returns to the
  menu with a notice; the session goes on.
* **A lost life** in course play restarts the floor on the scene that is already loaded (nothing is read or
  built), as practice always did.

## Mobile

* Menus work with touch alone: drag to scroll (with a short glide), page arrows and a scroll bar beside long
  lists, a Back button in the top-left corner of every screen but the title (and on the pause screen). A tap
  is a press and release without movement, however long; a drag never activates.
* The browser's back gesture goes one step back inside the game while it is in a sub-menu or a stage (one
  history entry is kept for that, added and removed when the game says so, with no polling); at the title it
  leaves the page as usual.
* On a touch device the file input asks for generic binary files (`application/octet-stream` and the ISO
  types), not for extensions: a phone's chooser works with media types and knows none for `.rvz` or `.gcm`.
  It does not ask for "any file" either, because a chooser then also offers the camera and the sound recorder,
  and Firefox for Android asks for the camera and "record audio" permissions, which the page never uses. A
  second button, "File greyed out? Show all files", drops the filter for a file manager that reports the image
  under another type (that one can bring the permission question back; denying it is harmless). The file is
  judged by its content. Not tried on a phone.
* Playing from the picked file (nothing stored), the file can vanish mid-session: moved, deleted, or the
  permission a phone gave for it has ended. The game then cannot load another stage; the page says "The game
  file can no longer be read" and offers the way back to the picker. Progress is already saved.
* A hidden page (another tab or app in front) produces no frames, stops its audio clock and pauses the stage;
  nothing runs on a timer while the game is up.
* Options: fullscreen (with an attempt to lock landscape), controls by device tilt (calibrated to the pose
  the device is held in when a stage starts; iOS asks for permission at the next tap), reflections, remove
  game data, erase save data.
* **Reflections** (on by default on every platform; kept in the save, so it survives a reload and comes
  out as on from a save 0.1.0 wrote) draws the mirror floors of the 37 stages that have one, with one more
  pass over what the mirror shows. "Mirror floors in the browser" below has what it costs; a player whose
  device is too slow for it turns it off here.
* The screen is kept awake while the page is visible (Screen Wake Lock, re-acquired on return).
* Portrait and landscape both work; no rotate hint is shown.
* A lost graphics context or a fatal error shows a dialog with one button that restarts the page, which is
  quick now (the title menu is back in a fraction of a second).

## Browser requirements

**Chrome or Android System WebView 97, Safari 16, Firefox 79, or newer.** Only Chrome 154 on Windows was
run; the other numbers are derived from what the build uses and the browsers' documentation.

| Needed for | Feature | Chrome / WebView | Safari | Firefox |
|---|---|---|---|---|
| running at all | WebAssembly with reference types, bulk memory, non-trapping conversions, sign extension, multi-value (rustc 1.98's defaults for `wasm32-unknown-unknown`; the built module has two tables and uses `memory.copy`/`memory.fill`) | 96 | 15 | 79 |
| running at all | `'wasm-unsafe-eval'` in the page's Content-Security-Policy (an older engine refuses to compile WebAssembly under the policy) | 97 | 16 | any (applied to WebAssembly from 102) |
| running at all | WebGL2, or WebGPU; `Worker` with `FileReaderSync`; `Blob.arrayBuffer` | 76 | 15 | 69 |
| keeping the game data | Origin Private File System with `createSyncAccessHandle` | 102 | 15.2 | 111 |
| saves as files | `createWritable` (else `localStorage`, else memory) | 86 | 26 | 111 |

The module is deliberately not built for an older WebAssembly feature level (`-C target-cpu=mvp`): the policy
already sets the floor at Chrome 97, above every WebAssembly feature used, so it would buy nothing.

What the page does where something is missing:

* **Cannot run** (no WebAssembly, the engine refuses the module, no `Worker`, neither WebGL2 nor WebGPU, a
  script that does not load or parse): a message on the landing page that names what is missing and the
  versions above. There is deliberately no import path without a worker: every environment the site is meant
  for has dedicated workers and `FileReaderSync`, the import is the one place that needs synchronous reads,
  and a second, main-thread reader would be a second code path to keep correct for no known user.
* **No private file system** (plain http on a network address, a page opened from disk, private windows, some
  web views): plays from the picked file for the session.
* **No `localStorage`** (blocked site data; a web view with DOM storage off, where it is `null`): saves last
  until the page closes and every save says so.
* **No service worker, WebGPU, fullscreen, wake lock, orientation lock, device orientation, gamepads,
  `storage.persist`, `compileStreaming`:** each is optional and its absence changes nothing else.
* **Audio blocked by the autoplay policy:** the context is resumed at the first key, pointer or touch.
* **A lost graphics context or device:** a dialog with a restart button.

## Measurements

Chrome 154, Windows 11, RTX 4080, the tested RVZ, a window of 1100 x 860 (canvas 1084 x 765).
`scripts/web-e2e.ts` takes and prints the 0.2.0 column on every run; the figures are those of three runs in
a row on 2026-10-13, with their spread where there was one. The display ran at 240 Hz (a frame every 4.2 ms)
for the 0.2.0 column and at 165 Hz for 0.1.0, which changes how many frames a stretch has and what a frame's
p99 is, not the update and render times.

| | Before 0.1.0 (image held in the tab) | 0.1.0 | 0.2.0 | How it is taken |
|---|---|---|---|---|
| JS heap with the game running | 509 MB | 2 MB | not taken again | DevTools, by hand |
| wasm memory after floors 1 and 2 of Beginner | (plus the above) | 22 to 23 MB | 19.1 MB | `mbPerf.wasmMemoryBytes()` in the page that did the import, after Course Play floor 1, the goal and a fall on floor 2. Checked: at most 2 MB over 0.1.0 |
| wasm memory with Expert 50 loaded | | 28 MB | 23.5 MB | the same, in a page started from the stored data that loaded Practice floor 2 and then Expert 50. Checked: at most 2 MB over 0.1.0 |
| wasm memory, one stage in a fresh page, six seconds into play | | stage 89: 26.1 MB, 76: 22.2 MB, 21: 28.1 MB, 90: 25.3 MB | 19.1 MB on each of them (89, 76 and 21 with Reflections on and off) | a page that loaded only that stage; 0.1.0 from a build of the `v0.1.0` tag, measured the same way. Checked for stages 89, 76 and 21 with Reflections on: at most 2 MB over 0.1.0 |
| Music of the current stage (float samples held by the browser, outside wasm memory) | | 24 MB (`st1`) to 54 MB (`st2`) | 24 MB and 33 MB measured, the same tracks | `mbPerf.audio().musicBytes` |
| Textures on the GPU (RGBA8 with mip levels), heaviest stage | | 16.6 MB (stage 109, 78 textures); 6 to 15 MB typical | 16.7 MB (stage 109, 82 textures: the four particle textures) | counted by the loader; `crates/mb-app/tests/budgets.rs` holds it |
| Mirror target (colour and depth, half the frame each way), on the 37 mirror stages with Reflections on | | none | 2 bytes per pixel of the frame: 1.7 MB under this canvas, 1.84 MB under 1280 x 720 | `Renderer::mirror_bytes`; not in wasm memory |
| Game files held in memory between loads | all | 0 bytes (largest single stage fetch: 10.6 MB, released after the load) | 0 bytes | `mbPerf.preparedBytes()`, checked |
| First run: pick to menu | read 475 MB every visit | 1.3 s import (copy 144 MB, verify), once | 1.4 s | `mbStats.importMs` |
| Later runs: page load to first frame | same as first | 0.1 s after the engine is compiled; no picker | 0.1 s | `mbStats.firstFrameMs` |
| Whole browser during import (12 processes, private bytes) | | +132 MB at the peak, back to baseline after | not taken again | Task Manager, by hand |
| Stage load, Beginner 1 / Expert 50, WebGL2 | 176 ms / 432 ms at 6x CPU throttle | 82 ms / 28 ms unthrottled; Expert 50 at 6x: 137 ms | not taken again | `mbPerf.lastLoad()` |
| A still menu | | 1.5 % of one core (no redraw without change) | 1.5 to 2.0 % | DevTools `TaskDuration` over 4 s, checked below 5 % |
| Play, at the display's rate | | 21 % of one core on the main thread (165 fps) | 15 to 17 % (240 fps) | the same over 3 s of Beginner 1 |
| Play, Beginner 1, WebGL2, 6x CPU throttle | | update 0.16 + render 5.1 ms per frame (275 draws) | update 0.11 to 0.12 + render 3.09 to 3.32 ms (183 draws) | Practice, the ball standing where it is put down, 8 s from 1.5 s after GO. Checked: the sum within 1.5 times 0.1.0, no more draws than 0.1.0's 275 plus the particles' 12 |
| The same on WebGPU | | not taken | update 0.18 + render 0.87 ms; 0 long of 1930 frames, p99 of the frame 4.5 to 4.6 ms | the same without `?gl=1`. Checked: p99 and long frames as for Expert 50 |
| Frames, 30 s of steering, Expert 50, WebGL2, 6x throttle | 2 long of 4963 | 2 long of 4917, p99 6.6 ms (165 Hz) | 1 to 4 long of 7202 to 7213, p99 4.5 ms (240 Hz) | Practice, the keys W, D, S, A held 0.7 s each in turn (falls and restarts included). A frame is long above 1.5 times the median. Checked: p99 within 1.5 times 0.1.0, long frames within three times its share |
| Frames, 30 s, Beginner 2, WebGPU | 0 of 5783, p99 6.6 ms | 1 of 4997, p99 6.5 ms | not taken again | by hand |

Not measured: any real phone, iOS, an Android web view, Firefox, Safari.

How the frame figures are taken, since they depend on it:

* `?gl=1&autoscale=0`, a fresh page per scenario, `mbPerf.enable()`, DevTools `Emulation.setCPUThrottlingRate`
  with rate 6 from the stage's first frame (Beginner 1 and Expert 50: from GO). Update and render are the
  engine's own clocks around the game update and around acquire, encode, submit and present
  (`mb_app::perf::FrameSample`); every frame of the stretch is read (`mbPerf.frames()`, `counts()`,
  `total()`), not a sample.
* The throttle is coarser than its number, and it is not the same in every tab: with rate 6 the first tab of
  a browser session ran Beginner 1 about 6.7 times slower than unthrottled, a tab opened later in the session
  8.6 times (measured before the meshes were merged: render 0.68 ms became 4.5 ms and 6.0 ms). The script
  therefore measures in the session's first tab, where a build of the `v0.1.0` tag gives 0.10 to 0.14 + 4.5
  to 4.9 ms today, against the recorded 0.16 + 5.1 ms.
* A check takes a failed timing a second time before it fails. Over the three runs none needed it.
* The limits scale with the display: the p99 limit of Expert 50 is 1.5 times 0.1.0's 6.6 ms, stretched by
  the display's frame over 0.1.0's 6.06 ms where the display is slower.
* Long frames in Expert 50: 0.1.0's figure is 2 of 4917, and a build of the `v0.1.0` tag gives 2 and 3 today.
  A count that small cannot be held to 1.5 times itself without failing by chance, so the limit is three
  times its share of the frames (9 of 7200).

### Why 0.2.0 needs less wasm memory and less time than 0.1.0

0.2.0 draws more than 0.1.0 (particles, flipbooks, mirror floors) and its first builds needed more of both:
the mirror stages took up to 36.6 MB of wasm memory against a budget of 2 MB over 0.1.0, and stage 89 with
its mirror did not fit a 60 Hz frame at 6x throttle (16.9 to 17.2 ms at p99). What changed that is in
`docs/GMA.md`, "Meshes, uploads and memory"; as far as the page is concerned:

* **Memory.** wasm memory never shrinks, so a page is as large as the largest moment of its heaviest load.
  On WebGL2 `wgpu` keeps a copy of every texture and buffer it is handed until the next submit, and frees
  what a submission used only when the browser reports its fence as passed. A stage load used to hand over
  everything and submit once, so the stage's textures and meshes were in wasm memory twice at that moment.
  Loads now submit whenever a megabyte is staged, and the engine waits for the driver at its own fences
  (`GlFenceBehavior::AutoFinish`, `mb_app::gpu::instance_descriptor`). Every page measured here now stays
  at 19.1 MB, which is below what 0.1.0 needed for the same stage (22.2 to 28.1 MB).
* **Time.** The models' meshes are merged per pipeline and material at upload, so a frame has 15 to 50 %
  fewer draw calls than in 0.1.0 (Beginner 1: 183 instead of 275), and the cost of a frame on WebGL2 is its
  calls. The mirror's second pass is cut to what the mirror models in view can show.

### Mirror floors in the browser

Reflections are on by default on every platform. The three mirror stages that drew the most in 0.1.0, WebGL2
at 6x CPU throttle, nobody steering, from the first frame of the fly-in to six seconds after GO (the
simulation is deterministic, so every run draws the same frames). "0.1.0" is a build of the `v0.1.0` tag
measured the same way (two runs, of which the second shared the machine with a compiler for a moment: its
p99 figures are the upper ends). Draws are the median of the stretch and, in brackets, the most in a frame.

| Stage | | Update + render, fly-in | in play | p99 of update + render | Draws, fly-in | in play | wasm memory |
|---|---|---|---|---|---|---|---|
| 89 (Expert 49) | 0.1.0 | 0.11 to 0.13 + 6.1 to 6.7 ms | 0.15 + 4.0 to 4.2 ms | 9.1 to 12.0 ms | (598) | (312) | 26.1 MB |
| | 0.2.0, Reflections off | 0.11 to 0.12 + 3.4 to 3.6 ms | 0.16 to 0.17 + 2.9 to 3.0 ms | 5.8 to 5.9 ms | 288 (398) | (413) | 19.1 MB |
| | 0.2.0, Reflections on | 0.14 to 0.15 + 6.1 to 6.6 ms | 0.17 to 0.20 + 3.9 to 4.1 ms | **9.8 to 10.1 ms** | 286 (398) + 216 to 220 (323) for the mirror | (412) + (381) | 19.1 MB |
| 76 (Expert 36) | 0.1.0 | 0.16 + 5.1 ms | 0.19 to 0.21 + 6.7 to 6.9 ms | 7.9 to 10.3 ms | (448) | 506 | 22.2 MB |
| | 0.2.0, Reflections off | 0.13 to 0.14 + 3.3 to 3.5 ms | 0.16 to 0.19 + 4.1 to 4.4 ms | 5.1 to 5.6 ms | 220 to 226 (350) | 320 (323) | 19.1 MB |
| | 0.2.0, Reflections on | 0.16 to 0.18 + 5.4 to 5.5 ms | 0.22 to 0.25 + 7.3 to 7.8 ms | **9.4 to 10.4 ms** | 204 to 207 (350) + 170 (312) | 320 (323) + 315 | 19.1 MB |
| 21 (Advanced 11) | 0.1.0 | 0.12 to 0.13 + 5.0 ms | 0.16 + 6.7 to 6.9 ms | 9.7 to 10.0 ms | (402) | 501 | 28.1 to 29.7 MB |
| | 0.2.0, Reflections off | 0.13 to 0.15 + 4.0 to 4.1 ms | 0.18 + 5.4 to 5.6 ms | 6.7 to 7.3 ms | 207 to 213 (403) | 365 (368) | 19.1 MB |
| | 0.2.0, Reflections on | 0.17 to 0.18 + 6.8 to 7.1 ms | 0.23 to 0.26 + 9.5 to 10.3 ms | **12.2 to 12.5 ms** | 193 to 201 (403) + 145 (298) | 365 (368) + 289 (291) | 19.1 MB |

(For 0.1.0 only the largest count of a stretch was recorded, so its draw columns are in brackets. In play on
stage 89 the ball falls off after a few seconds with nobody steering, so the median is of little use there.)

What follows from it:

* **With Reflections on, the heaviest mirror stages fit a 60 Hz frame at this throttle** with 4 ms and more
  to spare (checked on stages 89, 76 and 21: p99 of update + render at most 16.6 ms; measured 9.4 to 12.5 ms),
  and are about as fast as 0.1.0 was without any mirror. The mirror costs 3.9 to 5.8 ms at p99 on them.
* **Without Reflections** the same stages take 5.1 to 7.3 ms (checked against the same 16.6 ms).
* **The mirror costs no wasm memory**: 19.1 MB with and without it (checked on the three stages against
  0.1.0's figure for the stage plus 2 MB, the plan's budget: 28.1, 24.2 and 30.1 MB).
* Stage 21 is now the slowest of the three: its mirror is a strip of the floor, but most of the night
  backdrop can be seen in it.
* Not measured: a phone. The throttle slows the page's main thread only; a phone's GPU also has to fill the
  second, half-size target, which nothing here shows. A player whose device is too slow turns the option
  off (Options -> Reflections).

### What the browser check sees of the new drawing

`mbPerf.counts()` gives, for every frame of play, the draw calls of the picture and of the mirror, binds,
uploaded bytes, the particles' instances and quads, and whether the ball's shadow was drawn
(`mb_app::perf::FrameSample`). The check asserts, on WebGL2 and on WebGPU:

* floor 1 of Course Play, before the goal: instances or quads in a frame (the landing's sparks and star;
  seen: up to 16 instances and 19 quads);
* from the goal on: instances (the confetti; seen: up to 168, with up to 148 quads);
* the shadow drawn in every frame from GO to the goal (seen: 483 of 483);
* no mirror draw in any frame of floor 1, which has no mirror floor;
* stage 89: mirror draws in the fly-in with Reflections on (seen: 298 to 307 a frame beside 344 of the
  picture) and exactly none with it off, after a reload;
* stage 92 (the waved floor), a night stage (21) and a storm stage (81) load and run;
* no error in the console and no warning in the browser's log from the graphics driver, in any of it and
  in the timed runs.

**The fences on WebGL2.** `AutoFinish` makes the engine wait for the driver after a submission instead of
polling a sync object later. What was looked for in Chrome: any entry of the browser's log from its
rendering side or naming WebGL, a fence, a sync object, a finish or a lost context (none in three runs, in
any section of the check); frame pacing on WebGL2 at 6x throttle (Expert 50: 1 to 4 long frames of 7200,
p99 4.5 ms at a display frame of 4.2 ms); and frame pacing on WebGPU, which the setting does not touch (0 long
frames, p99 4.5 to 4.6 ms). **Not verified:** Firefox, Safari and Android WebView, where a blocking finish
may cost differently or be reported differently; nothing but Chrome 154 on Windows (ANGLE on Direct3D 11)
was run.

One notice is let pass: Chrome on Windows writes "The powerPreference option is currently ignored" on every
WebGPU start, because the page asks for the low-power adapter (`mb-web`, `power_preference`), which other
platforms honour.

### Where the frame time goes, and what is left to gain

On WebGL2 a frame of play is almost all render calls: update 0.16 ms against render 5.1 ms at 6x CPU
throttle for the 275 draws of Beginner 1 in 0.1.0 (0.2.0: 0.11 against 3.2 ms for 183 draws; the same frame
on WebGPU: 0.9 ms). The renderer already uploads only what changed (one buffer with
dynamic offsets for all objects), sorts opaque draws by pipeline and material, skips redundant binds, culls
by bounding sphere, compiles each pipeline with only the texture fetches its material uses, renders at most
two device pixels per CSS pixel and lowers the resolution by itself when frames stay slow. A still menu or a
pause screen is not drawn again at all. Not done, because the gain could not be measured without a phone:

* **Fewer draws still.** Since 0.2.0 the meshes of one model that share a pipeline and a material are one
  draw (`docs/GMA.md`). Meshes of different models that never move and share a material could be merged as
  well; the draw count would fall towards the number of materials, and on a mirror stage twice.
* **One render pass instead of two.** The backdrop and the world are separate passes so the depth buffer can
  be cleared between them; on a tile-based phone GPU the second pass has to load the colour buffer back.
  Splitting the depth range between the two layers in one pass would avoid that.
* **Compressed textures.** Textures are uploaded as RGBA8 with mip levels (up to 16.6 MB for a stage). The
  disc's own DXT1 (`CMPR`) blocks could go to the GPU as they are where the device supports S3TC, and ETC2
  would need a transcoder.

## Checking it

```
deno run -A scripts/web-e2e.ts [--lan]
```

serves `web/dist` with `python -m http.server`, starts the installed Chrome with a throw-away profile and
drives it over the DevTools protocol, setting the real disc image on the page's real file input. It needs the
bundle, Chrome, Python and the image (`MB_DISC`, or `Super Monkey Ball*.rvz` next to the repository) and says
what is missing otherwise. 93 checks (96 with `--lan`; 83 where Chrome gives the page no WebGPU), about
eight minutes: wrong, empty, random and truncated files; import and
storage contents; play floor 1 into floor 2; in-place retry; audio; wasm memory against 0.1.0; the page hidden
and shown again; a save written by 0.1.0 (read, Reflections on, written back whole); the new drawing on
WebGL2 and WebGPU (sparks, confetti, the shadow, stage 92, a night and a storm stage, the mirror's draws
with Reflections on and none with it off, the Reflections row by touch and across a reload, no graphics
error); frame times at 6x CPU throttle against 0.1.0 and, with Reflections on and off, against a 60 Hz
frame, and the wasm memory of the mirror stages (see "Measurements"); closing
the tab mid-play and mid-import; touch-only menus at 412x915; the back gesture; idle cost; tilt with an
emulated sensor; remove data; cancel; play without storage; the picked file deleted mid-session; a bare web
view (the private file system, `localStorage`, service worker, WebGPU, fullscreen, wake lock, device
orientation, gamepads and streaming compilation taken away before the page loads); browsers that cannot run
the game (no workers, no WebGL2, an engine that rejects the module, no WebAssembly); the bundle served from a
sub-path; the network address (with `--lan`); start with the server
stopped (service worker).
