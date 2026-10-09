//! Gameplay state: one stage, started from an `mb_game::PlayRequest`, ended by writing a
//! `mb_game::PlayResult` into the request's report slot and popping itself.
//!
//! The simulation (`mb_engine::Sim`) advances in fixed 60 Hz ticks from an accumulator, independent of the
//! display rate. Rendering blends the last two ticks by the accumulator's fraction of a tick (the reference's
//! `getInterpolationAlpha`), so motion is even at any display rate and under frame-time jitter; the blend is
//! display only and never reaches the simulation. Flow follows `sim.phase`: fly-in and READY, GO, play, then
//! the goal / fall-out / time-over sequence, after which the outcome is reported. Pause (Esc / P / Start, or
//! the on-screen button) offers continue, retry and quit; what retry and quit may do is decided by
//! [`retry_available`] and [`quit_needs_confirmation`].

use std::cell::RefCell;
use std::rc::Rc;

use glam::{Mat4, Vec3};
use mb_engine::determinism::quantize_input;
use mb_engine::{Outcome, Phase, Sim, SimConfig, BUTTON_PRIMARY_ACTION};
use mb_game::progress::ControlMode;
use mb_game::{GoalType, PlayMode, PlayRequest, PlayResult, StageOutcome};
use mb_render::Camera;

use crate::audio::{Audio, Sfx, StageSounds};
use crate::game::{Game, GameCtx, PlayView, Transition};
use crate::hud::{Hud, HudFrame};
use crate::input::{Action, InputState};
use crate::play_scene::{lerp_s16, PlayScene};
use crate::stage_scene::S16_TO_RADIANS;
use crate::text::{rgb, Align, FontId, Painter, UiAssets};
use crate::ui::{Layout, Rect};
use crate::widgets::{
    back_button_rect, draw_back_button, draw_list, fit_scale, ListLayout, ListState, Row, Tap,
};

/// Seconds per simulation tick.
pub const TICK: f64 = 1.0 / 60.0;
/// Most ticks run in one frame; a longer stall is dropped instead of fast-forwarding the game.
const MAX_CATCH_UP: u32 = 5;
/// Frame times this close to a whole number of ticks are treated as exactly that many, so a vsynced
/// 60 Hz (or 30 Hz) display steps the simulation evenly instead of 0 / 2 ticks on timer jitter.
const SNAP: f64 = 0.12;

const PAUSE_CONTINUE: usize = 0;
const PAUSE_RETRY: usize = 1;
const PAUSE_QUIT: usize = 2;

const CONFIRM_KEEP_PLAYING: usize = 0;
const CONFIRM_QUIT: usize = 1;

/// Whether the pause menu's Retry may be chosen.
///
/// * Only while the attempt is undecided (fly-in or play). Once the ball is through the goal, has fallen
///   out or the clock has run out, the sequence on screen is the result: a retry there would turn a goal
///   into a lost life, or dodge a fall-out's cost in practice.
/// * Never on a bonus stage in course play. A bonus stage is one attempt: failing it costs nothing and
///   the course moves on, so "retry" (which is reported as a fall-out) would skip the stage instead.
///
/// Pausing itself, Continue and Quit work in every phase.
pub fn retry_available(mode: PlayMode, bonus_stage: bool, phase: Phase) -> bool {
    let undecided = matches!(phase, Phase::Intro | Phase::Playing);
    undecided && !(mode == PlayMode::Course && bonus_stage)
}

/// Quitting a course gives up the whole run (floors, score, lives), so it asks first; quitting practice
/// loses nothing.
pub fn quit_needs_confirmation(mode: PlayMode) -> bool {
    mode == PlayMode::Course
}

fn goal_type(g: mb_stage::GoalType) -> GoalType {
    match g {
        mb_stage::GoalType::Blue => GoalType::Blue,
        mb_stage::GoalType::Green => GoalType::Green,
        mb_stage::GoalType::Red => GoalType::Red,
    }
}

/// Where the accumulator rests on a display that runs a whole number of ticks per frame, as a fraction of a
/// tick. The frame shows the blend at that fraction, so a high value keeps what is drawn close to the newest
/// tick (0 would lag a full tick behind it).
const REST_PHASE: f64 = 0.85;
/// Share of the distance to [`REST_PHASE`] covered per snapped frame: slow enough to be invisible (well under
/// 1 % of a tick's motion per frame), and it moves both ways, so the game clock is not biased.
const REST_PULL: f64 = 0.04;

/// `dt` as a whole number of ticks when it is close to one (see [`SNAP`]).
fn snapped(dt: f64) -> Option<f64> {
    let whole = (dt / TICK).round();
    ((1.0..=2.0).contains(&whole) && (dt - whole * TICK).abs() < TICK * SNAP)
        .then_some(whole * TICK)
}

/// After a snapped frame, ease the accumulator towards [`REST_PHASE`].
pub fn settle_phase(rest: f64, dt: f64) -> f64 {
    if snapped(dt.max(0.0)).is_some() {
        rest + (REST_PHASE * TICK - rest) * REST_PULL
    } else {
        rest
    }
}

/// How many ticks to run for a frame of `dt` seconds; returns the count and the new accumulator.
pub fn ticks_for_frame(accumulator: f64, dt: f64) -> (u32, f64) {
    let mut dt = dt.max(0.0);
    if let Some(whole) = snapped(dt) {
        dt = whole;
    }
    let total = accumulator + dt;
    let steps = ((total + 1e-9) / TICK).floor();
    if steps > MAX_CATCH_UP as f64 {
        return (MAX_CATCH_UP, 0.0);
    }
    (steps as u32, (total - steps * TICK).max(0.0))
}

/// The reference's `applyInputFalloff`: ease the stick magnitude with `power` (1 = linear).
pub fn apply_falloff((x, y): (f32, f32), power: f32) -> (f32, f32) {
    let max_axis = x.abs().max(y.abs());
    if max_axis <= 0.0 {
        return (x, y);
    }
    let scale = max_axis.min(1.0).powf(power) / max_axis;
    (x * scale, y * scale)
}

/// Vertical field of view for a target of `aspect` (width / height). The reference always uses 60 degrees,
/// which is fine from 4:3 upwards; narrower targets (a phone held upright) keep the horizontal view of a
/// 4:3 screen instead, so the stage does not turn into a keyhole.
pub fn fov_y_for_aspect(aspect: f32) -> f32 {
    let base = 60f32.to_radians();
    let reference = 4.0 / 3.0;
    if aspect >= reference || aspect <= 0.0 {
        return base;
    }
    let half_width = (base / 2.0).tan() * reference;
    (2.0 * (half_width / aspect).atan()).min(MAX_FOV_Y)
}

/// Widest vertical field of view used on tall targets (110 degrees).
const MAX_FOV_Y: f32 = 1.92;

/// The simulation's camera as a render camera: `T(eye) * Ry * Rx * Rz` (`applyGameCamera`).
pub fn camera_from(c: &mb_engine::GameplayCamera, aspect: f32) -> Camera {
    let eye = Vec3::new(c.eye.x as f32, c.eye.y as f32, c.eye.z as f32);
    let world = Mat4::from_translation(eye)
        * Mat4::from_rotation_y(c.rot_y as f32 * S16_TO_RADIANS)
        * Mat4::from_rotation_x(c.rot_x as f32 * S16_TO_RADIANS)
        * Mat4::from_rotation_z(c.rot_z as f32 * S16_TO_RADIANS);
    Camera {
        view: world.inverse(),
        fov_y: fov_y_for_aspect(aspect),
        ..Camera::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y)
    }
}

/// What rendering needs of the simulation's camera at one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub eye: Vec3,
    /// S16 angles about X, Y, Z.
    pub rot: [i32; 3],
}

impl CameraPose {
    pub fn of(c: &mb_engine::GameplayCamera) -> Self {
        Self {
            eye: Vec3::new(c.eye.x as f32, c.eye.y as f32, c.eye.z as f32),
            rot: [c.rot_x, c.rot_y, c.rot_z],
        }
    }

    /// A cut (new camera mode, respawn) rather than a move: the eye or an angle jumps further than any
    /// camera travels in one tick.
    fn is_cut_from(&self, prev: &CameraPose) -> bool {
        const EYE: f32 = 8.0;
        const ANGLE: i32 = 0x2000;
        (self.eye - prev.eye).length() > EYE
            || (0..3).any(|i| (self.rot[i].wrapping_sub(prev.rot[i]) as i16 as i32).abs() > ANGLE)
    }

    /// The render camera `t` of the way from `prev` to `self` (`getCameraPose(alpha)`).
    pub fn blend(&self, prev: &CameraPose, t: f32, aspect: f32) -> Camera {
        let eye = prev.eye.lerp(self.eye, t);
        let rot = |i: usize| lerp_s16(prev.rot[i], self.rot[i], t) * S16_TO_RADIANS;
        let world = Mat4::from_translation(eye)
            * Mat4::from_rotation_y(rot(1))
            * Mat4::from_rotation_x(rot(0))
            * Mat4::from_rotation_z(rot(2));
        Camera {
            view: world.inverse(),
            fov_y: fov_y_for_aspect(aspect),
            ..Camera::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y)
        }
    }
}

fn aspect(size: (u32, u32)) -> f32 {
    size.0.max(1) as f32 / size.1.max(1) as f32
}

pub struct PlayState {
    request: PlayRequest,
    ui: Rc<UiAssets>,
    scene: Option<PlayScene>,
    sim: Option<Sim>,
    hud: Hud,
    accumulator: f64,
    pause: Option<ListState>,
    /// The "quit the course?" question, on top of the pause menu.
    quit_confirm: Option<ListState>,
    camera: Camera,
    /// Camera of the last two ticks (`[previous, current]`), blended for the frame.
    cameras: [CameraPose; 2],
    ticks: u64,
    audio: Audio,
    /// Sound triggers of the running attempt.
    sounds: Option<StageSounds>,
    /// Where the scene goes when this state ends, so a retry of the same stage can take it over instead
    /// of loading everything again. Without one the scene is unloaded on exit.
    park: Option<Rc<RefCell<Option<PlayScene>>>>,
    /// The pause screen's Back button (resumes).
    back: Tap,
    /// The primary action has been seen released since the stage started or the pause closed. Until then
    /// a key that is still down from the menu (Enter on the stage, on Continue) is not the player holding
    /// the action button, and does not fast-forward the fly-in.
    primary_armed: bool,
    /// Frames left in which the HUD's textures are drawn without size ([`UiAssets::warm`]). More than
    /// one, because a driver may drop a frame it cannot present (and the windowless one drops the first).
    warm_frames: u8,
}

impl PlayState {
    /// Play `request` on `scene`, which is loaded and built already (by `loading::StageLoad`, or by the
    /// previous attempt at the same stage).
    pub fn new(
        request: PlayRequest,
        ui: Rc<UiAssets>,
        audio: Audio,
        scene: PlayScene,
        park: Option<Rc<RefCell<Option<PlayScene>>>>,
    ) -> Self {
        let hud = Hud::new(!request.retry, Self::start_score(&request));
        Self {
            request,
            ui,
            park,
            back: Tap::default(),
            primary_armed: false,
            warm_frames: 3,
            scene: Some(scene),
            sim: None,
            hud,
            accumulator: REST_PHASE * TICK,
            pause: None,
            quit_confirm: None,
            camera: Camera::look_at(Vec3::new(0.0, 5.0, 10.0), Vec3::ZERO, Vec3::Y),
            cameras: [CameraPose {
                eye: Vec3::ZERO,
                rot: [0; 3],
            }; 2],
            ticks: 0,
            audio,
            sounds: None,
        }
    }

    fn start_score(request: &PlayRequest) -> u64 {
        match request.mode {
            PlayMode::Course => request.score,
            PlayMode::Practice => 0,
        }
    }

    fn sim_config(&self, retry: bool) -> SimConfig {
        let id = self.request.stage_id;
        let mut cfg = if retry {
            SimConfig::retry(id)
        } else {
            SimConfig::first_attempt(id)
        };
        cfg.time_limit_frames = self.request.time_limit as i32;
        cfg.bonus_stage = self.request.is_bonus_stage();
        if let Some(scene) = &self.scene {
            if let Some(y) = scene.goal_tape_anchor_y {
                cfg.goal_tape_anchor_y = y;
            }
            cfg.bound_sphere = scene.bound_sphere;
        }
        cfg
    }

    /// Start the stage again in place (practice retry): same scene, fresh simulation, short intro.
    fn restart(&mut self) {
        let cfg = self.sim_config(true);
        let Some(scene) = &mut self.scene else {
            return;
        };
        let sim = Sim::new(&scene.assets.stagedef, cfg);
        // A new motion: nothing is blended from the old run into the new one.
        scene.reset_pose();
        scene.sync(&sim);
        self.cameras = [CameraPose::of(&sim.camera); 2];
        crate::host::post(crate::host::Command::CalibrateTilt);
        // Nothing of the old attempt keeps sounding; the music plays on.
        self.audio.stop_sounds();
        self.sounds = Some(StageSounds::new(&sim, true, &self.audio));
        self.sim = Some(sim);
        self.hud = Hud::new(false, Self::start_score(&self.request));
        self.accumulator = REST_PHASE * TICK;
        self.pause = None;
        self.quit_confirm = None;
        self.primary_armed = false;
    }

    fn retry_available(&self) -> bool {
        self.sim.as_ref().is_some_and(|sim| {
            retry_available(self.request.mode, self.request.is_bonus_stage(), sim.phase)
        })
    }

    fn hud_frame(&self, sim: &Sim) -> HudFrame {
        let course = self.request.mode == PlayMode::Course;
        HudFrame {
            intro_timer: sim.intro_timer,
            goal_timer: sim.goal_timer,
            ringout_timer: sim.ringout_timer,
            timeover_timer: sim.timeover_timer,
            time_limit: sim.cfg.time_limit_frames,
            stage_timer: sim.stage_timer_frames,
            // Capped like the course flow caps it, so the counter never shows more than will be kept.
            score: (Self::start_score(&self.request) + sim.score.max(0) as u64)
                .min(mb_game::run::MAX_SCORE),
            bananas: sim.ball.bananas,
            bananas_left: sim.bananas_left,
            lives: course.then_some(self.request.lives),
            speed: sim.ball.speed,
            floor: self.request.floor,
            difficulty: self.request.difficulty,
            is_bonus: sim.cfg.bonus_stage,
            is_final: course && self.request.floor >= self.request.floors,
            goal: sim.goal_hit.map(|h| goal_type(h.goal_type)),
            bonus_clear: sim.goal_hit.is_none()
                && matches!(
                    sim.phase,
                    Phase::GoalSequence | Phase::Finished(Outcome::BonusClear)
                ),
        }
    }

    fn outcome(sim: &Sim, outcome: Outcome) -> StageOutcome {
        let time_left = (sim.cfg.time_limit_frames - sim.stage_timer_frames).max(0) as u32;
        let bananas = sim.ball.bananas.max(0) as u32;
        match outcome {
            Outcome::Goal(g) => StageOutcome::Goal {
                goal: goal_type(g),
                time_left,
                bananas,
            },
            Outcome::BonusClear => StageOutcome::BonusClear { time_left, bananas },
            Outcome::FellOut => StageOutcome::FallOut { bananas },
            Outcome::TimeOver => StageOutcome::TimeOver { bananas },
        }
    }

    fn pause_layout(layout: &Layout) -> ListLayout {
        ListLayout::column(layout, 3, 0.36)
    }

    fn confirm_layout(layout: &Layout) -> ListLayout {
        ListLayout::column(layout, 2, 0.46)
    }

    fn finish(&mut self, result: PlayResult) -> Transition {
        self.request.report.set(result);
        Transition::Pop
    }

    /// Steering is by the device's tilt: the player chose it and the device reports its orientation.
    fn tilt_steering(&self) -> bool {
        self.request.settings.control_mode == ControlMode::Tilt && crate::host::caps().tilt
    }

    fn draw_touch_controls(p: &mut Painter, layout: &Layout, input: &InputState) {
        let disc = p.ui.disc;
        let tilt = input.tilt_steering();
        if !tilt {
            let radius = layout.stick_radius();
            let (origin, knob) = match input.touch_stick() {
                Some(s) => (s.origin, s.pos),
                None => {
                    let rest = layout.stick_rest();
                    (rest, rest)
                }
            };
            let base = radius * 2.0 * 1.25 / disc.w;
            p.image(disc, origin.0, origin.1, base, base, [1.0, 1.0, 1.0, 0.22]);
            let top = radius * 2.0 * 0.55 / disc.w;
            p.image(disc, knob.0, knob.1, top, top, [1.0, 1.0, 1.0, 0.5]);
        }
        for (action, rect) in layout.buttons(tilt) {
            let (cx, cy) = rect.center();
            let s = rect.w / disc.w;
            p.image(disc, cx, cy, s, s, [1.0, 1.0, 1.0, 0.28]);
            if action == Action::Pause {
                let (bw, bh) = (rect.w * 0.1, rect.h * 0.34);
                for dx in [-1.0, 1.0] {
                    p.rect(
                        Rect {
                            x: cx + dx * bw - bw / 2.0,
                            y: cy - bh / 2.0,
                            w: bw,
                            h: bh,
                        },
                        [1.0, 1.0, 1.0, 0.8],
                    );
                }
            }
            if action == Action::Recenter {
                // A crosshair: "this pose is level".
                let (long, thin) = (rect.w * 0.44, rect.w * 0.07);
                for (w, h) in [(long, thin), (thin, long)] {
                    p.rect(
                        Rect {
                            x: cx - w / 2.0,
                            y: cy - h / 2.0,
                            w,
                            h,
                        },
                        [1.0, 1.0, 1.0, 0.8],
                    );
                }
            }
        }
    }
}

impl Game for PlayState {
    fn name(&self) -> &'static str {
        "play"
    }

    fn on_enter(&mut self, ctx: &mut GameCtx) -> Result<(), String> {
        let cfg = self.sim_config(self.request.retry);
        let scene = self.scene.as_mut().ok_or("scene missing")?;
        let sim = Sim::new(&scene.assets.stagedef, cfg);
        scene.reset_pose();
        scene.sync(&sim);
        scene.apply(ctx.renderer, 1.0);
        self.cameras = [CameraPose::of(&sim.camera); 2];
        self.camera = camera_from(&sim.camera, aspect(ctx.size));
        // The loader read and started this stage's music; on a retry it is still playing. (Reading it
        // here again is a no-op in both cases and keeps a state built without the loader correct.)
        self.audio
            .stage_music(self.request.bg, &mut |path| ctx.content.read(path));
        // The pose the device is held in now is "level" for tilt steering.
        crate::host::post(crate::host::Command::CalibrateTilt);
        self.sounds = Some(StageSounds::new(&sim, self.request.retry, &self.audio));
        self.sim = Some(sim);
        Ok(())
    }

    fn on_exit(&mut self, ctx: &mut GameCtx) {
        if let Some(scene) = self.scene.take() {
            match &self.park {
                // Kept loaded: the menu underneath either retries on it or unloads it this frame.
                Some(park) => {
                    if let Some(older) = park.borrow_mut().replace(scene) {
                        older.unload(ctx.renderer);
                    }
                }
                None => scene.unload(ctx.renderer),
            }
        }
        self.sim = None;
        self.sounds = None;
        self.audio.stop_sounds();
    }

    fn update(&mut self, ctx: &mut GameCtx) -> Transition {
        if self.sim.is_none() || self.scene.is_none() {
            return self.finish(PlayResult::Quit);
        }
        // The pause screen's Back button resumes, like the keys that opened it.
        let back = self.pause.is_some()
            && self
                .back
                .update(ctx.input, back_button_rect(&ctx.layout), ctx.dt);
        let toggle = back || ctx.input.pressed(Action::Pause) || ctx.input.pressed(Action::Cancel);
        if toggle {
            if self.quit_confirm.take().is_some() {
                // Back out of the question to the pause menu, not out of the pause.
            } else {
                self.pause = match self.pause {
                    Some(_) => None,
                    None => Some(ListState::new(PAUSE_CONTINUE)),
                };
            }
        }
        ctx.input.request_virtual_controls(self.pause.is_none());
        let tilt = self.tilt_steering();
        ctx.input.request_tilt_steering(tilt);
        if tilt && self.pause.is_none() && ctx.input.pressed(Action::Recenter) {
            crate::host::post(crate::host::Command::CalibrateTilt);
        }
        let retry_available = self.retry_available();
        // `getButtonsBitmask`: the action button, once it is really the player's (see `primary_armed`).
        let primary = ctx.input.primary_held();
        self.primary_armed = self.pause.is_none() && (self.primary_armed || !primary);
        let buttons = if self.primary_armed && primary {
            BUTTON_PRIMARY_ACTION
        } else {
            0
        };

        if let (Some(confirm), true) = (&mut self.quit_confirm, self.pause.is_some()) {
            if let Some(sounds) = self.sounds.as_mut() {
                sounds.hold(&self.audio);
            }
            let layout = Self::confirm_layout(&ctx.layout);
            let before = confirm.selected;
            let chosen = confirm.update(ctx.input, &layout, &|_| true, ctx.dt);
            if chosen.is_some() {
                self.audio.play(Sfx::MenuConfirm);
            } else if confirm.selected != before {
                self.audio.play(Sfx::MenuMove);
            }
            match chosen {
                Some(CONFIRM_QUIT) => return self.finish(PlayResult::Quit),
                Some(_) => self.quit_confirm = None,
                None => {}
            }
        } else if let Some(list) = &mut self.pause {
            // The simulation stands still: so does the roll. Music and running one-shots play on, as in
            // the reference.
            if let Some(sounds) = self.sounds.as_mut() {
                sounds.hold(&self.audio);
            }
            let layout = Self::pause_layout(&ctx.layout);
            let before = list.selected;
            let chosen = list.update(
                ctx.input,
                &layout,
                &|i| i != PAUSE_RETRY || retry_available,
                ctx.dt,
            );
            if chosen.is_some() {
                self.audio.play(Sfx::MenuConfirm);
            } else if list.selected != before {
                self.audio.play(Sfx::MenuMove);
            }
            match chosen {
                Some(PAUSE_CONTINUE) => self.pause = None,
                // The list never returns a disabled row; the guard keeps the rule in one visible place.
                Some(PAUSE_RETRY) if !retry_available => {}
                Some(PAUSE_RETRY) => match self.request.mode {
                    PlayMode::Practice => self.restart(),
                    // In course play giving the stage up costs a life, like falling out.
                    PlayMode::Course => {
                        let bananas = self
                            .sim
                            .as_ref()
                            .map_or(0, |s| s.ball.bananas.max(0) as u32);
                        return self
                            .finish(PlayResult::Finished(StageOutcome::FallOut { bananas }));
                    }
                },
                Some(PAUSE_QUIT) if quit_needs_confirmation(self.request.mode) => {
                    // The safe answer is the one under the cursor.
                    self.quit_confirm = Some(ListState::new(CONFIRM_KEEP_PLAYING));
                }
                Some(PAUSE_QUIT) => return self.finish(PlayResult::Quit),
                _ => {}
            }
        } else {
            let (steps, rest) = ticks_for_frame(self.accumulator, ctx.dt as f64);
            self.accumulator = settle_phase(rest, ctx.dt as f64);
            let power = self.request.settings.input_falloff as f32 / 100.0;
            let (x, y) = apply_falloff(ctx.input.tilt(), power);
            // The engine's stick has +Y pointing down (towards the player).
            let stick = quantize_input(x as f64, -(y as f64));
            for _ in 0..steps {
                let Some(sim) = self.sim.as_mut() else {
                    break;
                };
                let was_intro = sim.phase == Phase::Intro;
                // Held on a first attempt, the action runs the fly-in at twice the speed (`fastForwardIntro`);
                // the engine decides when that applies.
                sim.step_with_buttons(stick, buttons);
                if was_intro && sim.phase != Phase::Intro {
                    // GO: control starts now, so the pose held at this moment is level.
                    crate::host::post(crate::host::Command::CalibrateTilt);
                }
                self.ticks += 1;
                if let Some(sounds) = self.sounds.as_mut() {
                    sounds.tick(sim, &self.audio);
                }
                // Record what this tick looks like; the frame blends the last two (render only).
                if let Some(scene) = self.scene.as_mut() {
                    scene.sync(sim);
                }
                let pose = CameraPose::of(&sim.camera);
                let from = if pose.is_cut_from(&self.cameras[1]) {
                    pose
                } else {
                    self.cameras[1]
                };
                self.cameras = [from, pose];
                let finished = sim.is_finished();
                let frame = self.sim.as_ref().map(|s| self.hud_frame(s));
                if let Some(frame) = frame {
                    self.hud.tick(&frame);
                }
                if finished {
                    break;
                }
            }
        }

        let (Some(sim), Some(scene)) = (self.sim.as_ref(), self.scene.as_mut()) else {
            return self.finish(PlayResult::Quit);
        };
        if let Phase::Finished(outcome) = sim.phase {
            let result = PlayResult::Finished(Self::outcome(sim, outcome));
            return self.finish(result);
        }
        let alpha = (self.accumulator / TICK).clamp(0.0, 1.0) as f32;
        scene.apply(ctx.renderer, alpha);
        self.camera = self.cameras[1].blend(&self.cameras[0], alpha, aspect(ctx.size));
        // Particles are queued per frame like sprites: a paused frame queues the same ones again.
        scene.draw_particles(ctx.renderer, sim, &self.camera, alpha);

        let frame = self.hud_frame(sim);
        let ui = self.ui.clone();
        if self.warm_frames > 0 {
            self.warm_frames -= 1;
            ui.warm(ctx.renderer);
        }
        let mut p = Painter {
            r: ctx.renderer,
            ui: &ui,
        };
        self.hud.draw(&mut p, &ctx.layout, &frame);
        if let Some(list) = &self.pause {
            let safe = ctx.layout.safe_rect();
            p.rect(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: ctx.size.0 as f32,
                    h: ctx.size.1 as f32,
                },
                [0.0, 0.0, 0.05, 0.55],
            );
            let title = if self.quit_confirm.is_some() {
                "QUIT?"
            } else {
                "PAUSE"
            };
            let s = fit_scale(&p, FontId::Asc72, title, safe.w * 0.6, safe.h * 0.14);
            p.text(
                FontId::Asc72,
                title,
                safe.x + safe.w / 2.0,
                safe.y + safe.h * 0.2,
                s,
                s,
                rgb(0xffc800),
                Align::Center,
            );
            if let Some(confirm) = &self.quit_confirm {
                let note = "THIS RUN WILL BE LOST";
                let s = fit_scale(&p, FontId::Asc32, note, safe.w * 0.8, safe.h * 0.06);
                p.text(
                    FontId::Asc32,
                    note,
                    safe.x + safe.w / 2.0,
                    safe.y + safe.h * 0.36,
                    s,
                    s,
                    rgb(0xffffff),
                    Align::Center,
                );
                let labels = ["KEEP PLAYING", "QUIT COURSE"];
                draw_list(
                    &mut p,
                    &Self::confirm_layout(&ctx.layout),
                    confirm,
                    &|i| Row {
                        label: labels[i],
                        detail: None,
                        enabled: true,
                    },
                    false,
                );
            } else {
                let layout = Self::pause_layout(&ctx.layout);
                let retry = match self.request.mode {
                    PlayMode::Practice => "RETRY",
                    PlayMode::Course if self.request.is_bonus_stage() => "RETRY",
                    PlayMode::Course => "RETRY (LOSE A LIFE)",
                };
                let labels = ["CONTINUE", retry, "QUIT"];
                draw_list(
                    &mut p,
                    &layout,
                    list,
                    &|i| Row {
                        label: labels[i],
                        detail: None,
                        enabled: i != PAUSE_RETRY || retry_available,
                    },
                    false,
                );
            }
            draw_back_button(&mut p, back_button_rect(&ctx.layout), self.back.down());
        } else if ctx.input.touch_active() || tilt {
            // With tilt the buttons are always shown: the player may never touch the screen otherwise.
            Self::draw_touch_controls(&mut p, &ctx.layout, ctx.input);
        }
        self.audio.flush();
        Transition::None
    }

    fn camera(&self) -> Camera {
        self.camera
    }

    fn status(&self) -> String {
        match &self.sim {
            Some(sim) => format!(
                "stage {} {} | {:?} | tick {}{}",
                self.request.stage_id,
                self.request.stage_label,
                sim.phase,
                self.ticks,
                match (&self.pause, &self.quit_confirm) {
                    (None, _) => "",
                    (Some(_), None) => " | paused",
                    (Some(_), Some(_)) => " | quit?",
                }
            ),
            None => String::new(),
        }
    }

    fn nav_depth(&self) -> u32 {
        1
    }

    fn play_view(&self) -> Option<PlayView<'_>> {
        let scene = self.scene.as_ref()?;
        Some(PlayView {
            request: &self.request,
            sim: self.sim.as_ref()?,
            stage: &scene.assets.stagedef,
            shadow: scene.shadow_visible(),
            paused: self.pause.is_some(),
            banner: self.hud.banner(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixty_hertz_frames_run_exactly_one_tick() {
        let mut acc = 0.0;
        let mut total = 0;
        // Jittery vsync: 16.2 .. 17.1 ms.
        for i in 0..600 {
            let dt = 0.0162 + 0.0009 * ((i * 7) % 10) as f64 / 10.0;
            let (n, rest) = ticks_for_frame(acc, dt);
            assert_eq!(n, 1, "frame {i}");
            acc = rest;
            total += n;
        }
        assert_eq!(total, 600);
    }

    #[test]
    fn other_display_rates_average_sixty_ticks_per_second() {
        for hz in [30.0, 50.0, 75.0, 120.0, 144.0, 240.0] {
            let (mut acc, mut total) = (0.0, 0u32);
            let frames = (hz * 10.0) as u32;
            for _ in 0..frames {
                let (n, rest) = ticks_for_frame(acc, 1.0 / hz);
                acc = rest;
                total += n;
            }
            assert!((total as i32 - 600).abs() <= 1, "{hz} Hz ran {total} ticks");
        }
    }

    /// What a frame shows, in ticks of simulated time: the newest tick minus one, plus the blend fraction.
    /// It must advance by exactly the frame time whatever the display rate, or motion judders.
    #[test]
    fn shown_time_advances_evenly_at_any_display_rate() {
        for hz in [50.0, 60.0, 75.0, 90.0, 120.0, 144.0, 165.0, 240.0] {
            let (mut acc, mut ticks) = (REST_PHASE * TICK, 0u32);
            let mut shown = acc / TICK;
            for i in 0..2000 {
                // Up to 10 % jitter around the nominal frame time, as `requestAnimationFrame` delivers.
                let dt = (1.0 / hz) * (0.9 + 0.2 * ((i * 37) % 11) as f64 / 10.0);
                let (n, rest) = ticks_for_frame(acc, dt);
                acc = settle_phase(rest, dt);
                ticks += n;
                assert!(
                    (0.0..TICK).contains(&acc),
                    "{hz} Hz frame {i}: accumulator {acc}"
                );
                let now = ticks as f64 + acc / TICK;
                let step = now - shown;
                shown = now;
                // A snapped frame shows a whole number of ticks (plus the gentle pull to the rest phase);
                // any other frame shows exactly its own duration.
                let expect = snapped(dt).unwrap_or(dt) / TICK;
                assert!(
                    (step - expect).abs() < 0.05,
                    "{hz} Hz frame {i}: showed {step} ticks for a frame of {expect}"
                );
            }
        }
    }

    #[test]
    fn a_steady_sixty_hertz_display_settles_close_to_the_newest_tick() {
        let mut acc = 0.0;
        for _ in 0..300 {
            let (n, rest) = ticks_for_frame(acc, TICK);
            assert_eq!(n, 1);
            acc = settle_phase(rest, TICK);
        }
        assert!((acc / TICK - REST_PHASE).abs() < 0.01);
        // An unsnapped frame leaves the accumulator alone.
        assert_eq!(settle_phase(0.004, TICK * 0.5), 0.004);
    }

    #[test]
    fn camera_blend_takes_the_short_way_and_does_not_cross_cuts() {
        let pose = |x: f32, yaw: i32| CameraPose {
            eye: Vec3::new(x, 0.0, 0.0),
            rot: [0, yaw, 0],
        };
        // Halfway between two ticks.
        let cam = pose(2.0, 0).blend(&pose(0.0, 0), 0.5, 1.5);
        assert!((cam.position() - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5);
        // Yaw from just below to just above the S16 wrap passes through the wrap, not through zero.
        let cam = pose(0.0, -0x7f00).blend(&pose(0.0, 0x7f00), 0.5, 1.5);
        let forward = cam.view.inverse().transform_vector3(Vec3::NEG_Z);
        assert!((forward - Vec3::Z).length() < 1e-3, "{forward}");
        // Ordinary motion is blended; a jump across the stage or a half turn is a cut.
        assert!(!pose(0.5, 0x200).is_cut_from(&pose(0.0, 0)));
        assert!(pose(20.0, 0).is_cut_from(&pose(0.0, 0)));
        assert!(pose(0.0, 0x4000).is_cut_from(&pose(0.0, 0)));
        assert!(!pose(0.0, -0x7f00).is_cut_from(&pose(0.0, 0x7f00)));
    }

    #[test]
    fn retry_is_offered_only_while_the_attempt_is_undecided() {
        use Phase::{FallOut, Finished, GoalSequence, Intro, Playing, TimeOver};
        for mode in [PlayMode::Course, PlayMode::Practice] {
            for phase in [Intro, Playing] {
                assert!(retry_available(mode, false, phase), "{mode:?} {phase:?}");
            }
            // Through the goal, falling, out of time, or already finished: the result stands.
            for phase in [
                GoalSequence,
                FallOut,
                TimeOver,
                Finished(Outcome::Goal(mb_stage::GoalType::Blue)),
                Finished(Outcome::BonusClear),
                Finished(Outcome::FellOut),
                Finished(Outcome::TimeOver),
            ] {
                for bonus in [false, true] {
                    assert!(!retry_available(mode, bonus, phase), "{mode:?} {phase:?}");
                }
            }
        }
        // A bonus stage is one attempt in course play (a "retry" would skip it); practice may repeat it.
        assert!(!retry_available(PlayMode::Course, true, Playing));
        assert!(!retry_available(PlayMode::Course, true, Intro));
        assert!(retry_available(PlayMode::Practice, true, Playing));
        assert!(quit_needs_confirmation(PlayMode::Course));
        assert!(!quit_needs_confirmation(PlayMode::Practice));
    }

    #[test]
    fn long_stalls_are_clamped() {
        let (n, rest) = ticks_for_frame(0.0, 2.5);
        assert_eq!((n, rest), (MAX_CATCH_UP, 0.0));
        assert_eq!(ticks_for_frame(0.0, 0.0), (0, 0.0));
        assert_eq!(ticks_for_frame(0.0, -1.0).0, 0);
    }

    #[test]
    fn falloff_eases_magnitude_and_keeps_direction() {
        assert_eq!(apply_falloff((0.0, 0.0), 2.0), (0.0, 0.0));
        assert_eq!(apply_falloff((1.0, 0.0), 2.0), (1.0, 0.0));
        let (x, y) = apply_falloff((0.5, -0.25), 2.0);
        assert!((x - 0.25).abs() < 1e-6 && (y + 0.125).abs() < 1e-6);
        assert_eq!(apply_falloff((0.5, 0.5), 1.0), (0.5, 0.5));
    }

    #[test]
    fn camera_looks_down_negative_z_at_zero_rotation() {
        let mut c = mb_engine::GameplayCamera::new();
        c.eye = mb_engine::math::Vec3::new(1.0, 2.0, 3.0);
        (c.rot_x, c.rot_y, c.rot_z) = (0, 0, 0);
        let cam = camera_from(&c, 16.0 / 9.0);
        let p = cam.view.transform_point3(Vec3::new(1.0, 2.0, 2.0));
        assert!((p - Vec3::NEG_Z).length() < 1e-5);
        assert!((cam.fov_y - 60f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn tall_targets_keep_the_horizontal_view_of_four_by_three() {
        let base = 60f32.to_radians();
        assert_eq!(fov_y_for_aspect(16.0 / 9.0), base);
        assert_eq!(fov_y_for_aspect(4.0 / 3.0), base);
        let square = fov_y_for_aspect(1.0);
        assert!(square > base);
        // Same horizontal extent as 4:3 at 60 degrees.
        let half_width = |fov: f32, aspect: f32| (fov / 2.0).tan() * aspect;
        assert!((half_width(square, 1.0) - half_width(base, 4.0 / 3.0)).abs() < 1e-5);
        assert!(fov_y_for_aspect(9.0 / 16.0) <= MAX_FOV_Y);
        assert_eq!(fov_y_for_aspect(0.0), base);
    }
}
