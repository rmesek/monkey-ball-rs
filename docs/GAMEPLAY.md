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

### Shadow and particles (`play_scene.rs`, `particles.rs`)
Display only, as in the reference (`World.ts` `drawProjectedShadow`, `drawConfetti`, `drawEffects`, fed by
`game.ts` `getConfettiRenderState` / `getEffectRenderState`). Everything they are drawn with is in the shared
files every stage already loads (`common.gma` / `.tpl`, `common_p.lz` / `common.lz`): no file was added to
the lists and the pack is unchanged. A missing model or texture costs that one decoration.

| What | Drawn as | From |
|---|---|---|
| Ball shadow | a term in the stage shader (`Renderer::set_shadow`), no draw call: projected along the stage's light onto the models of the anim groups (not goals, bumpers, bananas), masked by the first texture of `circle_white`; off while the ball is invisible | the blended ball of `PlayScene::apply` |
| Confetti | five instance batches, `PAPER_PIECE_DEEPGREEN / GREEN / ORANGE / RED / YELLOW`, with the opaque geometry, lit and tilting with the stage | `runtime.confetti` |
| Spark (`Coli`) | a streak: one white additive quad built in view space from the spark to where it was up to two 33.4 ms samples ago, 9 px wide at 1200 px of height (27 px close to the camera), head in the spark's colour, tail darkened to red | `runtime.effects` |
| Star (`ColiStar`) | the `CRASH_STAR` model (two meshes) spinning, plus a streak at 0.6 of its scale | |
| Ground glow | `circle_white2` (else `circle_white`), additive, under every spark and star less than 0.5 above the ground, 0.2 towards the camera | |
| Flash (`ColiFlash`) | the same disc at 2.5 times the effect's scale on the surface that was hit, additive with a depth bias | |
| Sparkle (`Levitate`) | a white additive billboard of half side `scale`: four quads, one per quarter of the `CROSS_LIGHT` texture | |

Particles carry their own previous position, so `PlayScene::draw_particles` blends them straight from the
simulation with the frame's tick fraction (positions; angles of stars and confetti the short way round) and
queues them for every frame like sprites. A pause queues the same ones again: the picture stands still and
nothing is uploaded. Streak tails are sampled on a clock that counts the ticks in which the simulation moved
its particles, so they freeze with a pause and with the time-over sequence (where the simulation leaves the
particles alone and they are shown unblended). A retry clears the tails.

Cost: one draw per mesh of a batch that has instances and one per kind of quad, so at most 11 draws however
many particles there are (5 confetti, 2 star, glow, flash, streaks, sparkles); the budget is 12. Seen on the
first floors of Beginner and Advanced: 6 at the goal. Buffers are created at load for 928 instances and 704
quads (512 streaks, 48 sparkles); a larger burst loses the rest. The 0.1.0 texture bytes of a stage grow by
65,532 (the star, disc, mask and sparkle textures). Nothing is allocated or created on the GPU per frame; the
HUD's textures are drawn once without size in the first three frames of a stage so that the `GOAL` banner
creates nothing when it first shows.

Different from the reference, on purpose or by necessity:

- **Order.** The reference sorts every particle back to front with the other blended objects. Here a kind is
  one draw, so it has one place: confetti with the opaque geometry (as there), everything else after the
  sorted objects. A spark, glow or sparkle that is behind the ball is therefore hidden where the ball covers
  it instead of showing through its clear half, and particles are drawn over blended stage parts that do
  not write depth even when behind them.
- **Sparkle texture.** The reference loads `assets/particle/beautifulstar.png`, a file that is neither in its
  repository nor on the disc (without it the reference draws no sparkle at all). The port uses the game's own
  `CROSS_LIGHT` texture from the common NaomiLib archive, the model the reference's `CommonNlModelID` still
  names; geometry, size, colour and blending are the reference's.
- **Streak tails** follow simulated time, not wall time (see above); at 60 Hz they are the same.
- **Limits.** 512 streaks and 928 instances a frame; the reference has none (and the engine's pool is 1024).
- Ported as it is: the flash is moved 0.2 along the contact direction, which points into the surface, so
  on a floor seen from above it is mostly hidden by the floor it lies on, there as here.

## Controls
| | Keyboard | Gamepad (desktop: gilrs, web: Gamepad API) | Touch |
|---|---|---|---|
| Tilt | WASD or arrows | left stick | on-screen stick (bottom left) |
| Menu select | arrows / WASD, Enter | stick or d-pad, A | tap a row, drag to scroll |
| Back | Esc | B | Back row |
| Pause | Esc or P | Start | on-screen pause button |
| Action (hold: fly-in at double speed) | Space, Z or Enter | A | hold the play area outside the stick and buttons |
| Fullscreen | F11 or Alt+Enter | | |

Mouse: hover and click rows, wheel scrolls lists. The on-screen stick and pause button are drawn only while
touch is the device in use (any key or button press hides them). Device tilt (browser, Options -> Controls)
steers by holding the phone; see `docs/WEB.md`. While tilt steers there is no on-screen stick: the pause
button stays, always visible, and a second button above it (a crosshair) takes the pose the phone is held in
now as level. The pose is also taken as level when a stage loads and again at GO, when control starts.

The action button is the reference's `isPrimaryActionDown`: `InputState::primary_held` (`Action::Primary` for
Space, Z and the touch, `Action::Confirm` for Enter and the gamepad's A). `PlayState` passes it to
`Sim::step_with_buttons`, and the engine runs the fly-in of a first attempt at twice the speed while more
than 120 intro frames are left (`docs/ENGINE.md`, "Intro fast-forward"): 240 ticks instead of 360 when held
throughout, never on a retry. A key that is still down from choosing the stage or from the pause menu does
not count until it has been let go once.

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

## Input seam
Gameplay reads exactly one thing: `InputState::tilt() -> (x right, y forward)`, a vector in the unit circle.
Its sources, in priority order like the reference's `getStick`: the `TiltX`/`TiltY` axes (device tilt, fed by
the browser target through `tilt::TiltSteering`), the `MoveX`/`MoveY` axes (gamepad or touch stick), then
keys (unramped, as in the reference; the world tilt itself eases). `PlayState` applies the stick curve and
`quantize_input`. A new input device only has to write actions and axes into `InputState` from its driver
(`app.rs` on the desktop, the `Platform` hooks of `shell.rs` in the browser); nothing in the game states
changes. The pointer API (`set_pointer`, `set_pointer_down`, `add_scroll`) is documented at the top of
`input.rs`; `ui::TouchControls` feeds it for touches.

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
- `cargo test -p mb-app`: `tests/gameplay.rs` drives the real states on an offscreen renderer (goal advances
  the course and saves, fall-out retries until game over and continue, pause / retry / quit in practice,
  pointer and stick input, every course stage loads and renders); `tests/pack.rs` does the same over an
  imported game pack behind a deferred source, the way the browser reads; `tests/pause.rs`, `tests/rules.rs`,
  `tests/kill.rs` (the real program killed mid-run). They skip without a disc or a GPU adapter.
- `tests/gameplay.rs` and `tests/pack.rs` also play floor 1 and compare every frame's particle instances and
  quads with what the simulation holds (streaks, landing burst, confetti, sparkles, the shadow on exactly
  while the ball is visible, a pause that changes nothing), and `gameplay.rs` holds the action button through
  the fly-in in every way it can be held.
- `tests/budgets.rs`: the regression budgets of `docs/PARITY_PLAN.md`. Draw calls outside the particles of
  every course stage at tick 60 (fly-in) and 420 (play) equal a table recorded from 0.1.0 (its model draws
  were compared with a build of the `v0.1.0` tag, all 109 stages identical); particles add at most 12; the
  renderer's `GpuCounters`, texture and object counts do not change from the third frame of a stage to the
  end of its goal sequence; stage 109 uploads at most 1 MB more texture bytes than 0.1.0's 16,617,412.
  `tests/pack.rs` holds the pack at 382 files and 144,084,942 bytes. `MB_BUDGET_PRINT=1` prints the tables.
- `cargo test -p mb-app --test audio`: the autopilot's floor 1 with a recording audio backend and an offline
  render of its sound.

The tests run on developer support that the product does not contain: the windowless driver (`headless.rs`),
the waypoint autopilot (`autopilot.rs`) and a PNG writer (`image.rs`) are compiled only with the `dev` cargo
feature, which `cargo test` switches on by itself. A `dev` build of the program also has two options:

- `cargo run --release -p mb-app --features dev -- <disc> --autoplay <stage> [--screenshot-dir <dir>]`:
  windowless; walks the real menu with key presses, plays the stage with the autopilot and, when asked,
  writes PNGs of the menu, fly-in, READY, GO, play, goal and result to a directory of your choice. Exit code
  1 if the goal is not reached.
- `... -- <disc> [stage] --exit-after <s>` prints frame pacing and simulation ticks against the 60 Hz target,
  and what the audio device played (stream, callbacks, peak / RMS, late callbacks).

## Not ported from the reference
- Mirror floors, the bonus-wave vertex animation, per-background special effects (`Background.ts`), specular
  suppression on the inner ball shells.
- Result replays (the reference replays the fall-out and the goal).
- `JUMP TO FLOOR n` and the warp bonus line of the goal banner (`PlayRequest` carries no jump preview), the
  additive white flash of the `GOAL` and `GO` banners (sprites only multiply).
- Not in the reference either (original game only): the monkey inside the ball, its voice lines and the
  sounds listed under "Known gaps" in `docs/AUDIO.md`.
- Multiplayer, netplay, lobby, SMB2 content (out of scope by project rule).

Differences on purpose: lives are shown (the reference hides its counter), the HUD is edge-anchored instead
of letterboxed, tall targets get a wider vertical field of view, bumpers show the simulation's squash.
