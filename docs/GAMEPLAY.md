# Gameplay, HUD and menus (`mb-app`)

What the player sees on top of the engine, renderer and game rules. Reference: WebMonkeyBall `game.ts`,
`hud.ts`, `input.ts`, `noclip/SuperMonkeyBall/World.ts` + `AnimGroup.ts`.

## States and flow
`mb_app::initial_game(storage)` returns the root state, `menu::Menu`. Everything runs on one `GameStack`.

```
Menu (title -> Course Play | Practice | Options | Quit)
 |  Course Play: pick a course  -> CourseRun::new(d)
 |  Practice:    pick a course, pick a floor -> CourseRun::practice(d, i)
 v
 push PlayState(PlayRequest)            one stage: Intro -> Playing -> GoalSequence | FallOut | TimeOver
 pop  <- PlayResult in request.report   Finished(StageOutcome) or Quit (pause menu)
 |
 CourseRun::apply(outcome, progress) -> save profile ->
     Next / Retry      push the next PlayState at once (the HUD's floor intro is the transition)
     GameOver          "GAME OVER": Continue (CourseRun::continue_run) or Quit
     CourseComplete    "COURSE CLEAR": score, new best, unlocked course
     PracticeClear     "GOAL": time, new best; Retry / Next Stage / Stage Select
```

- `menu.rs` owns the `Profile` (progress + settings) and writes it through the injected `mb_game::Storage`
  after every stage and every option change. Extra and Master courses and unreached practice floors show as
  `LOCKED` unless the `Unlock all stages` option is on. Options only lists settings that act: that one, the
  stick curve (the reference's input falloff) and the three volumes (below).
- `play.rs` runs `mb_engine::Sim` in fixed 60 Hz ticks from an accumulator (`ticks_for_frame`): frame times
  within 12 % of one or two ticks are snapped, at most 5 ticks run per frame, a longer stall is dropped. The
  simulation is identical at any display rate. What is drawn is a blend of the last two ticks by the
  accumulator's fraction of a tick (the reference's `getInterpolationAlpha` and `get*RenderState(alpha)`):
  `PlayScene::sync` records each tick's pose, `PlayScene::apply` blends ball, stage tilt, anim groups,
  bananas, bumpers, jamabars, goal bags and tapes, and `CameraPose::blend` the camera, so motion is even at
  90/120/144 Hz and under frame-time jitter. The blend is display only (the simulation and `sim_hash` never
  see it) and is skipped across a restart, a teleport of the ball or a camera cut. On a display that runs a
  whole number of ticks per frame the accumulator eases to 0.85 of a tick, so the picture stays within a
  sixth of a tick of the newest state.
  The game's own trig tables are installed from `main.dol` first (`Content::install_trig_tables`).
- Pause freezes the simulation in any phase and offers Continue, Retry, Quit. Retry restarts in place in
  practice; in course play it gives the attempt up as a fall-out (one life). Retry is disabled once the
  attempt is decided (goal sequence, fall-out, time over: the result on screen stands) and on bonus stages
  in course play (a bonus stage is one attempt; a fall-out there advances the course, so a retry would skip
  it): `play::retry_available`. Quit reports `PlayResult::Quit`; in course play it first asks
  `QUIT? / THIS RUN WILL BE LOST` with `KEEP PLAYING` selected, and Esc / Back returns to the pause menu.
- `GameStack::update` updates the state that is on top after a push or pop in the same frame, with `dt = 0`
  and the frame's input edges removed, so there is never a blank frame and one key press acts once.

## Scene (`play_scene.rs`)
Built once per stage: the stage scene (`stage_scene.rs`), the ball shells, goal bag halves, one goal tape mesh
and five clock digits per goal. `PlayScene::sync` poses everything from the simulation each frame:
anim groups from `runtime.anim_groups`, bumpers (spin and squash), jamabars, bananas (hold / fly-away, hidden
when collected), goal bags (openness), tapes (28 vertices rewritten only when the tape moves), goal clocks,
keyframed background objects, the ball (position, orientation, radius), and the stage tilt: every stage
object is rotated about the ball by `world.xrot/zrot * 0.6` exactly like `getTiltedViewMatrix`; the backdrop is
not. Camera: `T(eye) * Ry * Rx * Rz` from `sim.camera`, 60 degrees vertical; on targets narrower than 4:3 the
horizontal view of 4:3 is kept instead (portrait phones).

## Controls
| | Keyboard | Gamepad (desktop: gilrs, web: Gamepad API) | Touch |
|---|---|---|---|
| Tilt | WASD or arrows | left stick | on-screen stick (bottom left) |
| Menu select | arrows / WASD, Enter | stick or d-pad, A | tap a row, drag to scroll |
| Back | Esc | B | Back row |
| Pause | Esc or P | Start | on-screen pause button |
| Fullscreen | F11 or Alt+Enter | | |

Mouse: hover and click rows, wheel scrolls lists. The on-screen stick and pause button are drawn only while
touch is the device in use (any key or button press hides them).

## Options and sound
| Option | Values | Effect |
|---|---|---|
| Unlock all stages | on / off | every course and practice floor selectable |
| Stick curve | 1.00 .. 2.00 | input falloff power |
| Music volume | off, 10 .. 100 (default 50) | stage music |
| Sound volume | off, 10 .. 100 (default 30) | effects: roll, impacts, bananas, goal, clock, menus |
| Announcer volume | off, 10 .. 100 (default 30) | READY, GO, GOAL, FALL OUT, countdown, ... |

Selecting a row steps its value (volumes by ten, wrapping from 100 to off). Changes are saved with the
profile and apply at once; a category that is off is not played at all. Menus click on move, confirm and
back. In the pause menu the music plays on and the ball's roll is silent; leaving a stage cuts its effects.
What plays when, and how sound reaches the device, is in `docs/AUDIO.md`.

## Input seam (Stage 2 tilt)
Gameplay reads exactly one thing: `InputState::tilt() -> (x right, y forward)`, a vector in the unit circle.
Its sources, in priority order like the reference's `getStick`: the `TiltX`/`TiltY` axes (reserved for the
accelerometer), the `MoveX`/`MoveY` axes (gamepad or touch stick), then keys (unramped, as in the reference;
the world tilt itself eases). `PlayState` applies the stick curve and `quantize_input`. An Android shell only
has to call `input.set_axis(Axis::TiltX / TiltY, ..)`; nothing in the game states changes. The pointer API
(`set_pointer`, `set_pointer_down`, `add_scroll`) is documented at the top of `input.rs`; `ui::TouchControls`
feeds it for touches.

## HUD (`hud.rs`, `text.rs`)
A port of `hud.ts` with the same timings, laid out on its 640 x 480 grid but anchored to the edges and centre
of the safe area (`ui::Layout`), so it works in any aspect including portrait. Everything is decoded at run
time from the user's disc; nothing is embedded:

| Element | Texture |
|---|---|
| Timer digits, centiseconds | `bmp_nml.tpl` 12, 13 |
| Score digits | `bmp_nml.tpl` 5 |
| Labels (`SCORE`, `BANANA(S)`, floor, speed) | `bmp_nml.tpl` 9 |
| Menu text, `HURRY UP!`, bananas left | `bmp_nml.tpl` 7 |
| Banners (`READY`, `GO`, `FALL OUT`, `TIME OVER`, floor intro) | `bmp_com.tpl` 80 |
| Goal score lines, hints | `bmp_nml.tpl` 6 |
| `GOAL` banner, bomb, crack, fragments, difficulty icon | `bmp_nml.tpl` 4, 0, 16, 17..26, 1 |
| Banana, bunch, mph, player icon | `bmp_com.tpl` 12, 85, 88, 3 |

Font cell layouts are the reference's `FONT_PARAMS`. The touch stick uses a disc computed in code.

## Automated verification
- `mb-app <disc> --autoplay <stage> --screenshot-dir <dir>`: headless; walks the real menu with key presses,
  plays the stage with the waypoint autopilot (`autopilot.rs`) and writes PNGs of the menu, fly-in, READY,
  GO, play, goal and result. Exit code 1 if the goal is not reached.
- `cargo run -p mb-app --example screens -- <disc> <dir>`: PNGs of the other screens (course HUD, pause,
  goal score, floor intro, fall-out, game over, time over, portrait with touch controls).
- `cargo test -p mb-app`: `tests/gameplay.rs` drives the real states on an offscreen renderer (goal advances
  the course and saves, fall-out retries until game over and continue, pause / retry / quit in practice,
  pointer and stick input, every course stage loads and renders). Skips without a disc or adapter.
- `mb-app <disc> [stage] --exit-after <s>` prints frame pacing and simulation ticks against the 60 Hz target,
  and what the audio device played (stream, callbacks, peak / RMS, late callbacks). `--mute` opens no device.
- `cargo test -p mb-app --test audio`: the autopilot's floor 1 with a recording audio backend and an offline
  render of its sound.

## Not ported from the reference
- The monkey inside the ball (separate skeletal system in the reference), the projected ball shadow,
  confetti, sparks, streaks and the other `effects.ts` particles, mirror floors, the bonus-wave vertex
  animation, per-background special effects (`Background.ts`), specular suppression on the inner ball shells.
- Result replays (the reference replays the fall-out and the goal), intro skipping.
- `JUMP TO FLOOR n` and the warp bonus line of the goal banner (`PlayRequest` carries no jump preview), the
  additive white flash of the `GOAL` and `GO` banners (sprites only multiply).
- The monkey's voice lines and the sounds listed under "Known gaps" in `docs/AUDIO.md`.
- Multiplayer, netplay, lobby, SMB2 content (out of scope by project rule).

Differences on purpose: lives are shown (the reference hides its counter), the HUD is edge-anchored instead
of letterboxed, tall targets get a wider vertical field of view, bumpers show the simulation's squash.
