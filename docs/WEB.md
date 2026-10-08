# Browser build

The game runs in a browser as WebAssembly (`crates/mb-web`) behind a small static page (`web/`). The page
contains the engine only. The player supplies their own disc image through the file picker (or by dropping it
on the page); it is read on the device and never uploaded. The page makes no network requests except for its
own bundle files, and its Content-Security-Policy forbids anything else.

The output is a **static site**: plain files, relative paths only, no server-side component, no dependency on
a service worker, WebGL2 as a first-class backend. It can be hosted by any static file server, installed as a
PWA, placed inside another application's web view, or opened as a single `.html` file.

## Build

Needs the wasm target and the wasm-bindgen CLI at exactly the version of the `wasm-bindgen` crate in
`Cargo.lock` (pinned in `crates/mb-web/Cargo.toml`, currently 0.2.129):

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
```

One command builds the site into `web/dist/` (git-ignored):

```
scripts\build-web.ps1 [-Single] [-Dev]       # Windows PowerShell
scripts/build-web.sh [--single] [--dev]      # bash
```

It checks the CLI version, runs `cargo build -p mb-web --target wasm32-unknown-unknown --profile web`,
`wasm-bindgen --target no-modules`, and `cargo run -p xtask -- bundle-web`, which assembles the directory:

| File | What |
|---|---|
| `index.html` | the page (stamped with a version hash of the bundle) |
| `main.js`, `worker.js` | the page's script and the import worker (classic scripts, no modules) |
| `pkg/mb_web.js`, `pkg/mb_web_bg.wasm` | the engine: 5.0 MB wasm (1.56 MB gzipped), 0.19 MB glue |
| `sw.js`, `manifest.webmanifest`, `icon*.{svg,png}` | offline start and installation (optional at run time) |
| `monkey-ball.html` | with `-Single` / `--single`: everything in one file, 2.3 MB (1.6 MB gzipped) |

`-Dev` / `--dev` builds unoptimised. The `web` profile (workspace `Cargo.toml`) is `release` plus fat LTO, one
codegen unit and `panic = "abort"`. `wasm-opt` is not used (not required; not installed on the build machine).

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

### The single file

`web/dist/monkey-ball.html` (built with `-Single`) has the scripts inlined as classic scripts, the wasm
embedded as gzipped base64 (unpacked with `DecompressionStream`), the import worker started from a Blob, and the
icon inlined. No service worker, no manifest. Its policy lists the two inline scripts by SHA-256 hash and has
`connect-src 'none'`. Copy the one file anywhere and open it.

What a page opened from disk may do depends on the browser. Measured in Chrome 154 on Windows: it is treated as
a secure context and gets WebGPU, but no private file system, so the session plays from the picked file and
saves go to `localStorage`. Not verified: Android, where opening a local `.html` from a file manager gives the
browser a `content://` address (whether Blob workers and `FileReaderSync` work there is untested), and iOS.
If the single file does not work somewhere, host `web/dist/` instead.

### Page parameters

`?gl=1` forces WebGL2 where WebGPU is available; `?scale=0.5` renders at that fraction of the device
resolution (0.25 to 1); `?autoscale=0` turns the automatic resolution fallback off; `?fps=1` shows the frame
timing overlay; `?nostore=1` skips on-device storage (plays from the picked file); `?tilt=1` offers tilt
control on a device without a touch screen. `window.mbStats` holds what happened at start-up (mode, import
time, pack size, save mode), `window.mbPerf` frame timings and load breakdowns (`summary()`, `lastLoad()`,
`status()`, `preparedBytes()`, `wasmMemoryBytes()`, `audio()`).

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
hostile-size inputs under an allocation-counting allocator. `cargo run --release -p xtask -- check-rvz <image>`
compares the readers with the `nod` crate over a whole image (identical on the tested file: 1,459,978,240 disc
bytes, 897 files, `main.dol`); `nod` is a test and tool dependency only.

## Storage model

### What is kept, and where

| | Browser | Desktop |
|---|---|---|
| Game data | one **pack** file in the page's Origin Private File System: `monkey-ball/game.pack` plus the marker `monkey-ball/game.json` | nothing is copied: the image is read in place; only its path is remembered (`last_disc`) |
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
| Shared models (ball, bananas, goal, clock digits) | 1.2 MB |

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
* **Space.** About 145 MB; `navigator.storage.estimate()` is checked first.
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
  error dialog also offer the removal. On desktop, Options shows the save folder and the remembered image
  path, with "Forget game image" and "Erase save data".

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
and cuts them off after every one, checking the result is never accepted as a pack; `tests/kill.rs` kills the
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
  history entry is kept for that); at the title it leaves the page as usual.
* Options: fullscreen (with an attempt to lock landscape), controls by device tilt (calibrated to the pose
  the device is held in when a stage starts; iOS asks for permission at the next tap), remove game data,
  erase save data.
* The screen is kept awake while the page is visible (Screen Wake Lock, re-acquired on return).
* Portrait and landscape both work; no rotate hint is shown.
* A lost graphics context or a fatal error shows a dialog with one button that restarts the page, which is
  quick now (the title menu is back in a fraction of a second).

## Browser requirements

WebAssembly; WebGPU, or WebGL2 (automatic fallback, and the only backend on plain http and in most web views);
`Worker` and `FileReaderSync`. On-device storage additionally needs the Origin Private File System with
`createSyncAccessHandle` (Chrome 102+, Firefox 111+, Safari 15.2+); the single file needs
`DecompressionStream` (Chrome 80+, Firefox 113+, Safari 16.4+).

## Measurements

Chrome 154, Windows 11, RTX 4080, 165 Hz display, the tested RVZ; `scripts/web-e2e.ts` prints most of these.

| | Before (image held in the tab) | Now |
|---|---|---|
| JS heap with the game running | 509 MB | 1.7 MB |
| wasm memory while playing | (plus the above) | 32 MB (Beginner), 40 MB (Expert 50) |
| Game files held in memory between loads | all | 0 bytes (largest single stage fetch: 10.6 MB, released after the load) |
| First run: pick to menu | read 475 MB every visit | 1.3 s import (copy 144 MB, verify), once |
| Later runs: page load to first frame | same as first | 0.1 s after the engine is compiled; no picker |
| Whole browser during import (12 processes, private bytes) | | +132 MB at the peak, back to baseline after |
| Stage load, Beginner 1 / Expert 50, WebGL2 | 176 ms / 432 ms at 6x CPU throttle | 82 ms / 28 ms unthrottled; Expert 50 at 6x: 137 ms |
| Frames, 30 s of steering, Expert 50, WebGL2, 6x throttle | 2 long of 4963 | 2 long of 4917, p99 6.6 ms |
| Frames, 30 s, Beginner 2, WebGPU | 0 of 5783, p99 6.6 ms | 1 of 4997, p99 6.5 ms |
| A still menu | | 1.9 % of one core (no redraw without change) |
| Play, 165 fps | | 27 % of one core on the main thread |

Not measured: any real phone, iOS, an Android web view, Firefox, Safari.

## Checking it

```
deno run -A scripts/web-e2e.ts [--lan]
```

serves `web/dist` with `python -m http.server`, starts the installed Chrome with a throw-away profile and
drives it over the DevTools protocol, setting the real disc image on the page's real file input. It needs the
bundle, Chrome, Python and the image (`MB_DISC`, or `Super Monkey Ball*.rvz` next to the repository) and says
what is missing otherwise. 45 checks (48 with `--lan`): wrong, empty, random and truncated files; import and
storage contents; play floor 1 into floor 2; in-place retry; audio; closing the tab mid-play and mid-import;
touch-only menus at 412x915; the back gesture; idle cost; tilt with an emulated sensor; remove data; cancel;
play without storage; the network address (with `--lan`); the single file opened from disk; start with the
server stopped (service worker).
