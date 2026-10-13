# Gameplay, HUD and menus (`mb-app`)

What the player sees on top of the engine, renderer and game rules. Reference: WebMonkeyBall `game.ts`,
`hud.ts`, `input.ts`, `noclip/SuperMonkeyBall/World.ts` + `AnimGroup.ts`.

## States and flow
`mb_app::initial_game_with_audio(storage, audio)` returns the root state, `menu::Menu`. Everything runs on one `GameStack`.

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
  stick curve (the reference's input falloff), the three volumes and the reflections (below).
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
65,532 (the star, disc, mask and sparkle textures). Nothing is allocated or created on the GPU per frame.
What the HUD's sprites need (a bind group per texture, room for 1024 sprites) is made when the textures are
loaded and again when a stage is entered (`UiAssets::warm`, through `Renderer::warm_sprite_texture`:
unloading a stage's textures drops the sprites' bind groups), so the `GOAL` banner creates nothing when it
first shows and the count of GPU objects holds from the first frame of a stage.

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
- **The shadow and discarded fragments.** The reference draws the receivers a second time with its shadow
  program, over the finished frame. Here the shadow is a term at the end of the stage shader, after the
  alpha test, so a fragment the receiver's material discards (alpha zero: a hole in a texture) takes no
  shadow. That is what keeps the shadow at no draw call.

### The shadow, checked
The shadow is cast along the stage's light, which for most backgrounds comes from above, behind and to the
left of a camera that looks down -Z: the shadow then lies ahead of the ball and to its right, 0.5 from the
point the ball stands on, and a low camera behind the ball sees little more than its right-hand rim. That is
why it is plain at GO on some stages and hard to make out while rolling away from the camera on others; it
is not a fault. `tests/gameplay.rs` (`the_shadow_darkens_the_floor_beside_the_ball_from_every_side`)
renders the same frame with and without the shadow on stages 1 and 2 from 16 cameras around the ball: the
floor where the light throws the shadow is at least 15 % darker in every view that sees the spot (12 of 16;
in the others the ball is in front of it), a spot away from the ball does not change, and through the game's
own camera about 3,600 (stage 1) and 4,100 (stage 2) pixels of 640 x 360 are darker.

### Backgrounds and the bonus wave (`backdrop.rs`, `stage_scene.rs`)
Ports of `BgNight`, `BgStorm` and `BgSunset` (`Background.ts`) and of `World.bonusWaveModel`. All of it is
display only, runs on the stage timer (`runtime.timer_frames`, the reference's `getAnimTimeFrames`) blended
between ticks, and uses files a stage loads anyway (the background archive, the stage's NaomiLib archive):
the file lists and the pack are unchanged.

| What | How | Cost |
|---|---|---|
| Night: dancers in the diner windows | a window of a shown background object names one of seven lists of frame models (`NIG_MADO_A_PT00` ...: 14, 11, 18, 18, 18, 16, 15 frames) and shows frame `floor(t / 2) % frames` at its own position and rotation | an object per frame, made at load and hidden; a frame change flips two visibility flags. One draw per window in view (23 to 33 windows a stage); 2.2 MB of textures |
| Storm: fires in the braziers | 32 frame models (`STM_FIRE00` ...), frame `(floor(t) + 4 * offset) % 32` (the offsets on the disc are the letters `A` to `H`), at `T(pos) * Ry(camera)` so the flame faces the camera | as above: 8 fires, one draw each in view; 2.1 MB of textures |
| Sunset: drifting ground haze and clouds | `SUN_GROUND` and `SUN_CLOUD_*` get a texture offset (`Renderer::set_tex_offset`) that moves at 0.9 to 1.1 of one repeat per 660 frames in a heading of its own; below 660 frames on the clock each takes a new heading at twice the speed, and the velocity closes 5 % of the gap per frame | two objects re-uploaded per frame |
| Stage 92: the rippling floor | `SHAPE_STAGE134` of the stage's NaomiLib archive, which no anim group names, drawn in stage space with both faces and `WaveParams::BONUS_WAVE`; `Renderer::set_wave_time` gets the stage timer the collision ripple uses | one object, one draw; no vertex is rewritten |

Stages 28 and 48 keep their diners among the foreground objects, and a few diners and two braziers per
stage lack the visible flag: the reference plays the flipbooks of created background objects only, so those
windows and fires stay dark there and here. The clouds stand still while the stage timer does (the fly-in
of a first attempt until 120 frames are left, the time-over sequence, a pause).

The ball rests on the drawn wave: over 900 ticks of rolling about stage 92 the distance from the ball to
the surface the vertex shader draws under it is within 0.03 on 320 ticks and never below -0.021
(`the_bonus_wave_floor_ripples_under_the_ball`). As in the reference the wave floor takes no shadow (the
shadow falls on anim-group models) and the stage's own `STAGE092` model is drawn with it; that model has a
flat quad at y = -0.35, which the troughs of the wave dip under within 4.9 of the centre.

Different from the reference, on purpose:

- **Random numbers.** `BgSunset` draws offsets, speeds and headings from `Math.random`. Here they come from
  a generator of their own (`mb_engine::rng::DeterministicRng`, seeded with the stage id), in the
  reference's order of draws: a stage looks the same on every load and retry, and the simulation's
  generators are not touched.
- **The scroll is visible.** The reference hands the offset to layers flagged for UV scrolling, and no layer
  on the SMB1 disc has the flag (`docs/GMA.md`), so its clouds stand still on this data. Here
  `Renderer::set_tex_matrix_every_layer` applies it to every diffuse and alpha layer of the cloud objects.
- **The hurry-up clock.** `MkbTime` assumes a limit of 60 seconds for every stage (with a note to fix it), so
  on a 30 second stage the reference's clouds never hurry. Here the stage's own limit is used; on a 60
  second stage the two agree to the frame (the scene's clock is one frame behind the HUD's in both).
- **Updates per tick.** The reference advances the clouds by the frame's share of a tick; here they advance
  once per tick and the offset is blended between ticks, like every other pose.
- **Window order.** The reference draws a window with `RenderSort.None`, with the opaque geometry. The
  renderer sorts every mesh the file marks as blended, so the windows are given a sort offset that puts
  them first in that order, right after the opaque meshes.

### Mirror floors (`mirror.rs`, `stage_scene.rs`, `play_scene.rs`)
37 of the 109 course stages have a floor that reflects: `World.getMirrorMode`, `getMirrorPlaneMatrix`,
`prepareToRenderMirrors` and the capture of `Render.ts`. `mb-app` says which stage has a mirror, where
its plane is, which objects show the reflection and which are left out of it; the passes are
`mb-render`'s (`docs/GMA.md`, "Mirror floors"). Everything is decided from the parsed stage
(`mirror::stage_mirror`, no GPU) and set while the stage is built (`StageScene::build`), from files the
stage loads anyway: the mirror models are in the stage's own archive, so the file lists and the pack are
unchanged.

**Which stages** is the reference's rule (`mirror::mirror_kind`): the bonus wave (stage 92) when its
floor was found, and every stage whose stagedef names mirror models (or that is stage 102 or 103). On
the USA disc: 91 to 95, 21, 48, 62, 71, 72, 74 to 78, 82, 83, 85, 86, 89, 101 to 106, 108 to 113, 121,
123, 125, 128, 129. `tests/mirrors.rs` holds the list with what each has.

**Where the plane is** is not the reference's rule but read off the mirror models (`mirror::plan_mirror`):

* A mirror model lies in a plane when all its vertices are within 0.02 of it, one in a hundred aside
  (stage 48's has four of 1088 vertices half a unit down, in four slanted triangles of 1034). The plane
  is at the middle height of the vertices, facing the way their normals do. A model that is not flat
  would be no mirror; none on the disc is.
* Mirror models share a plane when they lie in it at 128 moments of the stage's animation loop, whatever
  anim groups carry them. On the disc the models of every stage do: also the 5, 16, 24 and 21 of stages
  78, 106, 121 and 123, whose groups only slide along the plane. Played with the engine moving the
  groups, no mirror model is ever more than 0.013 from the plane the frame mirrors in
  (`mirror_floors_lie_in_the_plane_of_their_mirror_models`).
* The plane with the most models is the mirror's. Planes parallel to it are further levels: stage 110 has
  a floor at y = -8.99 and one at y = -42.99, and the frame mirrors in the highest one the camera is
  above, showing only that floor's model (`mirror::level_for_eye`, `PlayScene::face_camera`; the
  reference's `cameraPos[1] >= -9.0`). A plane that leans against the mirror's would be dropped with
  its models; there is none on the disc.
* The plane stands still unless its group takes it along. Only stage 103's does: there it is recorded
  per tick with the pose (`PlayScene::sync`) and blended for the frame by the same blend as the mirror
  model that lies in it (`PlayScene::apply`, `Renderer::set_mirror_plane`). The stage tilt is the
  renderer's.

**What shows the reflection.** The mirror models of the anim groups (`AnimGroup.mirrorModels`) and of
the stage's own list, each an object of its group like the group's other models (posed, tilted and
blended between ticks with it), drawn with the reflection only (`Renderer::set_mirror_surface_only`):
the reference issues them with its mirror program and never with their own material, and without a
mirror not at all. Their textures are therefore not uploaded. The reflection's opacity is the model's
vertex alpha (22 to 255 of 255: faint on the checkered floors, a full mirror on the logo of stage 103).
Such a surface writes no depth. On stage 92 the floor of the wave (`SHAPE_STAGE134`) is drawn as ever
and shows the reflection over its own picture at 96 / 255 (`WAVY_MIRROR_ALPHA`), in the plane y = 0.02.
Eight of the nine models stage 101 names and one of the two of stage 74 are in no archive; the others
are the mirror.

**In the reflection**, as the reference's capture: the ball, goals with bags, tapes and clocks, bananas,
bumpers, jamabars, the background and foreground objects, confetti, and the models of the anim groups
that have flag bit 2 (`AnimGroup.prepareToRender`; stages 113, 125 and 128 have such models, on the
others the stage itself is not reflected). **Left out:** the other anim-group models, background and
foreground objects with flag bit 7 (`BgObjectInst`: 1 on stage 62, 11 to 17 on stages 104, 121, 123, 125
and 128), the mirror models, and all particles (`Renderer::set_in_mirror`, the renderer's own rules).

**Reflections off** (the option below): `set_mirror` is never called and the mirror models are not
created. The stage is then built and drawn exactly as before the mirrors existed.

Cost, at 640 x 360 (`tests/budgets.rs`, `DRAWS` and `MIRROR_DRAWS`): the draws of a frame without the
mirror and with it, in the fly-in (tick 60) and in play (tick 420), the dearest stages first, and what
version 0.1.0 drew for the same frame without any mirror.

| Stage | Fly-in | Play | 0.1.0 (and its additions), no mirror |
|---|---|---|---|
| 93 | 293 -> 567 (+94 %) | 303 -> 586 (+93 %) | 386 / 396 |
| 76 | 364 -> 613 (+68 %) | 367 -> 697 (+90 %) | 605 / 602 |
| 89 | 383 -> 706 (+84 %) | 444 -> 842 (+90 %) | 744 / 848 |
| 48 | 352 -> 665 (+89 %) | 175 -> 313 (+79 %) | 497 / 244 |
| 92 | 153 -> 287 (+88 %) | 163 -> 306 (+88 %) | 216 / 226 |
| 94 | 154 -> 288 (+87 %) | 144 -> 271 (+88 %) | 218 / 198 |
| 77 | 248 -> 356 (+44 %) | 205 -> 385 (+88 %) | 474 / 344 |
| 91 | 155 -> 290 (+87 %) | 165 -> 309 (+87 %) | 222 / 232 |
| 72 | 255 -> 437 (+71 %) | 193 -> 359 (+86 %) | 495 / 360 |
| 71 | 190 -> 304 (+60 %) | 176 -> 325 (+85 %) | 327 / 271 |
| 105 | 244 -> 396 (+62 %) | 248 -> 457 (+84 %) | 404 / 393 |
| 75 | 260 -> 431 (+66 %) | 262 -> 480 (+83 %) | 485 / 483 |
| 78 | 253 -> 391 (+55 %) | 216 -> 390 (+81 %) | 464 / 365 |
| 82 | 186 -> 303 (+63 %) | 234 -> 423 (+81 %) | 256 / 356 |
| 113 | 271 -> 427 (+58 %) | 226 -> 406 (+80 %) | 431 / 341 |
| 85 | 239 -> 289 (+21 %) | 232 -> 407 (+75 %) | 349 / 342 |
| 111 | 252 -> 388 (+54 %) | 227 -> 397 (+75 %) | 403 / 343 |
| 21 | 417 -> 657 (+58 %) | 411 -> 711 (+73 %) | 588 / 580 |
| 62 | 107 -> 182 (+70 %) | 117 -> 194 (+66 %) | 167 / 175 |
| 102 | 229 -> 368 (+61 %) | 200 -> 340 (+70 %) | 372 / 308 |
| 103 | 236 -> 380 (+61 %) | 206 -> 348 (+69 %) | 429 / 364 |
| 125 | 415 -> 684 (+65 %) | 418 -> 706 (+69 %) | 541 / 537 |
| 101 | 305 -> 446 (+46 %) | 300 -> 497 (+66 %) | 493 / 470 |
| 83 | 146 -> 189 (+29 %) | 202 -> 333 (+65 %) | 189 / 291 |
| 106 | 322 -> 530 (+65 %) | 248 -> 410 (+65 %) | 498 / 368 |
| 110 | 235 -> 348 (+48 %) | 206 -> 331 (+61 %) | 383 / 312 |
| 109 | 373 -> 537 (+44 %) | 328 -> 504 (+54 %) | 591 / 502 |
| 104 | 232 -> 337 (+45 %) | 200 -> 305 (+53 %) | 375 / 308 |
| 112 | 233 -> 327 (+40 %) | 235 -> 359 (+53 %) | 395 / 382 |
| 74 | 214 -> 279 (+30 %) | 188 -> 280 (+49 %) | 374 / 323 |
| 108 | 377 -> 462 (+23 %) | 270 -> 399 (+48 %) | 550 / 388 |
| 129 | 179 -> 213 (+19 %) | 186 -> 275 (+48 %) | 323 / 331 |
| 86 | 253 -> 318 (+26 %) | 243 -> 341 (+40 %) | 380 / 367 |
| 95 | 166 -> 212 (+28 %) | 176 -> 222 (+26 %) | 235 / 245 |
| 121 | 252 -> 300 (+19 %) | 274 -> 345 (+26 %) | 378 / 401 |
| 128 | 188 -> 222 (+18 %) | 195 -> 239 (+23 %) | 319 / 327 |
| 123 | 261 -> 292 (+12 %) | 268 -> 318 (+19 %) | 393 / 401 |

The capture is most of the picture again: everything but the stage's own models, so the backdrop with
its objects, the bananas, the bumpers and the goals. Two things keep it down, neither of which changes
a pixel of the picture:

* **The capture is cut to the mirror.** The overlay reads the capture only where a fragment of a mirror
  model lands in it, so only the bounds of the mirror models in view, as the capture sees them, are
  drawn (a scissor rectangle, two texels wider for the bilinear filter), and only the objects whose
  spheres reach into that part of the view (`mb-render`, `mirror::projected_box`, `window_planes`). With
  a floor that fills the lower half of the screen that saves little; where the mirror is a strip or a
  tile in the distance it saves most of the pass, and nothing is captured when no mirror model reaches
  into the picture. Over 183 frames of nine stages from 21 cameras, 16 % of the capture's draws.
  `cutting_the_capture_to_the_mirror_changes_no_pixel` draws each of those frames both ways and
  compares the bytes.
* **A model's meshes of one state are one mesh** (`Renderer::make_meshes`), in every pass: see
  `docs/GMA.md`. All 109 stages together draw 25,517 / 23,343 calls at the two ticks where 0.1.0 drew
  38,960 / 35,048, so a mirror stage with its capture now draws about what 0.1.0 drew without one
  (stage 89 in play: 842 against 848).

What is left is the reflection itself: on the stages at the top of the table the capture holds nearly
everything the picture holds (the 40 bumpers and 40 bananas of stage 93, the 30 bumpers of stage 89),
all of it really visible in the floor, so the pass stays at 80 to 94 % of the frame's draws there. The
dearest frames are now those of stages 89 (842), 21 (711) and 125 (706). The master floors are cheap
because their background objects carry flag bit 7. The target is half the frame each way, colour and
depth: 460,800 bytes under 640 x 360, 1.84 MB under 1280 x 720 (`Renderer::mirror_bytes`, not texture
bytes). No texture is uploaded for a mirror; a flat mirror adds an object per mirror model. The load
creates the mirror's buffers, bind groups and target; a play frame creates nothing, also while the
plane of stage 103 moves or the level of stage 110 changes; a resize of the frame replaces the target
(two textures, one bind group) with the frame's depth texture, in the first frame after it. The
capture is submitted by itself before the picture is encoded, so a backend that keeps its command
lists in ordinary memory (WebGL2) never holds both.

Different from the reference, on purpose:

- **The plane comes from the mirror models.** `getMirrorPlaneMatrix` takes the height of the first
  named model's bounding centre for most stages, fixed choices for stages 48, 102 and 110, and anim
  group 1's transform for stage 103. `tests/mirrors.rs` has both planes for every stage. They are the
  same on 32 stages; on stages 48 and 102 the reference's y = 0 is the floor 0.015 under the mirror
  model, and on stage 110 its two heights are 0.01 under the models, which makes no visible
  difference. They differ on two:
  - **Stage 103.** Group 1's transform is the plane y = 0 of the space the group's models are authored
    in. `STAGE103_MIR` is not there: like every model of the group it is authored around the group's
    initial pose, in a plane through y = 4.95 that leans 9 degrees. With the reference's plane the
    mirror reflects about a plane five units below it and shows black space with no goal or banana in
    it; with the model's own plane the goal and the bananas hang mirrored under themselves.
  - **Stage 74.** The first model its stagedef names (`MIR_STAGE074_B_1`) is in no archive, at which
    the reference gives up and shows no mirror; the second (`MIR_STAGE074_A`) exists and is the mirror
    here.
- **No distortion on the bonus wave.** The reference bends the wave floor's reflection by an offset
  read from a texture (`gb_grad`, through a second target). As it stands in the reference, and as
  `mb-render` ported it until 0.2.0, the offset is not a wobble about zero: it moves the whole reflection 0.28 of
  the picture's height down (the alpha it reads is always one) and, where the floor is level, about a
  tenth of its width sideways, so the bananas' reflections lie some 200 of 720 pixels below the bananas
  and the shifted horizon is a hard edge across the floor. Compared side by side, the undistorted
  reflection is the one that is under its objects. Stage 92 therefore has a plain mirror with the
  reference's plane and opacity; the distortion was never switched on and its code was removed after
  0.2.0.
- **An option.** The reference always draws its mirrors.
- The renderer's differences (the cut at the plane, the size of the target, the wave floor missing from
  its own reflection) are listed in `docs/GMA.md`.

Seen in screenshots (1280 x 720 and 640 x 360, desktop) of stages 21, 48, 72, 74, 77, 89, 91, 92, 102,
103, 105, 110, 121 and 123: the reflection is a mirror image hanging from the floor under each thing,
the ball's meets the ball where it stands, bananas and goals stand on their own reflections, and there
is no seam or flicker at the floor; the half-size target shows as slightly soft, stair-stepped edges in
the reflection. On stage 92 the reflections of the bananas and the ball are under them, on a floor that
ripples through them. On stages 21, 48 and 74 the mirror is a part of the floor away from the start and
little or nothing of it is in the pictures taken there.

### Material switches (`RenderParams`)
What `World.ts`, `AnimGroup.ts`, `BgObject.ts` and `Background.ts` set per object, and where it is set here:

| Reference | Here |
|---|---|
| `BallInst`: `disableSpecular` on `CLEAR_HEMI_INSIDE`, `RED_HEMI_INSIDE` | `Renderer::set_specular_disabled` on those two shells |
| `BallInst`: `depthOffset` 4, 3, 2, 1, 0 inside out, `RenderSort.Translucent` | unchanged from 0.1.0: `set_sort_bias` of a thousandth of those, which keeps the same inside-out order but sorts the ball against other blended objects by its true distance (with the reference's 4 units a blended object up to 4 units behind the ball is drawn between its shells) |
| `BgObjectInst` (background and foreground objects): `depthOffset = 400` | `set_depth_offset(400)` |
| `BgObjectInst`: `alpha = 1 - translucency`, `RenderSort.All` when translucent | `set_alpha` (an alpha below one sorts every mesh), since 0.1.0 |
| `BgObjectInst`: `lightingGroups.getForBgFlags(flags)`, no specular scale for `flags >>> 28 != 0` | not ported: no background or foreground object of the 109 course stages is in such a group (a switch for it was in the renderer until 0.2.0, without effect) |
| `BgObjectInst` / `AnimGroup`: `textureScroll` | SMB2 only; not parsed |
| `BgSunset`: `texMtx` | `set_tex_offset`, above |
| `BgNight`: `RenderSort.None` | a sort offset, above |
| `World.bonusWaveModel`: material hack, `setForceCullMode(None)` | `set_wave`, both faces, above |
| Stage models, goals, bananas, tape, digits: `RenderSort.None`; goal bag: `RenderSort.All` | the bag has `set_sort_bias(0)` since 0.1.0; a stage model's blended meshes are sorted, which the depth test makes equivalent except where two of them overlap |
| Effects: `colorMul`, `alpha` | instance colours (`particles.rs`) |

## Controls
| | Keyboard | Gamepad (desktop: gilrs, web: Gamepad API) | Touch |
|---|---|---|---|
| Tilt | WASD or arrows | left stick | on-screen stick (bottom left) |
| Menu select | arrows / WASD, Enter | stick or d-pad, A | tap a row, drag to scroll |
| Back | Esc | B | Back row |
| Pause | Esc or P | Start | on-screen pause button |
| Action (hold: fly-in at double speed, a fall-out cut short) | Space, Z or Enter | A | hold the play area outside the stick and buttons |
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

The same button cuts a fall-out short (the reference's `canSkip`, `docs/ENGINE.md`): the sequence lasts 270
ticks, and from its 60th tick on a tick with the action down is its last. It only has to be down, not
pressed anew. Nothing else differs: the state ends with the same `StageOutcome::FallOut`, so the life, the
retry on the loaded scene (or the game over) and what is saved are those of an unshortened fall
(`the_action_button_cuts_a_fall_out_short_at_the_same_cost`: 270 ticks against 60, same lives, score and
save). The `FALL OUT` banner belongs to the state and goes with it; leaving a stage cuts its sounds, as
always, so the announcer's line is cut where the sequence ends. A bonus stage's fall-out (120 ticks) cannot
be shortened. The next attempt is a new state, in which the button counts again
once it has been let go.

## Options and sound
| Option | Values | Effect |
|---|---|---|
| Unlock all stages | on / off | every course and practice floor selectable |
| Stick curve | 1.00 .. 2.00 | input falloff power |
| Music volume | off, 10 .. 100 (default 80) | stage music |
| Sound volume | off, 10 .. 100 (default 80) | effects and the announcer: roll, impacts, bananas, goal, clock, menus, READY, GO, GOAL, FALL OUT |
| Reflections | on / off (default on) | the mirror floors of the 37 stages that have one; off saves their extra pass (up to 94 % of those stages' draw calls, see "Mirror floors") on a weak device |

Selecting a row steps its value (volumes by ten, wrapping from 100 to off). Changes are saved with the
profile and apply at once; a category that is off is not played at all. Reflections is read when a stage
is built (`PlayScene::build`, from the settings in the stage's `PlayRequest`); no stage is loaded while
the options are open, so the next stage played has the new value. The row comes after the rows only some
platforms have (Controls, Tilt range, Fullscreen) and before the storage rows. In the save it is one more
`setting` line (`reflections`): a save written before it existed has none and loads with it on, and an
older build skips the line like any setting it does not know (`mb-game`,
`a_save_from_0_1_0_loads_with_reflections_on`). Menus click on move, confirm and
back. In the pause menu the music plays on and the ball's roll is silent; leaving a stage cuts its effects.
What plays when, and how sound reaches the device, is in `docs/AUDIO.md`.

## Input seam
Gameplay reads exactly one thing: `InputState::tilt() -> (x right, y forward)`, a vector in the unit circle.
Its sources, in priority order like the reference's `getStick`: the `TiltX`/`TiltY` axes (device tilt, fed by
the browser target through `tilt::TiltSteering`), the `MoveX`/`MoveY` axes (gamepad or touch stick), then
keys (unramped, as in the reference; the world tilt itself eases). `PlayState` applies the stick curve and
`quantize_input`. A new input device only has to write actions and axes into `InputState` from its driver
(`mb-desktop`'s `app.rs`, the `Platform` hooks of `mb-web`'s `shell.rs`); nothing in the game states
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

The goal banner, as `hud.ts` draws it:

- `GOAL`: four copies fly in over 30 frames, then for 15 frames the assembled banner flashes white: its own
  shape in plain white over it at `sin` of a half turn (`drawSpriteSolidTint`; `SpriteBlend::Solid`, in the
  same draw call as the banner).
- `GO`: the letters turn from `#00a0ff` to `#c0f2ff` on every other pair of frames between frames 15 and 45.
  The reference does this with a multiplying tint, not a white flash, and so does the port (since 0.1.0).
- Through a green or red goal in course play: `JUMP TO FLOOR n` in `#00c0ff` sways about the centre
  (`320 + 20 sin(t << 9)`, `240 + 10 sin((t << 8) + 0x4000)`) and leaves a trail on every odd frame that
  lives 16 frames, growing and fading from half opacity while its colour runs from `rgb(0, 96, 127)`
  towards cyan (at most eight at once). The score lines get ` WARP BONUS    X n` between `CLEAR SCORE` and
  `TIME BONUS`, ten frames after the first, and `FLOOR SCORE` is clear score x n x time bonus.
  `n` is `Course::peek_jump_count` for the goal the ball went through: `menu.rs` hands the gameplay state
  the course as it stands at the floor (`PlayState::with_course`; `PlayRequest` is unchanged), and
  `play.rs` asks it on the tick the banner starts.

Different from `hud.ts` here:

- **The floor in `JUMP TO FLOOR n`.** The reference prints the course's current floor, which is the floor
  being left (the course advances when the sequence is over). The port prints the floor the jump leads to,
  current floor + n.
- **Opacity of the line and its trails.** `hud.ts` passes their opacities to `drawText` in the place of the
  vertical alignment, so it draws them (and `HURRY UP!`) fully opaque. The port uses the values as the
  opacities they are, as it has for `HURRY UP!` since 0.1.0.
- **No course, no jump.** Without a course the reference falls back to a warp factor of 2 (green) or 3
  (red) and still shows the line. In practice nothing jumps, so the port shows neither.
- **`FLOOR SCORE` on a warp goal is more than is awarded.** As in the reference: its banner multiplies by
  the warp factor and its game does not (`computeGoalScore`; here `mb_game::Rules::goal_score` and the
  engine's score). Without a warp the two are equal, which `goal_banner_prices_the_floor_like_the_course_flow`
  holds. The original game awards the warp bonus; doing so is a rules change in `mb-game` and `mb-engine`.

## Automated verification
- `cargo test -p mb-app`: `tests/gameplay.rs` drives the real states on an offscreen renderer (goal advances
  the course and saves, fall-out retries until game over and continue, pause / retry / quit in practice,
  pointer and stick input, every course stage loads and renders); `tests/pack.rs` does the same over an
  imported game pack behind a deferred source, the way the browser reads; `tests/pause.rs`, `tests/rules.rs`;
  `crates/mb-desktop/tests/kill.rs` (the real program killed mid-run). They skip without a disc or a GPU
  adapter.
- `tests/gameplay.rs` and `tests/pack.rs` also play floor 1 and compare every frame's particle instances and
  quads with what the simulation holds (streaks, landing burst, confetti, sparkles, the shadow on exactly
  while the ball is visible, a pause that changes nothing), and `gameplay.rs` holds the action button through
  the fly-in in every way it can be held.
- `tests/gameplay.rs` also has: the windows of stage 21 and the fires of stage 81 showing the frame the
  stage timer names on every tick (and the fires turned to the camera), the clouds of stage 5 (still through
  the fly-in, drifting at the reference's speed, the same after a reload and a retry, hurried at the
  hurry-up), the wave floor of stage 92 under the ball, the green goal of Beginner floor 2 in course play
  (`JUMP TO FLOOR 5`, warp bonus 3, the course at floor 5 afterwards) and in practice (neither line), the
  shadow on and off, and the shortened fall-out. `tests/pack.rs` loads the same four backgrounds over the
  pack. `MB_SHOT_DIR=<dir outside the repository>` makes the goal, wave and mirror tests save PNGs there.
- The mirror floors. `tests/mirrors.rs` (no GPU): for every course stage, whether it has a mirror, how
  many models its stagedef names, on how many anim groups, how many are missing or dropped, the plane
  of each level, and the reference's plane beside it; the list is recorded in the test. `mirror.rs` has
  the rule's unit tests (shared planes across groups, a plane that rides, two levels and the choice by
  the eye, stray vertices, models that are not flat or face the other way). In `tests/gameplay.rs`:
  twelve mirror stages of every kind are played at 144 frames a second and every mirror model stays in
  the plane the frame mirrors in, the plane of stage 103 being the blend its model is drawn with, while
  six stages that name no mirror model have none; stage 110 mirrors in the floor under the camera and
  draws only that floor's model; the same four frames of twelve mirror stages played with the option on
  and off differ in up to 150,000 of 230,400 pixels, every one of them a pixel whose ray meets the
  mirror's plane inside a mirror model, the frame's other draws being equal, and in no pixel on stages
  1 and 5; the options row turns the option off with the keys and on with a tap on a 540 x 1200
  screen, saves at once, and a menu started again on the save loads stage 102 without and then with
  its mirror. `tests/pack.rs` loads seven mirror stages over the pack with the option on and off.
  `menu.rs` has the row's unit test.
- `tests/budgets.rs`: the regression budgets. Draw calls outside the particles of
  every course stage at tick 60 (fly-in) and 420 (play) equal a recorded table (`DRAWS`) and are never
  more than the table recorded from 0.1.0 (its model draws were compared with a build of the `v0.1.0`
  tag, all 109 stages identical) plus what `DRAWS_ADDED` records
  for the 22 stages that draw something new (the wave floor of stage 92: 1; the windows of the eleven night
  stages with shown diners: 4 to 29; the fires of the ten storm stages: 0 to 4); particles add at most 12;
  the renderer's `GpuCounters`, texture and object counts do not change from the end of a stage's load to
  the end of its goal sequence, on the first stage of a session too (`PlayScene::build` sizes the buffer
  the objects share with `Renderer::reserve_objects`; only the title menu has drawn a frame before);
  stage 109 uploads at most 1 MB more texture bytes than 0.1.0's 16,617,412, and no night or storm stage,
  with its 2.1 to 2.2 MB of flipbook textures, uploads more than that limit (the largest is stage 90 with
  14,251,776). The mirror's draws are counted apart (`FrameStats::mirror_draws`): a table holds them
  for the 37 mirror stages at the two ticks, no other stage has any, the frame's own draws of every
  stage are those of its row whether the mirror is on or off, no mirror doubles the draws of a
  frame, and with Reflections off the 37 stages have neither mirror draws, mirror models nor a target
  and upload the same texture bytes; the target is 460,800 bytes at the tests' 640 x 360 and is not
  counted with the textures; seven mirror stages (the wave, 102, 103, a bonus floor, an ice, a space
  and a master floor) create nothing over 520 frames of steering, a resize creates three textures and
  one bind group in its first frame, and the 200 frames after it nothing.
  `tests/pack.rs` holds the pack at 382 files and 144,084,942 bytes. `MB_BUDGET_PRINT=1` prints the tables.
- `tests/memory.rs`: the memory budget, under an allocator that counts, on `wgpu`'s OpenGL backend (the
  one the browser uses as WebGL2) with the music off. For stages 1, 21, 76, 89, 90, 92 and 109, with
  reflections on, the peak of live heap bytes over the load and 480 drawn frames and the bytes of the
  stage at rest are at most 2 MB above what 0.1.0 needed, measured the same way in a checkout of its
  tag (the figures are in the test); and no load hands the GPU queue more than about a megabyte, or
  one texture level, at once. Measured: peaks of 5.5 to 12.5 MB, between 0.76 MB below 0.1.0 (stage
  89) and 0.29 MB above (stage 21); at rest 0.22 to 1.32 MB above, of which 0.33 MB are the particle
  pool of the simulation, 0.16 MB the particles' instance and quad buffers, and on the night stage
  about 1 MB its 370 window objects and 110 frame models. The top of the file says where this differs
  from wasm memory in the browser.
- `cargo test -p mb-app --test audio`: the autopilot's floor 1 with a recording audio backend and an offline
  render of its sound.

The tests run on developer support that the product does not contain: the windowless driver (`headless.rs`),
the waypoint autopilot (`autopilot.rs`) and a PNG writer (`image.rs`) are compiled only with the `dev` cargo
feature, which `cargo test` switches on by itself. A `dev` build of the program also has one option:
`cargo run --release --features dev -- [--stage <n>] --exit-after <s>` closes the window after that long and prints frame
pacing and simulation ticks against the 60 Hz target, and what the audio device played (stream, callbacks,
peak / RMS, late callbacks). To look at pictures of a run, `MB_SHOT_DIR=<dir> cargo test -p mb-app --release
--test gameplay` writes PNGs of what the state tests draw.

## Not ported from the reference
- Result replays: the reference replays the fall-out and the goal, and with them come the goal-sequence
  skip and the rest of the confetti's life. Dropped by decision on 2026-10-13.
- Not in the reference either (original game only): the monkey inside the ball, its voice lines and the
  sounds listed under "Known gaps" in `docs/AUDIO.md`.
- Multiplayer, netplay, lobby, the replay-file player, the map viewer, SMB2 content (out of scope by
  decision).

Differences on purpose: lives are shown (the reference hides its counter), the HUD is edge-anchored instead
of letterboxed, tall targets get a wider vertical field of view, bumpers show the simulation's squash. The
other deliberate differences are in the sections above ("Shadow and particles", "Backgrounds and the bonus
wave", "Mirror floors", "Material switches", "HUD"); `docs/PLAN.md` has the index of all of them.
