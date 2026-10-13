//! Title menu, course / stage selection, options, and the course flow around the gameplay state.
//!
//! This is the root state. It owns the save profile and the `mb_game::CourseRun` of the run in progress;
//! to play a floor it pushes a [`PlayState`] built from the run's `PlayRequest` and, when that state pops,
//! reads the `PlayResult` from the shared report slot, applies it to the run (lives, score, floor skipping,
//! unlocks), saves, and either pushes the next floor or shows game over / course complete.
//!
//! Every screen is a title, a few info lines and one list of big rows (`widgets`), so keys, gamepad, mouse
//! and touch all work the same way everywhere. Every screen but the title has a Back button in the top-left
//! corner; long lists scroll by dragging, by the wheel or with the page arrows beside them.
//!
//! Stages are started through `loading::StageLoad`, which shows progress and comes back here with a
//! message when a stage cannot be loaded (shown as a notice; the session goes on). A stage that ends leaves
//! its scene parked: when the run's next step is a retry of the same floor the scene is played again as it
//! is, otherwise it is unloaded before the next screen or stage.

use std::rc::Rc;

use glam::Vec3;
use mb_game::course::stage_list;
use mb_game::progress::ControlMode;
use mb_game::run::{Floor, Step};
use mb_game::stages::{bg_name, DEFAULT_STAGE_TIME};
use mb_game::{
    CourseRun, Difficulty, PlayMode, PlayReport, PlayRequest, PlayResult, Profile, Storage,
};
use mb_render::Camera;

use crate::audio::{Audio, Sfx};
use crate::game::{Game, GameCtx, Transition};
use crate::host;
use crate::input::Action;
use crate::loading::{LoadOutcome, StageLoad, StageShared};
use crate::play::PlayState;
use crate::play_scene::PlayScene;
use crate::text::{rgb, Align, Color, FontId, Painter, UiAssets, WHITE};
use crate::ui::Rect;
use crate::widgets::{
    back_button_rect, draw_back_button, draw_list, draw_rail, fit_scale, fit_scale_lines,
    ListLayout, ListState, Row, Tap,
};

const BACKGROUND: [f64; 4] = [0.02, 0.07, 0.24, 1.0];
/// The same colour for the rectangles that back the menu and cover rows scrolled out of the list.
const BACKDROP: Color = [0.02, 0.07, 0.24, 1.0];
const HEADER_BAND: Color = [0.0, 0.02, 0.1, 0.55];
const TITLE_COLOR: Color = rgb(0xffc800);
const INFO_COLOR: Color = rgb(0xd8e6ff);
const GOOD_COLOR: Color = rgb(0x60ff60);
const NOTICE_COLOR: Color = rgb(0xff8c60);
/// A notice is wrapped to lines of about this many characters, at most [`NOTICE_LINES`] of them.
const NOTICE_WIDTH: usize = 30;
const NOTICE_LINES: usize = 4;
/// Storage lines on the options screen (they hold paths) are wrapped to this many characters.
const INFO_WIDTH: usize = 46;

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

/// A volume setting as shown in the options.
fn volume_label(percent: u8) -> String {
    if percent == 0 {
        "OFF".into()
    } else {
        format!("{percent}")
    }
}

/// Selecting a volume row steps it by ten and wraps from 100 to off.
fn next_volume(percent: u8) -> u8 {
    if percent >= 100 {
        0
    } else {
        (percent / 10 + 1) * 10
    }
}

const MAIN_COURSES: [Difficulty; 7] = Difficulty::ALL;

/// The rows of the options screen. Which ones exist depends on the platform ([`host::caps`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OptionRow {
    UnlockAll,
    StickCurve,
    Music,
    Sound,
    Announcer,
    Controls,
    TiltRange,
    Fullscreen,
    Reflections,
    RemoveData,
    ForgetImage,
    EraseSaves,
    Back,
}

/// Break `text` into lines of at most `width` characters at spaces (longer words are cut).
fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let mut word = word;
        loop {
            let fits = lines
                .last()
                .is_some_and(|l| l.chars().count() + 1 + word.chars().count() <= width);
            if fits {
                let last = lines.last_mut().unwrap();
                last.push(' ');
                last.push_str(word);
                break;
            }
            if word.chars().count() <= width {
                lines.push(word.to_string());
                break;
            }
            let cut = word
                .char_indices()
                .nth(width)
                .map_or(word.len(), |(i, _)| i);
            lines.push(word[..cut].to_string());
            word = &word[cut..];
        }
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            last.push_str("...");
        }
    }
    lines
}

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
    audio: Audio,
    /// Shared with the loading and gameplay states: the parked scene, the parsed-file cache and the
    /// outcome of a load that did not reach its stage.
    shared: StageShared,
    /// The scene of the stage that just ended, for this frame only: a retry takes it, anything else
    /// unloads it.
    parked: Option<PlayScene>,
    back_tap: Tap,
    /// Scroll the list to its selection at the next layout (a screen just opened).
    reveal: bool,
    /// A destructive options row was chosen once; choosing the same row again does it.
    confirm: Option<OptionRow>,
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
            // A page is left through the browser, not from a menu entry.
            allow_quit: cfg!(not(target_arch = "wasm32")),
            texture_base: 0,
            audio: Audio::silent(),
            shared: StageShared::default(),
            parked: None,
            back_tap: Tap::default(),
            reveal: true,
            confirm: None,
        }
    }

    /// What this menu shares with the stages it starts (tests look at the cache and the parked scene).
    pub fn shared(&self) -> &StageShared {
        &self.shared
    }

    /// Tell the platform how the player wants to steer.
    fn apply_controls(&self) {
        let s = &self.profile.settings;
        host::post(host::Command::Tilt {
            enabled: host::caps().tilt && s.control_mode == ControlMode::Tilt,
            range_deg: f32::from(s.tilt_range_deg),
        });
    }

    fn option_rows(&self) -> Vec<OptionRow> {
        let caps = host::caps();
        let mut rows = vec![
            OptionRow::UnlockAll,
            OptionRow::StickCurve,
            OptionRow::Music,
            OptionRow::Sound,
            OptionRow::Announcer,
        ];
        if caps.tilt {
            rows.push(OptionRow::Controls);
            if self.profile.settings.control_mode == ControlMode::Tilt {
                rows.push(OptionRow::TiltRange);
            }
        }
        if caps.fullscreen {
            rows.push(OptionRow::Fullscreen);
        }
        rows.push(OptionRow::Reflections);
        if caps.remove_data {
            rows.push(OptionRow::RemoveData);
        }
        if caps.forget_image {
            rows.push(OptionRow::ForgetImage);
        }
        rows.push(OptionRow::EraseSaves);
        rows.push(OptionRow::Back);
        rows
    }

    /// Every screen but the title can be left with Back.
    fn has_back(&self) -> bool {
        self.screen != Screen::Title
    }

    /// Give the game a voice (the default is silence). The platform driver keeps a clone for
    /// suspend / resume.
    pub fn with_audio(mut self, audio: Audio) -> Self {
        self.audio = audio;
        self
    }

    fn apply_volumes(&self) {
        let s = &self.profile.settings;
        self.audio
            .set_volumes(s.music_volume, s.sfx_volume, s.announcer_volume);
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
        self.reveal = true;
        self.confirm = None;
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

    /// Info lines under the title: the screen's own, then the notice (a save or load problem).
    fn info(&self) -> Vec<(String, Color)> {
        let mut lines = self.screen_info();
        if let Some(notice) = &self.notice {
            let wrapped = wrap(notice, NOTICE_WIDTH, NOTICE_LINES);
            lines.extend(wrapped.into_iter().map(|l| (l, NOTICE_COLOR)));
        }
        lines
    }

    fn screen_info(&self) -> Vec<(String, Color)> {
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
            // What is kept on this device and where, so it can be found and removed.
            Screen::Options => host::info()
                .iter()
                .flat_map(|line| wrap(&line.to_uppercase(), INFO_WIDTH, 2))
                .map(|line| (line, INFO_COLOR))
                .collect(),
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
                let on_off = |on: bool| if on { "ON" } else { "OFF" };
                self.option_rows()
                    .into_iter()
                    .map(|r| match r {
                        OptionRow::UnlockAll => {
                            row_with("UNLOCK ALL STAGES", on_off(s.unlock_all), true)
                        }
                        OptionRow::StickCurve => row_with(
                            "STICK CURVE",
                            format!("{}.{:02}", s.input_falloff / 100, s.input_falloff % 100),
                            true,
                        ),
                        OptionRow::Music => {
                            row_with("MUSIC VOLUME", volume_label(s.music_volume), true)
                        }
                        OptionRow::Sound => {
                            row_with("SOUND VOLUME", volume_label(s.sfx_volume), true)
                        }
                        OptionRow::Announcer => {
                            row_with("ANNOUNCER VOLUME", volume_label(s.announcer_volume), true)
                        }
                        OptionRow::Controls => row_with(
                            "CONTROLS",
                            match s.control_mode {
                                ControlMode::Stick => "STICK",
                                ControlMode::Tilt => "TILT",
                            },
                            true,
                        ),
                        OptionRow::TiltRange => {
                            row_with("TILT RANGE", format!("{} DEG", s.tilt_range_deg), true)
                        }
                        OptionRow::Fullscreen => {
                            row_with("FULLSCREEN", on_off(host::is_fullscreen()), true)
                        }
                        OptionRow::Reflections => {
                            row_with("REFLECTIONS", on_off(s.reflections), true)
                        }
                        r if self.confirm == Some(r) => row("TAP AGAIN TO CONFIRM"),
                        OptionRow::RemoveData => row("REMOVE GAME DATA"),
                        OptionRow::ForgetImage => row("FORGET GAME IMAGE"),
                        OptionRow::EraseSaves => row("ERASE SAVE DATA"),
                        OptionRow::Back => row("BACK"),
                    })
                    .collect()
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

    /// Start `request`: on the scene the previous attempt left behind when this is a retry of the same
    /// stage (nothing is read or built again), otherwise through the loading state.
    fn push_play(&mut self, request: PlayRequest) -> Transition {
        let Some(ui) = self.ui.clone() else {
            return Transition::None;
        };
        // In course play the goal banner announces how far a green or red goal jumps: the gameplay
        // state gets the course as it stands at this floor (`PlayRequest` has no room for it).
        let course = match &self.active {
            Some(Active::Run(run)) if run.mode() == PlayMode::Course => Some(run.course().clone()),
            _ => None,
        };
        let same_stage = |s: &PlayScene| s.stage_id() == request.stage_id;
        if request.retry && self.parked.as_ref().is_some_and(same_stage) {
            if let Some(scene) = self.parked.take() {
                let park = Some(self.shared.park.clone());
                let play = PlayState::new(request, ui, self.audio.clone(), scene, park);
                return Transition::Push(Box::new(play.with_course(course)));
            }
        }
        let load = StageLoad::new(request, ui, self.audio.clone(), self.shared.clone());
        Transition::Push(Box::new(load.with_course(course)))
    }

    /// A stage load ended without a stage: back to where it was started from, with the reason if it failed.
    fn on_load_ended(&mut self, outcome: LoadOutcome) {
        match self.active.take() {
            Some(Active::Run(run)) if run.mode() == PlayMode::Practice => {
                let index = run.floor().floor.saturating_sub(1) as usize;
                self.go(Screen::StageSelect(run.difficulty()), index);
            }
            _ => self.go(Screen::Title, 0),
        }
        if let LoadOutcome::Failed(reason) = outcome {
            self.notice = Some(format!("COULD NOT LOAD {reason}").to_uppercase());
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
                    // An id the game does not offer (no background of its own): stay on the title.
                    None if !mb_game::stages::is_playable_stage(id) => Transition::None,
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
                let chosen = self.option_rows().get(index).copied();
                let confirmed = self.confirm.take();
                if let Some(
                    r @ (OptionRow::RemoveData | OptionRow::ForgetImage | OptionRow::EraseSaves),
                ) = chosen
                {
                    // Deleting wants a second tap on the same row.
                    if confirmed != Some(r) {
                        self.confirm = Some(r);
                        return Transition::None;
                    }
                    match r {
                        OptionRow::RemoveData => host::post(host::Command::RemoveGameData),
                        OptionRow::ForgetImage => {
                            self.notice =
                                Some(match self.storage.remove(crate::content::LAST_DISC_KEY) {
                                    Ok(()) => {
                                        "THE GAME IMAGE WILL BE ASKED FOR AT THE NEXT START".into()
                                    }
                                    Err(e) => {
                                        format!("COULD NOT FORGET THE IMAGE: {e}").to_uppercase()
                                    }
                                });
                        }
                        _ => {
                            let erased = (self.storage.remove(mb_game::progress::PROFILE_KEY))
                                .and(self.storage.remove(mb_game::progress::BACKUP_KEY));
                            self.profile = Profile::default();
                            self.apply_volumes();
                            self.apply_controls();
                            self.notice = Some(match erased {
                                Ok(()) => "SAVE DATA ERASED".into(),
                                Err(e) => format!("COULD NOT ERASE: {e}").to_uppercase(),
                            });
                        }
                    }
                    return Transition::None;
                }
                let s = &mut self.profile.settings;
                match chosen {
                    Some(OptionRow::UnlockAll) => s.unlock_all = !s.unlock_all,
                    Some(OptionRow::StickCurve) => {
                        s.input_falloff = if s.input_falloff >= 200 {
                            100
                        } else {
                            s.input_falloff + 25
                        }
                    }
                    Some(OptionRow::Music) => s.music_volume = next_volume(s.music_volume),
                    Some(OptionRow::Sound) => s.sfx_volume = next_volume(s.sfx_volume),
                    Some(OptionRow::Announcer) => {
                        s.announcer_volume = next_volume(s.announcer_volume)
                    }
                    Some(OptionRow::Controls) => {
                        s.control_mode = match s.control_mode {
                            ControlMode::Stick => ControlMode::Tilt,
                            ControlMode::Tilt => ControlMode::Stick,
                        }
                    }
                    Some(OptionRow::TiltRange) => {
                        s.tilt_range_deg = if s.tilt_range_deg >= 25 {
                            10
                        } else {
                            s.tilt_range_deg + 5
                        }
                    }
                    Some(OptionRow::Fullscreen) => {
                        host::post(host::Command::SetFullscreen(!host::is_fullscreen()));
                        return Transition::None;
                    }
                    // Read when a stage is built (`loading.rs`); no stage is loaded while the options
                    // are open, so the next one played has it.
                    Some(OptionRow::Reflections) => s.reflections = !s.reflections,
                    Some(
                        OptionRow::Back
                        | OptionRow::RemoveData
                        | OptionRow::ForgetImage
                        | OptionRow::EraseSaves,
                    )
                    | None => {
                        self.go(Screen::Title, 2);
                        return Transition::None;
                    }
                }
                s.clamp();
                self.save();
                // Applied at once; a sample of the changed category lets the level be heard.
                self.apply_volumes();
                self.apply_controls();
                match chosen {
                    Some(OptionRow::Sound) => self.audio.play(Sfx::Banana),
                    Some(OptionRow::Announcer) => self.audio.play(Sfx::Ready),
                    _ => {}
                }
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
        let (width, height) = (ctx.size.0 as f32, ctx.size.1 as f32);
        let back = back_button_rect(&ctx.layout);
        let mut p = Painter {
            r: ctx.renderer,
            ui: &ui,
        };
        let full = |y: f32, h: f32| Rect {
            x: 0.0,
            y,
            w: width,
            h,
        };
        p.rect(full(0.0, height), BACKDROP);
        // The rows first, including ones that are only partly inside the list's window; what sticks out
        // above and below is then painted over, and the header and footer are drawn on top of that.
        draw_list(
            &mut p,
            layout,
            &self.list,
            &|i| Row {
                label: &rows[i].label,
                detail: rows[i].detail.as_deref().filter(|d| !d.is_empty()),
                enabled: rows[i].enabled,
            },
            true,
        );
        let list_bottom = layout.area.y + layout.area.h;
        p.rect(full(0.0, layout.area.y), BACKDROP);
        p.rect(full(list_bottom, height - list_bottom), BACKDROP);
        // Header band.
        p.rect(full(0.0, safe.y + safe.h * title_h * 0.96), HEADER_BAND);
        draw_rail(&mut p, layout, &self.list);
        if self.has_back() {
            draw_back_button(&mut p, back, self.back_tap.down());
        }
        let info = self.info();
        let title = self.title();
        let line_h = safe.h * 0.055;
        let title_box = safe.h * title_h - info.len() as f32 * line_h - safe.h * 0.05;
        let title_y = safe.y + safe.h * 0.03 + title_box / 2.0;
        // The title is centred; with a Back button it keeps clear of it (and of its mirror image).
        let title_w = if self.has_back() {
            (safe.w - 2.0 * (back.x - safe.x + back.w) - back.h * 0.4).max(safe.w * 0.3)
        } else {
            safe.w * 0.78
        };
        let s = fit_scale(
            &p,
            FontId::Asc72,
            &title,
            title_w,
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
        // One size for all of them: they are the lines of wrapped messages.
        let lines = info.iter().map(|(line, _)| line.as_str());
        let s = fit_scale_lines(&p, FontId::Asc32, lines, safe.w * 0.9, line_h * 0.8);
        for (line, color) in &info {
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
        let footer = (!ctx.input.touch_active())
            .then_some(("ARROWS / STICK: SELECT   ENTER: OK   ESC: BACK", INFO_COLOR));
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
        // The stages run on the game's own trig tables, read from the executable.
        ctx.content.install_trig_tables();
        // Sound effects are read and decoded here, once, before anything is played.
        let dol = ctx.content.main_dol();
        self.audio
            .ensure_sfx(&mut |path| ctx.content.read(path), dol.as_deref());
        self.apply_volumes();
        self.apply_controls();
        // The start-up files are parsed and decoded now; where they were fetched into memory, free it.
        ctx.content.release(&crate::content::boot_files());
        Ok(())
    }

    fn on_exit(&mut self, ctx: &mut GameCtx) {
        let parked = self.parked.take();
        let left = self.shared.park.borrow_mut().take();
        for scene in parked.into_iter().chain(left) {
            scene.unload(ctx.renderer);
        }
        self.shared.cache.borrow_mut().clear();
        if self.ui.take().is_some() {
            ctx.renderer.truncate_textures(self.texture_base);
        }
    }

    fn update(&mut self, ctx: &mut GameCtx) -> Transition {
        ctx.input.request_virtual_controls(false);
        ctx.renderer.clear_color = BACKGROUND;

        // The scene of a stage that ended this frame: a retry of that stage plays on it.
        self.parked = self.shared.park.borrow_mut().take();
        let mut transition = Transition::None;
        let load_ended = self.shared.outcome.borrow_mut().take();
        if let Some(outcome) = load_ended {
            self.on_load_ended(outcome);
        } else if let Some(auto) = self.autostart.take() {
            transition = self.start_auto(auto);
        } else if let Some(result) = self.report.take() {
            transition = self.on_result(result);
        }
        // Not taken by a retry: the stage is over, free it before anything else is shown or loaded.
        if let Some(scene) = self.parked.take() {
            scene.unload(ctx.renderer);
        }
        if !matches!(transition, Transition::None) {
            return transition;
        }
        // A menu screen is showing (no stage follows this frame): stage music ends here.
        self.audio.stop_music();

        let rows = self.rows();
        let title_h = 0.24 + self.info().len() as f32 * 0.055;
        let layout = ListLayout::column(&ctx.layout, rows.len(), title_h);
        if std::mem::take(&mut self.reveal) {
            self.list.reveal(&layout);
        }
        let enabled = |i: usize| rows.get(i).is_some_and(|r| r.enabled);
        let before = self.list.selected;
        let back = self.has_back()
            && self
                .back_tap
                .update(ctx.input, back_button_rect(&ctx.layout), ctx.dt);
        if back || ctx.input.pressed(Action::Cancel) {
            self.notice = None;
            self.confirm = None;
            self.audio.play(Sfx::MenuCancel);
            transition = self.back();
        } else if let Some(i) = self.list.update(ctx.input, &layout, &enabled, ctx.dt) {
            self.notice = None;
            self.audio.play(Sfx::MenuConfirm);
            transition = self.activate(i);
        } else if self.list.selected != before {
            self.audio.play(Sfx::MenuMove);
        }
        if !matches!(transition, Transition::None) {
            return transition;
        }
        // The screen may have changed: lay out and draw what is current now.
        let rows = self.rows();
        let title_h = 0.24 + self.info().len() as f32 * 0.055;
        let layout = ListLayout::column(&ctx.layout, rows.len(), title_h);
        self.list.selected = self.list.selected.min(rows.len().saturating_sub(1));
        if std::mem::take(&mut self.reveal) {
            self.list.reveal(&layout);
        }
        self.draw(ctx, &layout, &rows, title_h);
        self.audio.flush();
        Transition::None
    }

    fn camera(&self) -> Camera {
        Camera::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y)
    }

    fn status(&self) -> String {
        format!("menu: {}", self.screen_name())
    }

    fn nav_depth(&self) -> u32 {
        self.has_back() as u32
    }

    fn notice(&self) -> Option<String> {
        self.notice.clone()
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
        // Volumes step by ten and wrap to off; the defaults are the reference's 50 / 30 / 30.
        m.activate(2);
        m.activate(3);
        for _ in 0..8 {
            m.activate(4);
        }
        let s = &m.profile.settings;
        assert_eq!(
            (s.music_volume, s.sfx_volume, s.announcer_volume),
            (60, 40, 0)
        );
        let (saved, _) = Profile::load(m.storage.as_mut());
        assert_eq!(saved.settings.announcer_volume, 0);
        assert_eq!(volume_label(0), "OFF");
        assert_eq!(next_volume(95), 100);
        // "Erase save data" asks first: one tap arms it, moving on disarms it, two taps do it.
        let labels = |m: &Menu| -> Vec<String> { m.rows().into_iter().map(|r| r.label).collect() };
        assert_eq!(labels(&m)[5..], ["REFLECTIONS", "ERASE SAVE DATA", "BACK"]);
        m.activate(6);
        assert_eq!(labels(&m)[6], "TAP AGAIN TO CONFIRM");
        assert!(m.profile.settings.unlock_all, "nothing erased yet");
        m.activate(2);
        assert_eq!(labels(&m)[6], "ERASE SAVE DATA");
        m.activate(6);
        m.activate(6);
        assert_eq!(m.profile, Profile::default());
        assert_eq!(m.notice.as_deref(), Some("SAVE DATA ERASED"));
        let (saved, warning) = Profile::load(m.storage.as_mut());
        assert_eq!((saved, warning), (Profile::default(), None));
        m.activate(7);
        assert_eq!(m.screen_name(), "title");
    }

    /// Rows that only some platforms have appear when the platform says so, and the options screen says
    /// where data is kept.
    #[test]
    fn platform_rows_and_storage_info_follow_the_host() {
        let mut m = menu();
        m.go(Screen::Options, 0);
        host::set_caps(host::Caps {
            remove_data: true,
            fullscreen: true,
            tilt: true,
            forget_image: true,
        });
        host::set_info(vec!["Game data: 137 MB in this browser".into()]);
        let labels: Vec<String> = m.rows().into_iter().map(|r| r.label).collect();
        assert_eq!(
            labels[5..],
            [
                "CONTROLS",
                "FULLSCREEN",
                "REFLECTIONS",
                "REMOVE GAME DATA",
                "FORGET GAME IMAGE",
                "ERASE SAVE DATA",
                "BACK"
            ]
        );
        assert_eq!(m.info()[0].0, "GAME DATA: 137 MB IN THIS BROWSER");
        // Tilt adds its range row; removing data and forgetting the image want a second tap.
        m.activate(5);
        assert_eq!(m.rows()[6].label, "TILT RANGE");
        m.activate(9);
        assert!(host::drain()
            .iter()
            .all(|c| !matches!(c, host::Command::RemoveGameData)));
        m.activate(9);
        assert!(host::drain().contains(&host::Command::RemoveGameData));
        m.storage
            .write(crate::content::LAST_DISC_KEY, b"C:/x.rvz")
            .unwrap();
        m.activate(10);
        m.activate(10);
        assert_eq!(m.storage.read(crate::content::LAST_DISC_KEY).unwrap(), None);
        assert!(m.notice.as_deref().unwrap().contains("NEXT START"));
        host::set_caps(host::Caps::default());
        host::set_info(Vec::new());
    }

    /// Reflections are on until the row is chosen, and the choice is saved with the profile, reaches
    /// the next stage's request and is still there when the menu is started again on the same save.
    #[test]
    fn the_reflections_row_toggles_saves_and_reaches_the_next_stage() {
        let mut m = menu();
        m.go(Screen::Options, 0);
        let index = |m: &Menu| {
            let rows = m.option_rows();
            rows.iter().position(|r| *r == OptionRow::Reflections)
        };
        let at = index(&m).expect("the row is there on every platform");
        assert_eq!(at, 5, "after the sound rows, before the storage rows");
        let detail = |m: &Menu| m.rows()[at].detail.clone();
        assert_eq!(m.rows()[at].label, "REFLECTIONS");
        assert_eq!(detail(&m).as_deref(), Some("ON"));
        assert!(m.rows()[at].enabled && m.profile.settings.reflections);

        assert!(matches!(m.activate(at), Transition::None));
        assert_eq!(detail(&m).as_deref(), Some("OFF"));
        assert_eq!(m.screen_name(), "options");
        let (saved, warning) = Profile::load(m.storage.as_mut());
        assert!(!saved.settings.reflections && warning.is_none());
        // What a stage started now is told.
        let floor = CourseRun::new(Difficulty::Beginner).floor();
        let request = floor.request(&m.profile.settings, &m.report);
        assert!(!request.settings.reflections);

        // A new menu on the same save (a restart) reads it back off, and the row turns it on again.
        let mut again = Menu::new(std::mem::replace(
            &mut m.storage,
            Box::new(MemoryStorage::new()),
        ));
        again.profile = Profile::load(again.storage.as_mut()).0;
        again.go(Screen::Options, 0);
        assert_eq!(detail(&again).as_deref(), Some("OFF"));
        again.activate(at);
        assert_eq!(detail(&again).as_deref(), Some("ON"));
        assert!(Profile::load(again.storage.as_mut()).0.settings.reflections);
        // The platform rows come before it, so the row a browser test counts down to stays where it is.
        host::set_caps(host::Caps {
            tilt: true,
            fullscreen: true,
            ..host::Caps::default()
        });
        let rows = again.option_rows();
        assert_eq!(rows[5], OptionRow::Controls);
        assert_eq!(index(&again), Some(7));
        host::set_caps(host::Caps::default());
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
