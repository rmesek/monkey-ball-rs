//! Title menu, course / stage selection, options, and the course flow around the gameplay state.
//!
//! This is the root state. It owns the save profile and the `mb_game::CourseRun` of the run in progress;
//! to play a floor it pushes a [`PlayState`] built from the run's `PlayRequest` and, when that state pops,
//! reads the `PlayResult` from the shared report slot, applies it to the run (lives, score, floor skipping,
//! unlocks), saves, and either pushes the next floor or shows game over / course complete.
//!
//! Every screen is a title, a few info lines and one list of big rows (`widgets`), so keys, gamepad, mouse
//! and touch all work the same way everywhere.

use std::rc::Rc;

use glam::Vec3;
use mb_game::course::stage_list;
use mb_game::run::{Floor, Step};
use mb_game::stages::{bg_name, DEFAULT_STAGE_TIME};
use mb_game::{
    CourseRun, Difficulty, PlayMode, PlayReport, PlayRequest, PlayResult, Profile, Storage,
};
use mb_render::Camera;

use crate::game::{Game, GameCtx, Transition};
use crate::input::Action;
use crate::play::PlayState;
use crate::text::{rgb, Align, Color, FontId, Painter, UiAssets, WHITE};
use crate::ui::Rect;
use crate::widgets::{draw_list, fit_scale, ListLayout, ListState, Row};

const BACKGROUND: [f64; 4] = [0.02, 0.07, 0.24, 1.0];
const TITLE_COLOR: Color = rgb(0xffc800);
const INFO_COLOR: Color = rgb(0xd8e6ff);
const GOOD_COLOR: Color = rgb(0x60ff60);

/// Jump straight into a stage or course when the menu opens (developer command line, autoplay).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Autostart {
    /// Practice on this stage id.
    Stage(u32),
    Course(Difficulty),
}

#[derive(Clone, Debug, PartialEq)]
enum Screen {
    Title,
    CourseSelect,
    PracticeCourse,
    StageSelect(Difficulty),
    Options,
    GameOver {
        score: u64,
        continues_left: u32,
    },
    CourseComplete {
        difficulty: Difficulty,
        score: u64,
        new_best_score: bool,
        unlocked: Option<Difficulty>,
    },
    PracticeClear {
        frames: u32,
        new_best: bool,
    },
}

/// What is being played while a [`PlayState`] is on top.
enum Active {
    Run(Box<CourseRun>),
    /// A stage outside every course (developer command line): no rules, just replay.
    Direct(Box<PlayRequest>),
}

struct RowData {
    label: String,
    detail: Option<String>,
    enabled: bool,
}

fn row(label: impl Into<String>) -> RowData {
    RowData {
        label: label.into(),
        detail: None,
        enabled: true,
    }
}

fn row_with(label: impl Into<String>, detail: impl Into<String>, enabled: bool) -> RowData {
    RowData {
        label: label.into(),
        detail: Some(detail.into()),
        enabled,
    }
}

/// `12.34` seconds from frames.
fn format_time(frames: u32) -> String {
    format!("{}.{:02}", frames / 60, (frames % 60) * 100 / 60)
}

const MAIN_COURSES: [Difficulty; 7] = Difficulty::ALL;

pub struct Menu {
    storage: Box<dyn Storage>,
    profile: Profile,
    ui: Option<Rc<UiAssets>>,
    screen: Screen,
    list: ListState,
    active: Option<Active>,
    report: PlayReport,
    notice: Option<String>,
    autostart: Option<Autostart>,
    allow_quit: bool,
    /// Texture count before the UI textures were uploaded (to free them when the menu itself is left).
    texture_base: usize,
}

impl Menu {
    pub fn new(storage: Box<dyn Storage>) -> Self {
        Self {
            storage,
            profile: Profile::default(),
            ui: None,
            screen: Screen::Title,
            list: ListState::default(),
            active: None,
            report: PlayReport::new(),
            notice: None,
            autostart: None,
            // A page or an Android activity is left through the system, not from a menu entry.
            allow_quit: cfg!(not(any(target_arch = "wasm32", target_os = "android"))),
            texture_base: 0,
        }
    }

    pub fn with_autostart(mut self, autostart: Option<Autostart>) -> Self {
        self.autostart = autostart;
        self
    }

    /// The run in progress (tests).
    pub fn run(&self) -> Option<&CourseRun> {
        match &self.active {
            Some(Active::Run(run)) => Some(run),
            _ => None,
        }
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// Name of the screen shown (tests, window title).
    pub fn screen_name(&self) -> &'static str {
        match self.screen {
            Screen::Title => "title",
            Screen::CourseSelect => "course select",
            Screen::PracticeCourse => "practice",
            Screen::StageSelect(_) => "stage select",
            Screen::Options => "options",
            Screen::GameOver { .. } => "game over",
            Screen::CourseComplete { .. } => "course complete",
            Screen::PracticeClear { .. } => "practice clear",
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.profile.save(self.storage.as_mut()) {
            self.notice = Some(format!("COULD NOT SAVE: {e}"));
        }
    }

    fn go(&mut self, screen: Screen, selected: usize) {
        self.screen = screen;
        self.list = ListState::new(selected);
    }

    fn unlock_all(&self) -> bool {
        self.profile.settings.unlock_all
    }

    fn course_unlocked(&self, d: Difficulty) -> bool {
        self.profile
            .progress
            .is_course_unlocked(d, self.unlock_all())
    }

    fn title(&self) -> String {
        match &self.screen {
            Screen::Title => "MONKEY BALL".into(),
            Screen::CourseSelect => "COURSE PLAY".into(),
            Screen::PracticeCourse => "PRACTICE".into(),
            Screen::StageSelect(d) => d.label().to_uppercase(),
            Screen::Options => "OPTIONS".into(),
            Screen::GameOver { .. } => "GAME OVER".into(),
            Screen::CourseComplete { .. } => "COURSE CLEAR".into(),
            Screen::PracticeClear { .. } => "GOAL".into(),
        }
    }

    /// Info lines under the title.
    fn info(&self) -> Vec<(String, Color)> {
        match &self.screen {
            Screen::GameOver {
                score,
                continues_left,
            } => vec![
                (format!("SCORE {score}"), INFO_COLOR),
                (format!("CONTINUES LEFT {continues_left}"), INFO_COLOR),
            ],
            Screen::CourseComplete {
                difficulty,
                score,
                new_best_score,
                unlocked,
            } => {
                let mut lines = vec![
                    (difficulty.label().to_uppercase(), INFO_COLOR),
                    (format!("SCORE {score}"), INFO_COLOR),
                ];
                if *new_best_score {
                    lines.push(("NEW BEST SCORE".into(), GOOD_COLOR));
                }
                if let Some(u) = unlocked {
                    lines.push((format!("{} UNLOCKED", u.label().to_uppercase()), GOOD_COLOR));
                }
                lines
            }
            Screen::PracticeClear { frames, new_best } => {
                let mut lines = Vec::new();
                if let Some(Active::Run(run)) = &self.active {
                    let f = run.floor();
                    lines.push((format!("FLOOR {} {}", f.floor, f.stage_label), INFO_COLOR));
                }
                lines.push((format!("TIME {}", format_time(*frames)), INFO_COLOR));
                if *new_best {
                    lines.push(("NEW BEST TIME".into(), GOOD_COLOR));
                }
                lines
            }
            Screen::StageSelect(_) => vec![("PICK A STAGE".into(), INFO_COLOR)],
            _ => Vec::new(),
        }
    }

    fn course_rows(&self, practice: bool) -> Vec<RowData> {
        let mut rows: Vec<RowData> = MAIN_COURSES
            .into_iter()
            .map(|d| {
                let c = self.profile.progress.course(d);
                let unlocked = self.course_unlocked(d);
                let detail = if !unlocked {
                    "LOCKED".to_string()
                } else if practice {
                    format!("{} FLOORS", stage_list(d).len())
                } else if c.best_score > 0 {
                    format!("BEST {}", c.best_score)
                } else {
                    String::new()
                };
                row_with(d.label().to_uppercase(), detail, unlocked)
            })
            .collect();
        rows.push(row("BACK"));
        rows
    }

    fn rows(&self) -> Vec<RowData> {
        match &self.screen {
            Screen::Title => {
                let mut rows = vec![row("COURSE PLAY"), row("PRACTICE"), row("OPTIONS")];
                if self.allow_quit {
                    rows.push(row("QUIT"));
                }
                rows
            }
            Screen::CourseSelect => self.course_rows(false),
            Screen::PracticeCourse => self.course_rows(true),
            Screen::StageSelect(d) => {
                let mut rows: Vec<RowData> = stage_list(*d)
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let unlocked =
                            self.profile
                                .progress
                                .is_stage_unlocked(*d, e.id, self.unlock_all());
                        let best = self.profile.progress.stage_best(e.id);
                        let detail = if !unlocked {
                            "LOCKED".to_string()
                        } else if best.best_frames > 0 {
                            format_time(best.best_frames)
                        } else {
                            String::new()
                        };
                        row_with(format!("{:2} {}", i + 1, e.label), detail, unlocked)
                    })
                    .collect();
                rows.push(row("BACK"));
                rows
            }
            Screen::Options => {
                let s = &self.profile.settings;
                vec![
                    row_with(
                        "UNLOCK ALL STAGES",
                        if s.unlock_all { "ON" } else { "OFF" },
                        true,
                    ),
                    row_with(
                        "STICK CURVE",
                        format!("{}.{:02}", s.input_falloff / 100, s.input_falloff % 100),
                        true,
                    ),
                    row("BACK"),
                ]
            }
            Screen::GameOver { continues_left, .. } => vec![
                RowData {
                    label: "CONTINUE".into(),
                    detail: None,
                    enabled: *continues_left > 0,
                },
                row("QUIT"),
            ],
            Screen::CourseComplete { .. } => vec![row("OK")],
            Screen::PracticeClear { .. } => {
                let mut rows = vec![row("RETRY")];
                if self.next_practice_stage().is_some() {
                    rows.push(row("NEXT STAGE"));
                }
                rows.push(row("STAGE SELECT"));
                rows
            }
        }
    }

    /// Index of the stage after the practice stage just cleared, if it is selectable.
    fn next_practice_stage(&self) -> Option<(Difficulty, usize)> {
        let Some(Active::Run(run)) = &self.active else {
            return None;
        };
        let d = run.difficulty();
        let next = run.floor().floor as usize;
        let entry = stage_list(d).into_iter().nth(next)?;
        self.profile
            .progress
            .is_stage_unlocked(d, entry.id, self.unlock_all())
            .then_some((d, next))
    }

    fn push_play(&mut self, request: PlayRequest) -> Transition {
        match &self.ui {
            Some(ui) => Transition::Push(Box::new(PlayState::new(request, ui.clone()))),
            None => Transition::None,
        }
    }

    fn start_floor(&mut self, floor: &Floor) -> Transition {
        self.report = PlayReport::new();
        let request = floor.request(&self.profile.settings, &self.report);
        self.push_play(request)
    }

    fn start_run(&mut self, run: CourseRun) -> Transition {
        let floor = run.floor();
        if run.mode() == PlayMode::Course {
            self.profile
                .progress
                .mark_reached(run.difficulty(), floor.stage_id);
        }
        self.active = Some(Active::Run(Box::new(run)));
        self.start_floor(&floor)
    }

    fn start_direct(&mut self, stage_id: u32, retry: bool) -> Transition {
        self.report = PlayReport::new();
        let request = PlayRequest {
            mode: PlayMode::Practice,
            stage_id,
            bg: bg_name(stage_id).unwrap_or_default(),
            time_limit: DEFAULT_STAGE_TIME,
            retry,
            difficulty: Difficulty::Beginner,
            floor: 1,
            floors: 1,
            stage_label: format!("STAGE {stage_id}"),
            lives: 0,
            score: 0,
            bananas: 0,
            settings: self.profile.settings.clone(),
            report: self.report.clone(),
        };
        self.active = Some(Active::Direct(Box::new(request.clone())));
        self.push_play(request)
    }

    fn start_auto(&mut self, auto: Autostart) -> Transition {
        match auto {
            Autostart::Course(d) => self.start_run(CourseRun::new(d)),
            Autostart::Stage(id) => {
                let found = Difficulty::ALL.into_iter().find_map(|d| {
                    stage_list(d)
                        .iter()
                        .position(|e| e.id == id)
                        .map(|i| (d, i))
                });
                match found {
                    Some((d, i)) => self.start_run(CourseRun::practice(d, i)),
                    None => self.start_direct(id, false),
                }
            }
        }
    }

    /// The gameplay state popped: account for its result and decide what comes next.
    fn on_result(&mut self, result: PlayResult) -> Transition {
        match self.active.take() {
            Some(Active::Run(mut run)) => {
                let outcome = match result {
                    PlayResult::Finished(outcome) => outcome,
                    PlayResult::Quit => {
                        match run.mode() {
                            PlayMode::Course => self.go(Screen::Title, 0),
                            PlayMode::Practice => {
                                let index = run.floor().floor.saturating_sub(1) as usize;
                                self.go(Screen::StageSelect(run.difficulty()), index);
                            }
                        }
                        return Transition::None;
                    }
                };
                let step = run.apply(outcome, &mut self.profile.progress);
                self.save();
                let difficulty = run.difficulty();
                self.active = Some(Active::Run(run));
                match step {
                    Step::Next { floor, .. } | Step::Retry(floor) => self.start_floor(&floor),
                    Step::GameOver {
                        score,
                        continues_left,
                    } => {
                        let selected = if continues_left > 0 { 0 } else { 1 };
                        self.go(
                            Screen::GameOver {
                                score,
                                continues_left,
                            },
                            selected,
                        );
                        Transition::None
                    }
                    Step::CourseComplete {
                        score,
                        new_best_score,
                        unlocked,
                        ..
                    } => {
                        self.active = None;
                        self.go(
                            Screen::CourseComplete {
                                difficulty,
                                score,
                                new_best_score,
                                unlocked,
                            },
                            0,
                        );
                        Transition::None
                    }
                    Step::PracticeClear {
                        frames,
                        new_best_time,
                        ..
                    } => {
                        let selected = if self.next_practice_stage().is_some() {
                            1
                        } else {
                            0
                        };
                        self.go(
                            Screen::PracticeClear {
                                frames,
                                new_best: new_best_time,
                            },
                            selected,
                        );
                        Transition::None
                    }
                }
            }
            Some(Active::Direct(request)) => match result {
                PlayResult::Finished(_) => self.start_direct(request.stage_id, true),
                PlayResult::Quit => {
                    self.go(Screen::Title, 0);
                    Transition::None
                }
            },
            None => Transition::None,
        }
    }

    fn activate(&mut self, index: usize) -> Transition {
        match self.screen.clone() {
            Screen::Title => match index {
                0 => self.go(Screen::CourseSelect, 0),
                1 => self.go(Screen::PracticeCourse, 0),
                2 => self.go(Screen::Options, 0),
                _ => return Transition::Quit,
            },
            Screen::CourseSelect | Screen::PracticeCourse => {
                let practice = self.screen == Screen::PracticeCourse;
                match MAIN_COURSES.get(index) {
                    Some(d) if practice => self.go(Screen::StageSelect(*d), 0),
                    Some(d) => return self.start_run(CourseRun::new(*d)),
                    None => self.go(Screen::Title, if practice { 1 } else { 0 }),
                }
            }
            Screen::StageSelect(d) => {
                if index < stage_list(d).len() {
                    return self.start_run(CourseRun::practice(d, index));
                }
                let back = MAIN_COURSES.iter().position(|c| *c == d).unwrap_or(0);
                self.go(Screen::PracticeCourse, back);
            }
            Screen::Options => {
                let s = &mut self.profile.settings;
                match index {
                    0 => s.unlock_all = !s.unlock_all,
                    1 => {
                        s.input_falloff = if s.input_falloff >= 200 {
                            100
                        } else {
                            s.input_falloff + 25
                        }
                    }
                    _ => {
                        self.go(Screen::Title, 2);
                        return Transition::None;
                    }
                }
                s.clamp();
                self.save();
            }
            Screen::GameOver { .. } => {
                if index == 0 {
                    if let Some(Active::Run(run)) = &mut self.active {
                        if let Some(Step::Retry(floor)) = run.continue_run() {
                            return self.start_floor(&floor);
                        }
                    }
                }
                self.active = None;
                self.go(Screen::Title, 0);
            }
            Screen::CourseComplete { .. } => self.go(Screen::Title, 0),
            Screen::PracticeClear { .. } => {
                let next = self.next_practice_stage();
                let Some(Active::Run(run)) = self.active.take() else {
                    self.go(Screen::Title, 1);
                    return Transition::None;
                };
                let d = run.difficulty();
                let current = run.floor().floor.saturating_sub(1) as usize;
                match (index, next) {
                    (0, _) => {
                        let floor = run.floor();
                        self.active = Some(Active::Run(run));
                        return self.start_floor(&floor);
                    }
                    (1, Some((d, i))) => return self.start_run(CourseRun::practice(d, i)),
                    _ => self.go(Screen::StageSelect(d), current),
                }
            }
        }
        Transition::None
    }

    fn back(&mut self) -> Transition {
        match self.screen.clone() {
            Screen::Title => {
                if self.allow_quit {
                    return Transition::Quit;
                }
            }
            Screen::CourseSelect => self.go(Screen::Title, 0),
            Screen::PracticeCourse => self.go(Screen::Title, 1),
            Screen::Options => self.go(Screen::Title, 2),
            Screen::StageSelect(d) => {
                let back = MAIN_COURSES.iter().position(|c| *c == d).unwrap_or(0);
                self.go(Screen::PracticeCourse, back);
            }
            Screen::GameOver { .. } | Screen::CourseComplete { .. } => {
                self.active = None;
                self.go(Screen::Title, 0);
            }
            Screen::PracticeClear { .. } => {
                let last = self.rows().len().saturating_sub(1);
                return self.activate(last);
            }
        }
        Transition::None
    }

    fn draw(&self, ctx: &mut GameCtx, layout: &ListLayout, rows: &[RowData], title_h: f32) {
        let Some(ui) = self.ui.clone() else {
            return;
        };
        let safe = ctx.layout.safe_rect();
        let scale = ctx.layout.hud_scale();
        let mut p = Painter {
            r: ctx.renderer,
            ui: &ui,
        };
        // Header band.
        p.rect(
            Rect {
                x: 0.0,
                y: 0.0,
                w: ctx.size.0 as f32,
                h: safe.y + safe.h * title_h * 0.96,
            },
            [0.0, 0.02, 0.1, 0.55],
        );
        let info = self.info();
        let title = self.title();
        let line_h = safe.h * 0.055;
        let title_box = safe.h * title_h - info.len() as f32 * line_h - safe.h * 0.05;
        let title_y = safe.y + safe.h * 0.03 + title_box / 2.0;
        let s = fit_scale(
            &p,
            FontId::Asc72,
            &title,
            safe.w * 0.78,
            title_box.min(96.0 * scale),
        );
        p.text(
            FontId::Asc72,
            &title,
            safe.x + safe.w / 2.0,
            title_y,
            s,
            s,
            TITLE_COLOR,
            Align::Center,
        );
        if self.screen == Screen::Title {
            let w = p.measure(FontId::Asc72, &title, s);
            let icon = (title_box * 0.9 / ui.banana_bunch.h).min(1.2 * scale);
            for side in [-1.0, 1.0] {
                let x = safe.x + safe.w / 2.0 + side * (w / 2.0 + ui.banana_bunch.w * icon * 0.6);
                if x > safe.x + ui.banana_bunch.w * icon * 0.5
                    && x < safe.x + safe.w - ui.banana_bunch.w * icon * 0.5
                {
                    p.image(ui.banana_bunch, x, title_y, icon * side, icon, WHITE);
                }
            }
        }
        let mut y = safe.y + safe.h * 0.03 + title_box + line_h * 0.6;
        for (line, color) in &info {
            let s = fit_scale(&p, FontId::Asc32, line, safe.w * 0.9, line_h * 0.8);
            p.text(
                FontId::Asc32,
                line,
                safe.x + safe.w / 2.0,
                y,
                s,
                s,
                *color,
                Align::Center,
            );
            y += line_h;
        }
        draw_list(&mut p, layout, &self.list, &|i| Row {
            label: &rows[i].label,
            detail: rows[i].detail.as_deref().filter(|d| !d.is_empty()),
            enabled: rows[i].enabled,
        });
        let footer = match &self.notice {
            Some(n) => Some((n.as_str(), rgb(0xff8c60))),
            None if !ctx.input.touch_active() => {
                Some(("ARROWS / STICK: SELECT   ENTER: OK   ESC: BACK", INFO_COLOR))
            }
            None => None,
        };
        if let Some((text, color)) = footer {
            let h = safe.h * 0.028;
            let s = fit_scale(&p, FontId::Asc16, text, safe.w * 0.95, h);
            p.text(
                FontId::Asc16,
                text,
                safe.x + safe.w / 2.0,
                safe.y + safe.h - h * 0.8,
                s,
                s,
                color,
                Align::Center,
            );
        }
    }
}

impl Game for Menu {
    fn name(&self) -> &'static str {
        "menu"
    }

    fn on_enter(&mut self, ctx: &mut GameCtx) -> Result<(), String> {
        if self.ui.is_none() {
            // Before any stage, so stage teardown never touches the UI textures.
            self.texture_base = ctx.renderer.texture_count();
            self.ui = Some(Rc::new(UiAssets::load(ctx.renderer, ctx.content)?));
        }
        let (profile, message) = Profile::load(self.storage.as_mut());
        self.profile = profile;
        self.notice = message.map(|m| m.to_uppercase());
        Ok(())
    }

    fn on_exit(&mut self, ctx: &mut GameCtx) {
        if self.ui.take().is_some() {
            ctx.renderer.truncate_textures(self.texture_base);
        }
    }

    fn update(&mut self, ctx: &mut GameCtx) -> Transition {
        ctx.input.request_virtual_controls(false);
        ctx.renderer.clear_color = BACKGROUND;
        ctx.renderer.fog = None;

        let mut transition = Transition::None;
        if let Some(auto) = self.autostart.take() {
            transition = self.start_auto(auto);
        } else if let Some(result) = self.report.take() {
            transition = self.on_result(result);
        }
        if !matches!(transition, Transition::None) {
            return transition;
        }

        let rows = self.rows();
        let title_h = 0.24 + self.info().len() as f32 * 0.055;
        let layout = ListLayout::column(&ctx.layout, rows.len(), title_h);
        let enabled = |i: usize| rows.get(i).is_some_and(|r| r.enabled);
        if let Some(i) = self.list.update(ctx.input, &layout, &enabled) {
            self.notice = None;
            transition = self.activate(i);
        } else if ctx.input.pressed(Action::Cancel) {
            transition = self.back();
        }
        if !matches!(transition, Transition::None) {
            return transition;
        }
        // The screen may have changed: lay out and draw what is current now.
        let rows = self.rows();
        let title_h = 0.24 + self.info().len() as f32 * 0.055;
        let layout = ListLayout::column(&ctx.layout, rows.len(), title_h);
        self.list.selected = self.list.selected.min(rows.len().saturating_sub(1));
        self.list.reveal(&layout);
        self.draw(ctx, &layout, &rows, title_h);
        Transition::None
    }

    fn camera(&self) -> Camera {
        Camera::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y)
    }

    fn status(&self) -> String {
        format!("menu: {}", self.screen_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_game::{GoalType, MemoryStorage, StageOutcome};

    fn menu() -> Menu {
        Menu::new(Box::new(MemoryStorage::new()))
    }

    #[test]
    fn times_are_formatted_in_seconds() {
        assert_eq!(format_time(0), "0.00");
        assert_eq!(format_time(90), "1.50");
        assert_eq!(format_time(3599), "59.98");
        assert_eq!(format_time(60 * 125), "125.00");
    }

    #[test]
    fn title_lists_the_modes() {
        let m = menu();
        let labels: Vec<String> = m.rows().into_iter().map(|r| r.label).collect();
        assert_eq!(&labels[..3], ["COURSE PLAY", "PRACTICE", "OPTIONS"]);
        assert_eq!(m.screen_name(), "title");
    }

    #[test]
    fn extra_courses_are_locked_until_earned() {
        let mut m = menu();
        m.go(Screen::CourseSelect, 0);
        let rows = m.rows();
        assert!(rows[0].enabled && rows[1].enabled && rows[2].enabled);
        assert!(!rows[3].enabled && rows[3].detail.as_deref() == Some("LOCKED"));
        assert_eq!(rows.last().unwrap().label, "BACK");
        m.profile.settings.unlock_all = true;
        assert!(m.rows().iter().all(|r| r.enabled));
    }

    #[test]
    fn practice_lists_every_floor_and_locks_unreached_ones() {
        let mut m = menu();
        m.go(Screen::StageSelect(Difficulty::Beginner), 0);
        let rows = m.rows();
        assert_eq!(rows.len(), stage_list(Difficulty::Beginner).len() + 1);
        assert!(rows[0].enabled && !rows[1].enabled);
        assert!(rows[0].label.starts_with(" 1 "));
    }

    #[test]
    fn options_toggle_and_persist() {
        let mut m = menu();
        m.go(Screen::Options, 0);
        assert!(matches!(m.activate(0), Transition::None));
        assert!(m.profile.settings.unlock_all);
        m.activate(1);
        assert_eq!(m.profile.settings.input_falloff, 125);
        let (saved, _) = Profile::load(m.storage.as_mut());
        assert!(saved.settings.unlock_all && saved.settings.input_falloff == 125);
        m.activate(2);
        assert_eq!(m.screen_name(), "title");
    }

    /// The flow without a renderer: results are applied to the run exactly as when a stage pops.
    #[test]
    fn course_flow_advances_retries_and_ends() {
        let mut m = menu();
        m.active = Some(Active::Run(Box::new(CourseRun::new(Difficulty::Beginner))));
        let goal = PlayResult::Finished(StageOutcome::Goal {
            goal: GoalType::Blue,
            time_left: 3000,
            bananas: 4,
        });
        // No UI assets are loaded, so nothing is pushed; the run state is what matters.
        m.on_result(goal);
        let run = m.run().unwrap();
        assert_eq!(run.floor().floor, 2);
        assert!(run.score() > 0);

        let fall = PlayResult::Finished(StageOutcome::FallOut { bananas: 0 });
        m.on_result(fall);
        let run = m.run().unwrap();
        assert_eq!((run.floor().floor, run.lives()), (2, 2));
        assert!(run.floor().retry);

        m.on_result(fall);
        m.on_result(fall);
        assert_eq!(m.screen_name(), "game over");
        assert!(m.rows()[0].enabled);
        // Continue: lives are back, same floor.
        m.activate(0);
        let run = m.run().unwrap();
        assert_eq!((run.floor().floor, run.lives()), (2, 3));

        m.on_result(PlayResult::Quit);
        assert_eq!(m.screen_name(), "title");
        assert!(m.run().is_none());
    }

    #[test]
    fn practice_clear_offers_retry_and_returns_to_the_list() {
        let mut m = menu();
        m.active = Some(Active::Run(Box::new(CourseRun::practice(
            Difficulty::Beginner,
            0,
        ))));
        m.on_result(PlayResult::Finished(StageOutcome::Goal {
            goal: GoalType::Blue,
            time_left: 3000,
            bananas: 0,
        }));
        assert_eq!(m.screen_name(), "practice clear");
        let labels: Vec<String> = m.rows().into_iter().map(|r| r.label).collect();
        assert_eq!(labels, ["RETRY", "STAGE SELECT"]);
        assert_eq!(m.profile.progress.stage_best(1).best_frames, 600);
        m.back();
        assert_eq!(m.screen_name(), "stage select");
    }

    #[test]
    fn quitting_practice_returns_to_the_stage_list() {
        let mut m = menu();
        m.profile.settings.unlock_all = true;
        m.active = Some(Active::Run(Box::new(CourseRun::practice(
            Difficulty::Advanced,
            4,
        ))));
        m.on_result(PlayResult::Quit);
        assert_eq!(m.screen, Screen::StageSelect(Difficulty::Advanced));
        assert_eq!(m.list.selected, 4);
    }
}
