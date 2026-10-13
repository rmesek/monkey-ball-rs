# State of the project

What the project is for, what works, what nobody has checked, what is open and what was decided. How the
code is put together is in [ARCHITECTURE.md](ARCHITECTURE.md).

**Version 0.3.0, release candidate.** Not tagged: the owner tries it by hand first (see "Decisions").
Version 0.2.0 was released on 2026-10-13. What each version contains: [CHANGELOG.md](../CHANGELOG.md).

## Goal

Super Monkey Ball 1 in Rust, fully offline, small enough to understand, port and change by hand.

1. **Read the game image on the device and keep what the game needs**, on the desktop, in a browser and
   later in an Android app.
2. **Play the game as it is**: every course stage, the menus, the options; keyboard, stick, touch, and tilt
   where the device has it.
3. **Never leave a damaged state behind**, however the program ends.
4. **Stay that way**: tests and budgets that catch a regression, and no more machinery than that.

Targets: the desktop program and the static web site now; Android later, in a WebView and natively. Nobody
starts a port without the owner asking.

Not in scope, by decision: multiplayer and anything on a network; the reference's replay-file player, map
viewer and instant replays; Super Monkey Ball 2; what only the original game has (the monkey in the ball,
its voices, jingles, menu music, minigames).

## What works

| | Where | Held by |
|---|---|---|
| Disc image: RVZ (none, Zstandard), ISO, GCM; imported once into a pack of 382 files, 144 MB | `mb-assets`, `mb-app/src/{content,import}.rs` | recorded hashes of the reader; `mb-app/tests/pack.rs` |
| Formats: stagedef, TPL, GMA and NaomiLib, ADP music, MusyX sound banks | `mb-stage`, `mb-tpl`, `mb-gma`, `mb-audio` | golden hashes over every file; hostile-input tests |
| The simulation at 60 Hz: physics, collision, stage objects, camera, goal, fall-out, time over, bonus stages, particles | `mb-engine` | `tests/golden.rs`: every stage against hashes recorded where the port was proved equal to the reference |
| Courses, Practice, lives, score, continues, unlocks, settings; saves that survive a kill | `mb-game` | its tests; `mb-desktop/tests/kill.rs` |
| Drawing: stages, backgrounds, ball, materials, particles, shadow, moving backgrounds, mirror floors on 37 stages | `mb-render`, `mb-app` scenes | pixel tests on every backend; state tests |
| Title, course and stage select, options, loading, play, pause, HUD | `mb-app` | `mb-app/tests/{gameplay,pause,rules}.rs` |
| Sound effects, announcer, stage music | `mb-audio`, `mb-app/src/audio.rs` | `mb-app/tests/audio.rs` (signal levels and timing) |
| Keyboard, gamepad, mouse, touch, device tilt | `mb-app/src/{input,ui,tilt}.rs`, the drivers | state tests; the browser test |
| The desktop program | `mb-desktop` | the kill test |
| The static site | `mb-web`, `web/` | `scripts/web-e2e.ts` |

Last run of everything, 2026-10-13, Windows 11, USA disc: `cargo xtask check` passes with 549
tests; `deno run -A scripts/web-e2e.ts` passes its 93 checks in Chrome 154.

## Budgets

Counted, not timed, wherever a count exists. A budget is changed on purpose only: in the same commit as its
cause, with the reason in the message.

| What | Limit | Now | Enforced by |
|---|---|---|---|
| wasm module | 4 127 298 bytes | 4 009 217 bytes | `WASM_BUDGET_BYTES` in `xtask/src/main.rs`: the build fails above it |
| Game pack | 382 files, 144 084 942 bytes | the same | `the_import_copies_exactly_what_the_game_reads` |
| Draw calls per stage, fly-in and play | the recorded table `DRAWS` | 25 517 / 23 343 over the 109 stages | `every_stage_draws_what_is_recorded_outside_the_particles` |
| Draw calls of particles | 12 | at most 11 | `particles_add_at_most_twelve_draws_at_their_peak` |
| Draw calls of a mirror | the recorded table `MIRROR_DRAWS`; none without a mirror | 12 to 94 % on top of the frame | the same tests |
| GPU objects created in a play frame | 0 | 0 | `no_gpu_object_is_created_while_a_stage_is_played` |
| Heap allocations in a simulation tick | 0 | 0 | `stepping_with_particles_allocates_nothing` |
| Texture bytes of the heaviest stage | 17 617 412 | 16.7 MB | `the_heaviest_stage_uploads_no_more_texture_bytes_than_its_budget` |
| Heap while a stage loads and plays | a recorded figure per stage + 2 MB | within it | `mb-app/tests/memory.rs` |
| wasm memory and frame time in the browser | recorded figures (WebGL2, 6x CPU throttle; mirror stages within a 16.6 ms frame) | 19 to 24 MB; 3 to 12 ms | `scripts/web-e2e.ts` |

## Where the game differs from the reference, on purpose

Each is described once, with its reason, in the file named.

| Difference | In |
|---|---|
| A fall-out and a goal are shown once, without the instant replay; the confetti lives for the goal sequence | [ENGINE.md](ENGINE.md), "Not modelled" |
| Mirror floors: stage 92 without the distortion, the planes of stages 103 and 74, an option to switch them off | [GAMEPLAY.md](GAMEPLAY.md), "Mirror floors"; [GMA.md](GMA.md) |
| Particles are batched; streak tails follow simulated time; the pool is capped at 1024 | [GAMEPLAY.md](GAMEPLAY.md), "Shadow and particles"; [ENGINE.md](ENGINE.md) |
| Sunset clouds scroll; `JUMP TO FLOOR n` names the destination; lives are shown; the HUD is anchored to the screen's edges | [GAMEPLAY.md](GAMEPLAY.md) |
| Every sound is the disc's own sample; game over, five continues and a 1UP per 100 bananas exist | [AUDIO.md](AUDIO.md); `mb-game/src/run.rs` |
| Two volume dials, where 80 is the calibrated level | `mb-app/src/menu.rs`, `apply_volumes` |

## Unverified

Each is a statement about where the evidence stops.

* **Devices and browsers.** Only Chrome 154 on Windows 11 and the Windows desktop program were run. Not a
  phone, an Android WebView, iOS, Firefox, Safari, or a real gamepad.
* **Linux and macOS** desktop builds, and both CI workflows, have never run.
* **Android.** The game library compiles for `aarch64-linux-android`; no driver exists and nothing ran.
* **Another architecture.** The simulation ran on x86-64 and wasm32 only.
* **The Europe and Japan discs** are accepted by id and untested.
* **Sound** was verified by measurement (levels, no clipping, the right requests at the right times),
  never by listening. The new default volume is the old calibrated level under another number.
* **How it looks** against the original: no person has compared pictures.
* **Removing the game data from the options screen on the desktop** has no test; the rest of the desktop's
  import (first run, restart without the image, kills, a cut-short pack) has.

## Open items

| Item | State |
|---|---|
| Licence | `Cargo.toml` declares `MIT OR Apache-2.0`; there is no `LICENSE` file, and the reference this code was ported from carries no licence statement. For the owner to settle before publishing more widely. |
| The review findings | [ARCHITECTURE.md](ARCHITECTURE.md), "Review": ten places where the code is harder to change than it should be. None acted on. |
| Desktop import feedback | The import runs before the window opens and reports to the terminal only: a program started by double-click shows nothing for a second or two. |
| The browser's second content path | Where a page may not store data it plays from the picked file ("live" mode). Dropping it would remove a path through page, worker and glue; it is what keeps plain-http addresses and private windows working. Owner's call. |
| Tilt without readings | If the device never delivers sensor readings there is no stick to fall back on until the player switches Options -> Controls back. |
| Phones | Nothing ran on a real phone. The file chooser asks for `application/octet-stream`; a file manager that reports the image under another type greys it out, hence the "show all files" button. |
| Continue rule | "Continue resets the score and keeps the bananas", game over, five continues and the 1UP per 100 bananas come from memory of the original, not from a source. |
| Largest music track | About 54 MB of float samples in the browser (computed, not measured). |
| Pack version | `mb_assets::gamepack::VERSION` must be raised when the list of copied files changes; `tests/pack.rs` fails when the count or the bytes change, which is the reminder. |
| CI workflows | `ci.yml` (manual) runs the gates; `release.yml` attaches executables and the site archive when a release is published. Neither has ever run. |
| `rust-version` | 1.88 is what clippy's MSRV lint accepts; only the current stable toolchain has built the code. |
| 32-bit overflow fixes in the parsers | Made by inspection; only wasm32 exercises them, on valid data. |

## Decisions

- **Release gate.** No tag and no push before the owner has tried the release candidate by hand and
  confirmed.
- **Standalone.** The project no longer depends on the reference. What the port does is held by this
  repository's own recorded hashes and tests; a change of behaviour is a decision recorded in a commit.
- **One crate per platform** on a game library without a window; the next platform is another crate.
- **Game data is imported once on every platform** into one pack: a file on the desktop (and later on
  Android), the private file system in the browser. The image is not needed afterwards.
- **One command per target:** `cargo desktop`, `cargo web`. No shell scripts, nothing to install but Rust.
- **The site is plain files** that a server no more capable than `python -m http.server` can host: no
  custom headers, so no threads; relative paths; WebGL2 as a first-class backend.
- **Graphics:** `wgpu`, one renderer for every target, nothing beyond WebGL2's limits.
- **Saves:** two checksummed slots per value, written when they happen, never at exit.
- **Budgets are recorded constants** enforced by tests or by the build.
- **Effects never write into the simulation.** They use their own random stream and are outside `Sim::hash`.
- **Developer support stays out of the product:** the windowless driver and the autopilot are behind the
  `dev` feature.
- **Strictly offline.** No networking crates; the page requests nothing but its own files.
- **Scope** is closed (see "Goal"). The instant replays of the reference were dropped on 2026-10-13.
- **Rust edition 2024.**
