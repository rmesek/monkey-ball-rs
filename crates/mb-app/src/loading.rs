//! Stage loading as a visible state: fetch the stage's files, parse them, start its music and build the
//! scene, with a progress screen in between, then hand the finished scene to a [`PlayState`].
//!
//! Where files must be fetched asynchronously (the browser) every phase gets its own frame, so the loading
//! screen is on the display before any of the heavy work starts and the bar moves between the phases; where
//! reads are synchronous (desktop) the whole load runs in the frame that asked for it, as it always did.
//!
//! Nothing here is fatal: a stage that cannot be fetched or parsed is reported through
//! [`StageShared::outcome`] and the state pops itself, so the menu underneath can say what happened.

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;
use mb_game::{Course, PlayRequest};
use mb_render::Camera;

use crate::audio::Audio;
use crate::content::{stage_bg, Prepare, StageFiles, Wanted};
use crate::game::{Game, GameCtx, Transition};
use crate::input::Action;
use crate::play::PlayState;
use crate::play_scene::PlayScene;
use crate::stage_scene::{AssetCache, LoadTimings, StageAssets};
use crate::text::{rgb, Align, FontId, Painter, UiAssets, WHITE};
use crate::ui::Rect;
use crate::widgets::{back_button_rect, draw_back_button, fit_scale, Tap};

/// How a stage load that did not reach the stage ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    Failed(String),
    /// The player backed out of the loading screen.
    Cancelled,
}

/// What the menu shares with the states it starts.
#[derive(Clone, Default)]
pub struct StageShared {
    /// A scene left behind by a stage that ended, for a retry without reloading. Whoever takes it either
    /// plays it again or unloads it.
    pub park: Rc<RefCell<Option<PlayScene>>>,
    /// Parsed files kept between loads.
    pub cache: Rc<RefCell<AssetCache>>,
    /// Set by a load that ended without a stage.
    pub outcome: Rc<RefCell<Option<LoadOutcome>>>,
}

enum Phase {
    Fetch,
    Parse,
    Music(Box<(StageAssets, LoadTimings)>),
    Build(Box<(StageAssets, LoadTimings)>),
}

pub struct StageLoad {
    request: PlayRequest,
    ui: Rc<UiAssets>,
    audio: Audio,
    shared: StageShared,
    files: StageFiles,
    wanted: Vec<Wanted>,
    phase: Phase,
    progress: f32,
    back: Tap,
    /// Handed on to the [`PlayState`] (see `PlayState::with_course`).
    course: Option<Course>,
}

const BACKGROUND: [f32; 4] = [0.02, 0.05, 0.16, 1.0];

impl StageLoad {
    pub fn new(request: PlayRequest, ui: Rc<UiAssets>, audio: Audio, shared: StageShared) -> Self {
        let bg = stage_bg(request.stage_id, request.bg);
        let files = StageFiles::new(request.stage_id, bg, request.bg);
        Self {
            request,
            ui,
            audio,
            shared,
            files,
            wanted: Vec::new(),
            phase: Phase::Fetch,
            progress: 0.0,
            back: Tap::default(),
            course: None,
        }
    }

    /// The course the floor is part of, standing at it (course play only).
    pub fn with_course(mut self, course: Option<Course>) -> Self {
        self.course = course;
        self
    }

    fn end(&mut self, ctx: &mut GameCtx, outcome: LoadOutcome) -> Transition {
        ctx.content.release(&self.wanted);
        *self.shared.outcome.borrow_mut() = Some(outcome);
        Transition::Pop
    }

    fn fail(&mut self, ctx: &mut GameCtx, e: impl std::fmt::Display) -> Transition {
        let message = format!("stage {}: {e}", self.request.stage_id);
        self.end(ctx, LoadOutcome::Failed(message))
    }

    fn draw(&self, ctx: &mut GameCtx) {
        let ui = self.ui.clone();
        let safe = ctx.layout.safe_rect();
        let mut p = Painter {
            r: ctx.renderer,
            ui: &ui,
        };
        let full = Rect {
            x: 0.0,
            y: 0.0,
            w: ctx.size.0 as f32,
            h: ctx.size.1 as f32,
        };
        p.rect(full, BACKGROUND);
        let cx = safe.x + safe.w / 2.0;
        let label = format!("FLOOR {} {}", self.request.floor, self.request.stage_label);
        let s = fit_scale(&p, FontId::Asc32, &label, safe.w * 0.8, safe.h * 0.07);
        let y = safe.y + safe.h * 0.38;
        p.text(FontId::Asc32, &label, cx, y, s, s, WHITE, Align::Center);
        let s = fit_scale(&p, FontId::Asc20, "LOADING", safe.w * 0.5, safe.h * 0.045);
        let color = rgb(0xd8e6ff);
        let y = safe.y + safe.h * 0.5;
        p.text(FontId::Asc20, "LOADING", cx, y, s, s, color, Align::Center);
        let bar = Rect {
            x: safe.x + safe.w * 0.2,
            y: safe.y + safe.h * 0.57,
            w: safe.w * 0.6,
            h: (safe.h * 0.018).max(6.0),
        };
        p.rect(bar, [1.0, 1.0, 1.0, 0.18]);
        p.rect(
            Rect {
                w: bar.w * self.progress.clamp(0.0, 1.0),
                ..bar
            },
            rgb(0xffc800),
        );
        if ctx.content.is_deferred() {
            draw_back_button(&mut p, back_button_rect(&ctx.layout), self.back.down());
        }
    }
}

impl Game for StageLoad {
    fn name(&self) -> &'static str {
        "loading"
    }

    fn on_enter(&mut self, ctx: &mut GameCtx) -> Result<(), String> {
        // What this load will read, given what earlier loads left in the cache and what is playing.
        self.wanted = self.shared.cache.borrow().wanted(&self.files);
        if self.audio.needs_music_files(self.request.bg) {
            self.wanted.extend(self.files.music_wanted());
        }
        ctx.renderer.clear_color = BACKGROUND.map(f64::from);
        Ok(())
    }

    fn update(&mut self, ctx: &mut GameCtx) -> Transition {
        ctx.input.request_virtual_controls(false);
        let stepwise = ctx.content.is_deferred();
        let back = self
            .back
            .update(ctx.input, back_button_rect(&ctx.layout), ctx.dt);
        if stepwise && (back || ctx.input.pressed(Action::Cancel)) {
            return self.end(ctx, LoadOutcome::Cancelled);
        }
        loop {
            match std::mem::replace(&mut self.phase, Phase::Fetch) {
                Phase::Fetch => match ctx.content.prepare(&self.wanted) {
                    Prepare::Pending { done, total } => {
                        self.progress = 0.6 * done as f32 / total.max(1) as f32;
                        break;
                    }
                    Prepare::Failed(e) => return self.fail(ctx, e),
                    Prepare::Ready => {
                        self.progress = 0.6;
                        self.phase = Phase::Parse;
                    }
                },
                Phase::Parse => {
                    ctx.content.install_trig_tables();
                    let mut timings = LoadTimings::default();
                    let loaded = StageAssets::load_timed(
                        ctx.content,
                        &self.files,
                        &mut self.shared.cache.borrow_mut(),
                        &mut timings,
                    );
                    match loaded {
                        Ok(assets) => {
                            self.progress = 0.75;
                            self.phase = Phase::Music(Box::new((assets, timings)));
                        }
                        Err(e) => return self.fail(ctx, e),
                    }
                }
                Phase::Music(loaded) => {
                    // The same track as on the previous floor just keeps playing; a new one is decoded
                    // here and starts while the scene is built.
                    self.audio
                        .stage_music(self.request.bg, &mut |path| ctx.content.read_once(path));
                    self.progress = 0.85;
                    self.phase = Phase::Build(loaded);
                }
                Phase::Build(loaded) => {
                    let (assets, timings) = *loaded;
                    // The options are read when a stage is built: a change shows with the next load.
                    let reflections = self.request.settings.reflections;
                    let scene = PlayScene::build(ctx.renderer, assets, timings, reflections);
                    // Everything is parsed and on the GPU: the raw files are not needed any more.
                    ctx.content.release(&self.wanted);
                    let play = PlayState::new(
                        self.request.clone(),
                        self.ui.clone(),
                        self.audio.clone(),
                        scene,
                        Some(self.shared.park.clone()),
                    );
                    return Transition::Replace(Box::new(play.with_course(self.course.take())));
                }
            }
            if stepwise {
                break;
            }
        }
        self.draw(ctx);
        Transition::None
    }

    fn camera(&self) -> Camera {
        Camera::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y)
    }

    fn status(&self) -> String {
        format!("loading stage {}", self.request.stage_id)
    }

    fn nav_depth(&self) -> u32 {
        1
    }
}
