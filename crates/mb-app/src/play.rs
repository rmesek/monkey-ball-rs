//! Gameplay state: one stage, started from an `mb_game::PlayRequest`, ended by writing a
//! `mb_game::PlayResult` into the request's report slot and popping itself.
//!
//! The simulation (`mb_engine::Sim`) advances in fixed 60 Hz ticks from an accumulator, independent of the
//! display rate; rendering shows the latest tick. Flow follows `sim.phase`: fly-in and READY, GO, play, then
//! the goal / fall-out / time-over sequence, after which the outcome is reported. Pause (Esc / P / Start, or
//! the on-screen button) offers continue, retry and quit.

use std::rc::Rc;

use glam::{Mat4, Vec3};
use mb_engine::determinism::quantize_input;
use mb_engine::{Outcome, Phase, Sim, SimConfig};
use mb_game::{GoalType, PlayMode, PlayRequest, PlayResult, StageOutcome};
use mb_render::Camera;

use crate::game::{Game, GameCtx, PlayView, Transition};
use crate::hud::{Hud, HudFrame};
use crate::input::{Action, InputState};
use crate::play_scene::PlayScene;
use crate::stage_info::bg_by_name;
use crate::stage_scene::S16_TO_RADIANS;
use crate::text::{rgb, Align, FontId, Painter, UiAssets};
use crate::ui::{Layout, Rect};
use crate::widgets::{draw_list, fit_scale, ListLayout, ListState, Row};

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

fn goal_type(g: mb_stage::GoalType) -> GoalType {
    match g {
        mb_stage::GoalType::Blue => GoalType::Blue,
        mb_stage::GoalType::Green => GoalType::Green,
        mb_stage::GoalType::Red => GoalType::Red,
    }
}

/// How many ticks to run for a frame of `dt` seconds; returns the count and the new accumulator.
pub fn ticks_for_frame(accumulator: f64, dt: f64) -> (u32, f64) {
    let mut dt = dt.max(0.0);
    let whole = (dt / TICK).round();
    if (1.0..=2.0).contains(&whole) && (dt - whole * TICK).abs() < TICK * SNAP {
        dt = whole * TICK;
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
    camera: Camera,
    ticks: u64,
}

impl PlayState {
    pub fn new(request: PlayRequest, ui: Rc<UiAssets>) -> Self {
        let hud = Hud::new(!request.retry, Self::start_score(&request));
        Self {
            request,
            ui,
            scene: None,
            sim: None,
            hud,
            accumulator: 0.0,
            pause: None,
            camera: Camera::look_at(Vec3::new(0.0, 5.0, 10.0), Vec3::ZERO, Vec3::Y),
            ticks: 0,
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
        let Some(scene) = &self.scene else {
            return;
        };
        self.sim = Some(Sim::new(&scene.assets.stagedef, self.sim_config(true)));
        self.hud = Hud::new(false, Self::start_score(&self.request));
        self.accumulator = 0.0;
        self.pause = None;
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
            score: Self::start_score(&self.request) + sim.score.max(0) as u64,
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

    fn finish(&mut self, result: PlayResult) -> Transition {
        self.request.report.set(result);
        Transition::Pop
    }

    fn draw_touch_controls(p: &mut Painter, layout: &Layout, input: &InputState) {
        let disc = p.ui.disc;
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
        for (action, rect) in layout.buttons() {
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
        }
    }
}

impl Game for PlayState {
    fn name(&self) -> &'static str {
        "play"
    }

    fn on_enter(&mut self, ctx: &mut GameCtx) -> Result<(), String> {
        ctx.content.install_trig_tables();
        let bg = bg_by_name(self.request.bg).copied();
        let scene = PlayScene::load(ctx.renderer, ctx.content, self.request.stage_id, bg)?;
        self.scene = Some(scene);
        let cfg = self.sim_config(self.request.retry);
        let scene = self.scene.as_mut().ok_or("scene missing")?;
        let sim = Sim::new(&scene.assets.stagedef, cfg);
        scene.sync(ctx.renderer, &sim);
        self.camera = camera_from(&sim.camera, aspect(ctx.size));
        self.sim = Some(sim);
        Ok(())
    }

    fn on_exit(&mut self, ctx: &mut GameCtx) {
        if let Some(scene) = self.scene.take() {
            scene.unload(ctx.renderer);
        }
        self.sim = None;
    }

    fn update(&mut self, ctx: &mut GameCtx) -> Transition {
        if self.sim.is_none() || self.scene.is_none() {
            return self.finish(PlayResult::Quit);
        }
        let toggle = ctx.input.pressed(Action::Pause) || ctx.input.pressed(Action::Cancel);
        if toggle {
            self.pause = match self.pause {
                Some(_) => None,
                None => Some(ListState::new(PAUSE_CONTINUE)),
            };
        }
        ctx.input.request_virtual_controls(self.pause.is_none());

        if let Some(list) = &mut self.pause {
            let layout = Self::pause_layout(&ctx.layout);
            match list.update(ctx.input, &layout, &|_| true) {
                Some(PAUSE_CONTINUE) => self.pause = None,
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
                Some(PAUSE_QUIT) => return self.finish(PlayResult::Quit),
                _ => {}
            }
        } else {
            let (steps, rest) = ticks_for_frame(self.accumulator, ctx.dt as f64);
            self.accumulator = rest;
            let power = self.request.settings.input_falloff as f32 / 100.0;
            let (x, y) = apply_falloff(ctx.input.tilt(), power);
            // The engine's stick has +Y pointing down (towards the player).
            let stick = quantize_input(x as f64, -(y as f64));
            for _ in 0..steps {
                let Some(sim) = self.sim.as_mut() else {
                    break;
                };
                sim.step(stick);
                self.ticks += 1;
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
        scene.sync(ctx.renderer, sim);
        self.camera = camera_from(&sim.camera, aspect(ctx.size));

        let frame = self.hud_frame(sim);
        let ui = self.ui.clone();
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
            let s = fit_scale(&p, FontId::Asc72, "PAUSE", safe.w * 0.6, safe.h * 0.14);
            p.text(
                FontId::Asc72,
                "PAUSE",
                safe.x + safe.w / 2.0,
                safe.y + safe.h * 0.2,
                s,
                s,
                rgb(0xffc800),
                Align::Center,
            );
            let layout = Self::pause_layout(&ctx.layout);
            let retry = match self.request.mode {
                PlayMode::Practice => "RETRY",
                PlayMode::Course => "RETRY (LOSE A LIFE)",
            };
            let labels = ["CONTINUE", retry, "QUIT"];
            draw_list(&mut p, &layout, list, &|i| Row {
                label: labels[i],
                detail: None,
                enabled: true,
            });
        } else if ctx.input.touch_active() {
            Self::draw_touch_controls(&mut p, &ctx.layout, ctx.input);
        }
        Transition::None
    }

    fn camera(&self) -> Camera {
        self.camera
    }

    fn status(&self) -> String {
        match &self.sim {
            Some(sim) => format!(
                "stage {} {} | {:?} | tick {}",
                self.request.stage_id, self.request.stage_label, sim.phase, self.ticks
            ),
            None => String::new(),
        }
    }

    fn play_view(&self) -> Option<PlayView<'_>> {
        Some(PlayView {
            request: &self.request,
            sim: self.sim.as_ref()?,
            stage: &self.scene.as_ref()?.assets.stagedef,
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
