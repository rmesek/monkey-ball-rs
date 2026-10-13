//! In-game HUD: a port of the reference's `hud.ts` (score, bananas, bomb timer, speed, floor label and the
//! READY / GO / GOAL / FALL OUT / TIME OVER banners with their timings).
//!
//! The reference lays everything out on a centred 640 x 480 canvas. Here every element keeps its 640 x 480
//! coordinates but is anchored to the nearest edge or the centre of the safe area ([`HudView`]), so the HUD
//! hugs the corners on wide, tall and notched screens instead of floating in a 4:3 box.
//!
//! [`Hud::tick`] runs once per simulation tick (the reference advances its banners in 60 Hz frames);
//! [`Hud::draw`] may run at any display rate.

use std::fmt::Write as _;

use mb_game::{Difficulty, GoalType};

use crate::text::{Align, Color, FontId, Painter, WHITE, rgb, with_alpha};
use crate::ui::{Layout, Rect};

const HURRY_UP_FRAMES: i32 = 11 * 60;
const GO_BANNER_FRAMES: i32 = 60;
const GOAL_BANNER_FRAMES: i32 = 360;
const HURRY_UP_BANNER_FRAMES: i32 = 30;
const TIME_OVER_BANNER_FRAMES: i32 = 120;
const BONUS_BANNER_FRAMES: i32 = 90;
const READY_BANNER_FRAMES: i32 = 120;
const FALL_OUT_BANNER_FRAMES: i32 = 270;

const BOMB_FRAG_X: [f32; 10] = [7., 16., 26., 48., 0., 9., 55., 12., 33., 71.];
const BOMB_FRAG_Y: [f32; 10] = [9., 0., 0., 4., 24., 16., 23., 63., 56., 69.];

/// `ball.speed` (units per tick) to miles per hour, the reference's HUD factor.
const SPEED_TO_MPH: f64 = 134.21985;

const YELLOW: Color = rgb(0xffe66e);

/// Horizontal anchor of a HUD element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H {
    Left,
    Center,
    Right,
}

/// Vertical anchor of a HUD element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V {
    Top,
    Center,
    Bottom,
}

/// Maps the reference's 640 x 480 HUD coordinates into the safe area.
#[derive(Clone, Copy, Debug)]
pub struct HudView {
    /// Pixels per HUD unit.
    pub s: f32,
    safe: Rect,
}

impl HudView {
    pub fn new(layout: &Layout) -> Self {
        let safe = layout.safe_rect();
        Self {
            s: (safe.w / 640.0).min(safe.h / 480.0).max(0.05),
            safe,
        }
    }

    pub fn x(&self, vx: f32, h: H) -> f32 {
        match h {
            H::Left => self.safe.x + vx * self.s,
            H::Center => self.safe.x + self.safe.w / 2.0 + (vx - 320.0) * self.s,
            H::Right => self.safe.x + self.safe.w + (vx - 640.0) * self.s,
        }
    }

    pub fn y(&self, vy: f32, v: V) -> f32 {
        match v {
            V::Top => self.safe.y + vy * self.s,
            V::Center => self.safe.y + self.safe.h / 2.0 + (vy - 240.0) * self.s,
            V::Bottom => self.safe.y + self.safe.h + (vy - 480.0) * self.s,
        }
    }
}

/// What the HUD shows this tick, taken from the simulation and the play request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudFrame {
    pub intro_timer: i32,
    pub goal_timer: i32,
    pub ringout_timer: i32,
    pub timeover_timer: i32,
    pub time_limit: i32,
    pub stage_timer: i32,
    pub score: u64,
    /// Banana value collected in this attempt and bananas still on the stage.
    pub bananas: i32,
    pub bananas_left: i32,
    /// `None` hides the counter (practice).
    pub lives: Option<u32>,
    pub speed: f64,
    pub floor: u32,
    pub difficulty: Difficulty,
    pub is_bonus: bool,
    pub is_final: bool,
    /// Colour of the goal the ball went through, once it has.
    pub goal: Option<GoalType>,
    /// All bananas of a bonus stage collected.
    pub bonus_clear: bool,
}

impl HudFrame {
    pub fn time_left(&self) -> i32 {
        self.time_limit - self.stage_timer
    }

    fn floor_prefix(&self) -> &'static str {
        match self.difficulty {
            Difficulty::Master => "MASTER",
            d if d.is_extra() => "EXTRA",
            _ => "FLOOR",
        }
    }

    /// 0 beginner, 1 advanced, 2 expert / master (`difficultyIndex`).
    fn difficulty_index(&self) -> usize {
        match self.difficulty {
            Difficulty::Beginner | Difficulty::BeginnerExtra => 0,
            Difficulty::Advanced | Difficulty::AdvancedExtra => 1,
            _ => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Banner {
    timer: i32,
    duration: i32,
}

impl Banner {
    fn new(duration: i32) -> Self {
        Self { timer: 0, duration }
    }
    fn begin(&mut self) {
        self.timer = self.duration;
    }
    fn active(&self) -> bool {
        self.timer > 0
    }
    /// Frames since it started.
    fn t(&self) -> f32 {
        (self.duration - self.timer) as f32
    }
    fn advance(&mut self) {
        self.timer = (self.timer - 1).max(0);
    }
}

#[derive(Clone, Copy, Debug)]
struct GoalScore {
    clear_score: u64,
    floor_score: u64,
    /// `warpMultiplier`: the floors a green or red goal jumps, 1 when it jumps none.
    warp: u32,
    time_bonus: bool,
    goal: GoalType,
    /// A warp goal's banner says where it leads.
    jump_text: bool,
}

/// A fading copy of the `JUMP TO FLOOR` line, left behind as it sways (`WarpTrail`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct WarpTrail {
    x: f32,
    y: f32,
    scale: f32,
    opacity: f32,
    add_g: f32,
    add_b: f32,
    life: i32,
}

/// Most trails alive at once: one is born every other frame and lives sixteen.
const MAX_WARP_TRAILS: usize = 16;

/// `isWarpGoal`: the green and red goals skip floors.
fn is_warp_goal(goal: GoalType) -> bool {
    goal != GoalType::Blue
}

/// Where the `JUMP TO FLOOR` line is `t` frames into the banner (it sways about the centre).
fn warp_text_pos(t: i32) -> (f32, f32) {
    (
        320.0 + 20.0 * sin_fixed((t << 9) as f32),
        240.0 + 10.0 * sin_fixed(((t << 8) + 0x4000) as f32),
    )
}

#[derive(Clone, Copy, Debug)]
struct BombFragment {
    index: usize,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    opacity: f32,
    scale: f32,
}

fn sin_fixed(angle: f32) -> f32 {
    (angle * std::f32::consts::PI / 32768.0).sin()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `seconds` (3 digits) and `centis` (2 digits) of a frame count, `formatTimerSeconds`.
pub fn format_timer(frames: i32) -> (String, String) {
    let frames = frames.max(0);
    (
        format!("{:03}", frames / 60),
        format!("{:02}", (frames % 60) * 100 / 60),
    )
}

/// Bomb pulse of the last ten seconds, `func800802E0`.
fn bomb_pulse(timer_frames: i32) -> f32 {
    let t = timer_frames as f32;
    if timer_frames > 60 {
        let phase = ((60 - (timer_frames % 60)) & 0x3f) as f32;
        let w = (phase * (std::f32::consts::PI / 128.0)).sin().abs();
        return 0.2 * (1.0 - (1.0 - w * 2.0).abs());
    }
    if timer_frames < 15 {
        let u = (15.0 - t) / 15.0;
        if u < 0.5 {
            return 0.2 - u;
        }
        return (-0.3 + u) - 0.5;
    }
    (1.0 - (t - 15.0) / 45.0) * 0.2
}

/// White laid over the assembled `GOAL` banner `t` frames in, 0..=1: a half sine over frames 30 to 45
/// (`flash = sinFixed((t - 30) * 0x888) * 255`).
fn goal_flash(t: f32) -> f32 {
    if (30.0..45.0).contains(&t) {
        sin_fixed((t - 30.0) * 0x888 as f32).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub struct Hud {
    frame_counter: u32,
    go: Banner,
    goal: Banner,
    hurry: Banner,
    time_over: Banner,
    perfect: Banner,
    bonus_finish: Banner,
    ready: Banner,
    fall_out: Banner,
    last_intro: i32,
    last_goal: i32,
    last_timeover: i32,
    last_ringout: i32,
    last_bonus_clear: bool,
    last_time_left: Option<i32>,
    goal_score: Option<GoalScore>,
    score_display: u64,
    score_step_frames: u32,
    score_step_value: u64,
    bomb_fragments: Vec<BombFragment>,
    bomb_crack_pulse: f32,
    floor_intro_active: bool,
    floor_intro_frames: f32,
    floor_intro_fade: i32,
    intro_start: i32,
    bonus_floor_frames: f32,
    final_floor_frames: f32,
    bonus_banana_counter: f32,
    /// Floors the course jumps for the goal the ball went through ([`Hud::set_jump`]).
    jump: Option<u32>,
    warp_trails: Vec<WarpTrail>,
    /// `JUMP TO FLOOR n` and the value of the warp bonus line, written when the banner starts.
    warp_text: String,
    warp_value: String,
}

impl Hud {
    /// `first_attempt` selects the long floor introduction; `score` is the score at stage start.
    pub fn new(first_attempt: bool, score: u64) -> Self {
        let settled = if first_attempt { 0.0 } else { 150.0 };
        Self {
            frame_counter: 0,
            go: Banner::new(GO_BANNER_FRAMES),
            goal: Banner::new(GOAL_BANNER_FRAMES),
            hurry: Banner::new(HURRY_UP_BANNER_FRAMES),
            time_over: Banner::new(TIME_OVER_BANNER_FRAMES),
            perfect: Banner::new(BONUS_BANNER_FRAMES),
            bonus_finish: Banner::new(BONUS_BANNER_FRAMES),
            ready: Banner::new(READY_BANNER_FRAMES),
            fall_out: Banner::new(FALL_OUT_BANNER_FRAMES),
            last_intro: 0,
            last_goal: 0,
            last_timeover: 0,
            last_ringout: 0,
            last_bonus_clear: false,
            last_time_left: None,
            goal_score: None,
            score_display: score,
            score_step_frames: 0,
            score_step_value: 0,
            bomb_fragments: Vec::new(),
            bomb_crack_pulse: 0.0,
            floor_intro_active: first_attempt,
            floor_intro_frames: 0.0,
            floor_intro_fade: 0,
            intro_start: 0,
            bonus_floor_frames: settled,
            final_floor_frames: settled,
            bonus_banana_counter: 0.0,
            jump: None,
            warp_trails: Vec::with_capacity(MAX_WARP_TRAILS),
            warp_text: String::with_capacity(24),
            warp_value: String::with_capacity(8),
        }
    }

    /// How many floors the course moves on for the goal the ball went through
    /// (`Course::peek_jump_count`, the reference's `course.peekJumpCount`), before the tick in which the
    /// banner starts. `None` where no course decides what follows (practice): the banner then announces
    /// no jump and no warp bonus.
    pub fn set_jump(&mut self, jump: Option<u32>) {
        self.jump = jump;
    }

    /// The `JUMP TO FLOOR n` line on screen (tests).
    pub fn jump_text(&self) -> Option<&str> {
        let shown = self.goal.active() && self.goal_score.is_some_and(|g| g.jump_text);
        shown.then_some(self.warp_text.as_str())
    }

    /// The factor of the `WARP BONUS` line on screen, when there is one (tests).
    pub fn warp_bonus(&self) -> Option<u32> {
        let score = self.goal_score.filter(|_| self.goal.active())?;
        (score.warp > 1).then_some(score.warp)
    }

    /// Name of the banner currently on screen (tests and the window title).
    pub fn banner(&self) -> Option<&'static str> {
        [
            (&self.goal, "GOAL"),
            (&self.fall_out, "FALL OUT"),
            (&self.time_over, "TIME OVER"),
            (&self.go, "GO"),
            (&self.ready, "READY"),
            (&self.perfect, "PERFECT"),
            (&self.bonus_finish, "BONUS FINISH"),
            (&self.hurry, "HURRY UP"),
        ]
        .into_iter()
        .find(|(b, _)| b.active())
        .map(|(_, name)| name)
    }

    fn begin_ready(&mut self) {
        self.ready.begin();
        if self.floor_intro_active {
            self.floor_intro_fade = 15;
        }
    }

    /// Advance one 60 Hz frame (`syncState` + `update` of the reference).
    pub fn tick(&mut self, f: &HudFrame) {
        if self.last_intro > 0 && f.intro_timer == 0 {
            self.go.begin();
        }
        if self.last_intro == 0 && f.intro_timer > 0 {
            self.intro_start = f.intro_timer;
            if f.intro_timer <= 120 {
                self.begin_ready();
            }
        }
        if self.last_goal == 0 && f.goal_timer > 0 && !f.bonus_clear && !self.goal.active() {
            self.goal.begin();
            let time_left = f.time_left().max(0) as u64;
            let goal = f.goal.unwrap_or(GoalType::Blue);
            let clear_score = time_left * 100 / 60
                + match goal {
                    GoalType::Blue => 0,
                    GoalType::Green => 10_000,
                    GoalType::Red => 20_000,
                };
            let time_bonus = f.time_limit > 0 && f.time_left() > (f.time_limit >> 1);
            // The jump the course will make, when one decides: its length is the warp bonus.
            let jump = self.jump.filter(|n| *n > 0 && is_warp_goal(goal));
            let warp = jump.unwrap_or(1);
            let jump_text = jump.is_some() && !f.is_bonus;
            self.warp_text.clear();
            self.warp_value.clear();
            self.warp_trails.clear();
            if jump_text {
                let _ = write!(self.warp_text, "JUMP TO FLOOR {}", f.floor + warp);
            }
            let pad = if warp < 10 { "   X " } else { "  X " };
            let _ = write!(self.warp_value, "{pad}{warp}");
            self.goal_score = Some(GoalScore {
                clear_score,
                floor_score: clear_score * u64::from(warp) * if time_bonus { 2 } else { 1 },
                warp,
                time_bonus,
                goal,
                jump_text,
            });
        }
        if !self.last_bonus_clear && f.bonus_clear {
            self.perfect.begin();
        }
        if self.last_timeover == 0 && f.timeover_timer > 0 {
            if f.is_bonus {
                self.bonus_finish.begin();
            } else {
                self.time_over.begin();
            }
        }
        if self.last_intro > 120 && f.intro_timer <= 120 && f.intro_timer > 0 {
            self.begin_ready();
        }
        if self.last_ringout == 0 && f.ringout_timer > 0 && !self.fall_out.active() {
            if f.is_bonus {
                self.bonus_finish.begin();
            } else {
                self.fall_out.begin();
            }
        }
        self.last_intro = f.intro_timer;
        self.last_goal = f.goal_timer;
        self.last_timeover = f.timeover_timer;
        self.last_ringout = f.ringout_timer;
        self.last_bonus_clear = f.bonus_clear;
        if !self.goal.active() {
            self.goal_score = None;
        }

        for b in [
            &mut self.go,
            &mut self.goal,
            &mut self.hurry,
            &mut self.time_over,
            &mut self.perfect,
            &mut self.bonus_finish,
            &mut self.ready,
            &mut self.fall_out,
        ] {
            b.advance();
        }
        self.frame_counter = self.frame_counter.wrapping_add(1);

        // The displayed score counts up to the real one.
        if self.score_display > f.score {
            self.score_display = f.score;
            self.score_step_frames = 0;
        }
        if self.score_step_frames == 0 && f.score > self.score_display {
            let diff = f.score - self.score_display;
            self.score_step_frames = if diff >= 1000 { 120 } else { 30 };
            self.score_step_value = (diff / self.score_step_frames as u64).max(1);
        }
        if self.score_step_frames > 0 {
            self.score_display = (self.score_display + self.score_step_value).min(f.score);
            self.score_step_frames -= 1;
            if self.score_step_frames == 0 {
                self.score_display = f.score;
            }
        }

        let time_left = (f.time_limit > 0).then(|| f.time_left());
        if time_left == Some(HURRY_UP_FRAMES) && f.intro_timer == 0 && f.goal_timer == 0 {
            self.hurry.begin();
        }
        if let Some(t) = time_left {
            if t <= 0 && self.last_time_left.unwrap_or(1) > 0 {
                self.spawn_bomb_fragments();
            }
            if t < 240 && t > 0 {
                self.bomb_crack_pulse += 40.0 - (t as f32 * 40.0) / 240.0;
            }
        }
        self.last_time_left = time_left;
        self.bomb_fragments.retain_mut(|fr| {
            fr.opacity *= 0.95;
            fr.scale *= 1.01;
            let t = fr.opacity * fr.opacity;
            fr.x += 0.9 * fr.vx * t;
            fr.y += 0.97 * fr.vy * t + (1.0 - t);
            fr.opacity >= 0.005
        });

        if self.floor_intro_active {
            if f.intro_timer > 0 {
                self.intro_start = self.intro_start.max(f.intro_timer);
                let elapsed = (self.intro_start - f.intro_timer) as f32;
                self.floor_intro_frames = self.floor_intro_frames.max(elapsed);
            } else {
                self.floor_intro_frames += 1.0;
                if self.floor_intro_frames > 180.0 {
                    self.floor_intro_active = false;
                }
            }
        }
        if self.floor_intro_fade > 0 {
            self.floor_intro_fade -= 1;
            if self.floor_intro_fade == 0 {
                self.floor_intro_active = false;
            }
        }
        if f.is_bonus {
            self.bonus_floor_frames += 1.0;
            let in_play = f.intro_timer <= 0
                && f.goal_timer <= 0
                && f.ringout_timer <= 0
                && f.timeover_timer <= 0;
            self.bonus_banana_counter = if in_play && f.bananas_left > 0 {
                (self.bonus_banana_counter + 1.0).min(60.0)
            } else {
                (self.bonus_banana_counter - 1.0).max(0.0)
            };
        }
        if f.is_final {
            self.final_floor_frames += 1.0;
        }

        // The `JUMP TO FLOOR` line leaves a trail on every other frame of the banner.
        if self.goal.active() && self.goal_score.is_some_and(|g| g.jump_text) {
            let t = self.goal.duration - self.goal.timer;
            if t % 2 == 1 && self.warp_trails.len() < MAX_WARP_TRAILS {
                let (x, y) = warp_text_pos(t);
                self.warp_trails.push(WarpTrail {
                    x,
                    y,
                    scale: 1.0,
                    opacity: 0.5,
                    add_g: 96.0,
                    add_b: 127.0,
                    life: 16,
                });
            }
        }
        self.warp_trails.retain_mut(|trail| {
            trail.add_g += 6.0;
            trail.add_b += 7.0;
            trail.scale += 0.008;
            trail.opacity -= 0.03125;
            trail.life -= 1;
            trail.life > 0 && trail.opacity > 0.0
        });
        if !self.goal.active() {
            self.warp_trails.clear();
        }
    }

    fn spawn_bomb_fragments(&mut self) {
        self.bomb_fragments = (0..BOMB_FRAG_X.len())
            .map(|i| BombFragment {
                index: i,
                x: 320.0 - 44.0 + BOMB_FRAG_X[i],
                y: 68.0 - 44.0 + BOMB_FRAG_Y[i],
                vx: 1.2 * (BOMB_FRAG_X[i] - 30.0),
                vy: 1.2 * (BOMB_FRAG_Y[i] - 20.0),
                opacity: 1.0,
                scale: 1.0,
            })
            .collect();
    }

    /// Queue the HUD sprites for this frame.
    pub fn draw(&self, p: &mut Painter, layout: &Layout, f: &HudFrame) {
        let v = HudView::new(layout);
        let s = v.s;
        let ui = p.ui;
        let (top_l, top_c, top_r) = ((H::Left, V::Top), (H::Center, V::Top), (H::Right, V::Top));
        let bottom_l = (H::Left, V::Bottom);
        let text = |p: &mut Painter,
                    font: FontId,
                    t: &str,
                    (x, y): (f32, f32),
                    (h, vv): (H, V),
                    color: Color,
                    align: Align| {
            p.text(font, t, v.x(x, h), v.y(y, vv), s, s, color, align);
        };

        text(
            p,
            FontId::Asc20,
            "SCORE",
            (108.0, 24.0),
            top_l,
            WHITE,
            Align::Center,
        );
        text(
            p,
            FontId::Num22x22,
            &self.score_display.to_string(),
            (196.0, 48.0),
            top_l,
            WHITE,
            Align::Right,
        );
        if let Some(lives) = f.lives {
            let (x, y) = (v.x(34.0, H::Left), v.y(78.0, V::Top));
            p.image(ui.face, x, y, 0.19 * s, 0.19 * s, WHITE);
            text(
                p,
                FontId::Asc20,
                &format!("X{lives}"),
                (50.0, 79.0),
                top_l,
                WHITE,
                Align::Left,
            );
        }

        p.image(
            ui.banana,
            v.x(428.0, H::Right),
            v.y(22.0, V::Top),
            0.2 * s,
            0.2 * s,
            WHITE,
        );
        text(
            p,
            FontId::Asc20,
            "BANANA(S)",
            (536.0, 24.0),
            top_r,
            YELLOW,
            Align::Center,
        );
        text(
            p,
            FontId::Num22x22,
            "/",
            (518.0, 47.0),
            top_r,
            YELLOW,
            Align::Center,
        );
        let total = f.bananas + f.bananas_left;
        text(
            p,
            FontId::Asc20,
            &format!("{:03} {:03}", f.bananas.max(0), total.max(0)),
            (518.0, 48.0),
            top_r,
            YELLOW,
            Align::Center,
        );

        let time_left_raw = f.time_left();
        let time_left = time_left_raw.max(0);
        let (bomb_x, bomb_y) = (v.x(320.0, H::Center), v.y(68.0, V::Top));
        if time_left_raw > 0 {
            let scale = if time_left_raw <= 600 {
                1.0 + bomb_pulse(time_left)
            } else {
                1.0
            } * s;
            p.image(ui.bomb, bomb_x, bomb_y, scale, scale, WHITE);
            if time_left_raw <= 480 {
                let opacity = if time_left_raw > 420 {
                    1.0 - (time_left_raw - 420) as f32 / 60.0
                } else {
                    1.0
                };
                let mut color = WHITE;
                if time_left_raw < 240 {
                    let pulse = (255.0 - (self.bomb_crack_pulse.floor() % 510.0)).abs();
                    let blue = pulse.max(128.0);
                    color = [1.0, pulse / 255.0, (blue - 128.0) * 2.0 / 255.0, 1.0];
                }
                p.image(
                    ui.bomb_crack,
                    bomb_x,
                    bomb_y,
                    scale,
                    scale,
                    with_alpha(color, opacity),
                );
            }
        }
        // Timer: big seconds with the small centiseconds tucked against their baseline.
        let (seconds, centis) = format_timer(time_left);
        let sec_font = ui.font(FontId::Num24x37).p;
        let cs_font = ui.font(FontId::Num12x19).p;
        let sec_w = sec_font.measure(&seconds, 1.0);
        let sec_left = 320.0 - sec_w / 2.0;
        let sec_mid = 85.0 - sec_font.line_height / 2.0;
        text(
            p,
            FontId::Num24x37,
            &seconds,
            (sec_left, sec_mid),
            top_c,
            WHITE,
            Align::Left,
        );
        text(
            p,
            FontId::Num12x19,
            &format!(":{centis}"),
            (sec_left + sec_w - 4.0, 85.0 - cs_font.line_height / 2.0),
            top_c,
            WHITE,
            Align::Left,
        );

        // Bottom left: speed and floor.
        let speed = format!("{:02}", (f.speed.max(0.0) * SPEED_TO_MPH).round() as u32);
        text(
            p,
            FontId::Asc20,
            &speed,
            (32.0, 428.0),
            bottom_l,
            WHITE,
            Align::Left,
        );
        let speed_w = ui.font(FontId::Asc20).p.measure(&speed, 1.0);
        p.image(
            ui.mph,
            v.x(32.0 + speed_w + ui.mph.w / 2.0 + 6.0, H::Left),
            v.y(428.0, V::Bottom),
            s,
            s,
            WHITE,
        );
        let prefix = f.floor_prefix();
        let show_icon = prefix != "MASTER";
        let floor_color = if prefix == "EXTRA" {
            rgb(0xffe14d)
        } else {
            WHITE
        };
        text(
            p,
            FontId::Asc20,
            &format!("{prefix} {}", f.floor),
            (if show_icon { 72.0 } else { 32.0 }, 458.0),
            bottom_l,
            floor_color,
            Align::Left,
        );
        if show_icon {
            let cell = 64.0;
            p.image_part(
                ui.lv_icon,
                [f.difficulty_index() as f32 * cell, cell, cell, cell],
                v.x(48.0, H::Left),
                v.y(458.0, V::Bottom),
                0.5 * s,
                0.5 * s,
                WHITE,
            );
        }

        if f.is_bonus && self.bonus_banana_counter > 0.0 {
            let opacity = self.bonus_banana_counter.min(60.0) / 60.0;
            let mut blueness = 2.0 * ((self.frame_counter % 60) as f32 / 59.0);
            if blueness > 1.0 {
                blueness = 2.0 - blueness;
            }
            let plural = if f.bananas_left == 1 { ' ' } else { 'S' };
            p.text(
                FontId::Asc32,
                &format!("{:2} BANANA{plural} LEFT", f.bananas_left.max(0)),
                v.x(320.0, H::Center),
                v.y(380.0, V::Bottom),
                s,
                s,
                [1.0, 1.0, blueness, opacity],
                Align::Center,
            );
        }

        for fr in &self.bomb_fragments {
            let img = ui.bomb_parts[fr.index % ui.bomb_parts.len()];
            p.image(
                img,
                v.x(fr.x, H::Center),
                v.y(fr.y, V::Top),
                fr.scale * s,
                fr.scale * s,
                with_alpha(WHITE, fr.opacity),
            );
        }

        self.draw_banners(p, &v, f);
    }

    fn draw_banners(&self, p: &mut Painter, v: &HudView, f: &HudFrame) {
        let s = v.s;
        let big = p.ui.font(FontId::Asc72).p;
        let cx = |x: f32| v.x(x, H::Center);
        let cy = |y: f32| v.y(y, V::Center);

        if self.go.active() {
            let t = self.go.t();
            let blink = (self.frame_counter >> 1) & 1 == 1;
            // Squash in, two bounces, stretch out.
            let (sx, sy, opacity, flash) = if t < 15.0 {
                let temp = 15.0 - t;
                (1.0 + temp * 0.1, 1.0 - temp * 0.04, t / 15.0, false)
            } else if t < 30.0 {
                let wave = sin_fixed((t - 15.0) * 0x888 as f32);
                (1.0 - wave * 0.5, 1.0 + wave, 1.0, blink)
            } else if t < 45.0 {
                let wave = sin_fixed((t - 30.0) * 0x888 as f32);
                (1.0 + wave * 0.75, 1.0 - wave * 0.375, 1.0, blink)
            } else {
                let remaining = self.go.timer as f32;
                (
                    1.0 - (15.0 - remaining) * 0.04,
                    1.0 + (15.0 - remaining) * 0.1,
                    remaining / 15.0,
                    false,
                )
            };
            let color = if flash { rgb(0xc0f2ff) } else { rgb(0x00a0ff) };
            let (base_sx, base_sy) = (1.5 * sx, 1.5 * sy);
            let half_w = 1.5 * 36.0 * sx;
            let half_h = 1.5 * 32.0 * sy;
            for (i, code) in (*b"GO").into_iter().enumerate() {
                let side = if i == 0 { -1.0 } else { 1.0 };
                let (mut dx, mut dy) = (0.0, 0.0);
                if t < 15.0 {
                    dx = side * 320.0 * sin_fixed((15.0 - t) * 0x444 as f32);
                } else if t >= 45.0 {
                    dy = side * 240.0 * sin_fixed((15.0 - self.go.timer as f32) * 0x444 as f32);
                }
                let x = 320.0 + dx - half_w + side * half_w;
                let y = 240.0 + dy - half_h + big.line_height * base_sy / 2.0;
                p.glyph(
                    FontId::Asc72,
                    code,
                    cx(x),
                    cy(y),
                    base_sx * s,
                    base_sy * s,
                    with_alpha(color, opacity),
                );
            }
        }

        if self.goal.active() {
            let t = self.goal.t();
            let (mut x, mut y, mut scale, mut opacity) = (320.0, 320.0, 1.0, 1.0);
            if t < 30.0 {
                opacity = t / 30.0;
            } else if self.goal.timer < 15 {
                opacity = self.goal.timer as f32 / 15.0;
            }
            if t >= 180.0 {
                let t2 = ((t - 180.0) / 30.0).clamp(0.0, 1.0);
                x = lerp(320.0, 561.0, t2);
                y = lerp(320.0, 420.0, t2);
                scale = lerp(1.0, 0.5, t2);
            }
            let img = p.ui.goal;
            let color = with_alpha(WHITE, opacity);
            if t < 30.0 {
                // Four copies fly in from the corners.
                let scatter = (30.0 - t) * 0.03333;
                for (dx, dy) in [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                    p.image(
                        img,
                        cx(x + scatter * 216.0 * dx),
                        cy(y + scatter * 192.0 * dy),
                        scale * s,
                        scale * s,
                        color,
                    );
                }
            } else {
                p.image(img, cx(x), cy(y), scale * s, scale * s, color);
                // Once assembled the banner flashes white for a quarter of a second: its own shape in
                // plain white over it (`drawSpriteSolidTint`).
                let flash = goal_flash(t);
                if flash > 0.0 {
                    p.image_solid(img, cx(x), cy(y), scale * s, scale * s, opacity * flash);
                }
            }

            if self.goal_score.is_some_and(|g| g.jump_text) && !f.is_bonus {
                for trail in &self.warp_trails {
                    let color = [
                        0.0,
                        trail.add_g.round().min(255.0) / 255.0,
                        trail.add_b.round().min(255.0) / 255.0,
                        trail.opacity.clamp(0.0, 1.0),
                    ];
                    p.text(
                        FontId::Asc32,
                        &self.warp_text,
                        cx(trail.x),
                        cy(trail.y),
                        trail.scale * s,
                        trail.scale * s,
                        color,
                        Align::Center,
                    );
                }
                let opacity = if t < 30.0 {
                    t / 30.0
                } else if self.goal.timer < 15 {
                    self.goal.timer as f32 / 15.0
                } else {
                    1.0
                };
                let (x, y) = warp_text_pos(t as i32);
                p.text(
                    FontId::Asc32,
                    &self.warp_text,
                    cx(x),
                    cy(y),
                    s,
                    s,
                    with_alpha(rgb(0x00c0ff), opacity),
                    Align::Center,
                );
            }
        }

        if let (Some(info), true, false) = (self.goal_score, self.goal.active(), f.is_bonus) {
            self.draw_goal_score(p, v, &info);
        }

        if self.hurry.active() {
            let t = self.hurry.t() as u32;
            if t.is_multiple_of(2) {
                // The reference jitters with Math.random; a frame-keyed pattern keeps drawing repeatable.
                let n = self.frame_counter.wrapping_mul(2_654_435_761);
                let jx = ((n >> 8) & 0xff) as f32 / 255.0 - 0.5;
                let jy = ((n >> 16) & 0xff) as f32 / 255.0 - 0.5;
                p.text(
                    FontId::Asc32,
                    "HURRY UP!",
                    cx(320.0 + jx * 4.0),
                    cy(240.0 + jy * 2.0),
                    s,
                    s,
                    rgb(0xff8800),
                    Align::Center,
                );
            }
        }

        // TIME OVER and FALL OUT drop in from above and bounce twice.
        let drop_y = |t: f32| {
            if t < 30.0 {
                240.0 - (30.0 - t) * 6.0
            } else if t < 60.0 {
                240.0 - 32.0 * sin_fixed((t - 30.0) * 0x444 as f32)
            } else if t < 90.0 {
                240.0 - 8.0 * sin_fixed((t - 60.0) * 0x444 as f32)
            } else {
                240.0
            }
        };
        if self.time_over.active() {
            let t = self.time_over.t();
            let opacity = if t < 30.0 {
                t / 30.0
            } else if self.time_over.timer < 15 {
                self.time_over.timer as f32 / 15.0
            } else {
                1.0
            };
            p.text(
                FontId::Asc72,
                "TIME OVER",
                cx(320.0),
                cy(drop_y(t)),
                s,
                s,
                with_alpha(rgb(0xff8c00), opacity),
                Align::Center,
            );
        }

        if self.ready.active() {
            let t = self.ready.t();
            let (mut opacity, mut sx, mut sy) = (1.0, 1.0, 1.0);
            if t < 30.0 {
                opacity = 0.03333 * t;
                sy = sin_fixed(t * 0x222 as f32);
            } else if t < 60.0 {
                sy = 1.0 - sin_fixed((t - 30.0) * 0x444 as f32) * 0.2;
            } else if self.ready.timer <= 15 {
                opacity = 0.06666 * self.ready.timer as f32;
                sx = opacity;
            }
            p.text(
                FontId::Asc72,
                "READY",
                cx(320.0),
                cy(240.0),
                sx * s,
                sy * s,
                with_alpha(rgb(0xffc800), opacity),
                Align::Center,
            );
        }

        for (banner, label, color) in [
            (&self.perfect, "PERFECT", WHITE),
            (&self.bonus_finish, "BONUS FINISH", rgb(0xff8800)),
        ] {
            if banner.active() {
                let scale = lerp(5.0, 0.8, (banner.t() / 30.0).clamp(0.0, 1.0)) * s;
                p.text(
                    FontId::Asc72,
                    label,
                    cx(320.0),
                    cy(240.0),
                    scale,
                    scale,
                    color,
                    Align::Center,
                );
            }
        }

        if self.fall_out.active() {
            let t = self.fall_out.t();
            let opacity = if t < 30.0 {
                0.03333 * t
            } else if self.fall_out.timer <= 15 {
                0.06666 * self.fall_out.timer as f32
            } else {
                1.0
            };
            let (mut x, mut y, mut scale) = (320.0, drop_y(t), 1.0);
            if t >= 120.0 {
                (x, y, scale) = (496.0, 420.0, 0.5);
            } else if t >= 90.0 {
                let dt = t - 90.0;
                x = 320.0 + dt * 5.86666;
                y = 240.0 + dt * 6.0;
                scale = 1.0 - dt * 0.01666;
            }
            p.text(
                FontId::Asc72,
                "FALL OUT",
                cx(x),
                cy(y),
                scale * s,
                scale * s,
                with_alpha(rgb(0xff8c00), opacity),
                Align::Center,
            );
        }

        if self.floor_intro_active {
            self.draw_floor_intro(p, v, f);
        }
        if f.is_bonus {
            self.draw_corner_label(p, v, "BONUS FLOOR", rgb(0xff8800), self.bonus_floor_frames);
        }
        if f.is_final {
            let color = match f.difficulty_index() {
                0 => rgb(0x00d000),
                1 => rgb(0x0000e0),
                _ => rgb(0xffc000),
            };
            self.draw_corner_label(p, v, "FINAL FLOOR", color, self.final_floor_frames);
        }
    }

    /// `CLEAR SCORE` / `TIME BONUS` / `FLOOR SCORE` lines that slide in during the goal banner.
    fn draw_goal_score(&self, p: &mut Painter, v: &HudView, info: &GoalScore) {
        let s = v.s;
        let counter = self.goal.timer as f32;
        let base_t = self.goal.t();
        let font = p.ui.font(FontId::Asc16).p;
        let mut line_y = 128.0;
        let line = |p: &mut Painter, t: f32, y: f32, label: &str, value: &str, color: Color| {
            if t < 60.0 {
                return;
            }
            let (opacity, offset) = if t < 90.0 {
                (0.03333 * (t - 60.0), (t - 90.0) * 5.0)
            } else if counter <= 15.0 {
                (0.06666 * counter, 0.0)
            } else {
                (1.0, 0.0)
            };
            // The reference draws these top-aligned at y - 10.
            let mid = v.y(y - 10.0 + font.line_height / 2.0, V::Top);
            let x = v.x(24.0 + offset, H::Left);
            p.text(
                FontId::Asc16,
                label,
                x,
                mid,
                s,
                s,
                with_alpha(color, opacity),
                Align::Left,
            );
            if t >= 120.0 {
                let value_opacity = if t < 150.0 {
                    0.03333 * (t - 120.0)
                } else if counter <= 15.0 {
                    0.06666 * counter
                } else {
                    1.0
                };
                p.text(
                    FontId::Asc16,
                    value,
                    x + font.measure(label, s),
                    mid,
                    s,
                    s,
                    with_alpha(color, value_opacity),
                    Align::Left,
                );
            }
        };
        let clear_color = match info.goal {
            GoalType::Blue => rgb(0x6060ff),
            GoalType::Green => rgb(0x60ff60),
            GoalType::Red => rgb(0xff6060),
        };
        line(
            p,
            base_t,
            line_y,
            "CLEAR SCORE : ",
            &format!(" {:5}", info.clear_score),
            clear_color,
        );
        if info.warp > 1 {
            line_y += 20.0;
            line(
                p,
                base_t - 10.0,
                line_y,
                " WARP BONUS   ",
                &self.warp_value,
                rgb(0x00c0ff),
            );
        }
        if info.time_bonus {
            line_y += 20.0;
            line(
                p,
                base_t - 20.0,
                line_y,
                " TIME BONUS   ",
                "   X 2",
                rgb(0xffff00),
            );
        }
        line_y += 24.0;
        let floor_t = base_t - 60.0;
        let flash = (sin_fixed(floor_t * 0x280 as f32)).max(0.0);
        line(
            p,
            floor_t,
            line_y,
            "FLOOR SCORE : ",
            &format!("{:6}", info.floor_score),
            [1.0, (128.0 / 255.0 + flash).min(1.0), flash.min(1.0), 1.0],
        );
    }

    /// "FLOOR n" assembling itself letter by letter during the fly-in.
    fn draw_floor_intro(&self, p: &mut Painter, v: &HudView, f: &HudFrame) {
        let s = v.s;
        let prefix = f.floor_prefix();
        let text = format!("{prefix} {}", f.floor);
        let space_index = prefix.len() as f32;
        let center_x = text.len() as f32 * 57.0 / 2.0;
        let t = self.floor_intro_frames;
        let fade = self.floor_intro_fade as f32;
        let fade_opacity = if fade > 0.0 { 0.06666 * fade } else { 1.0 };
        let color = if prefix == "EXTRA" {
            rgb(0xffe14d)
        } else {
            rgb(0xffd200)
        };
        for (i, c) in text.bytes().enumerate() {
            let fi = i as f32;
            if c == b' ' {
                continue;
            }
            // Letters appear one after another; the number appears with the space.
            let appear = if fi > space_index {
                Some(30.0 + space_index * 16.0)
            } else if i > 0 {
                Some(30.0 + (fi - 1.0) * 16.0)
            } else {
                None
            };
            if appear.is_some_and(|a| t < a) {
                continue;
            }
            let mut x_offset = 0.0;
            if i == 0 && t < 30.0 {
                x_offset = (30.0 - t) * -5.0;
            } else if i > 0 && fi < space_index && t < 30.0 + fi * 16.0 {
                x_offset = -3.6 * (30.0 + fi * 16.0 - t);
            } else if fi > space_index {
                x_offset = -28.0;
            }
            let mut y_offset = 0.0;
            if fade > 0.0 {
                let odd = i & 1 == 1;
                let dir = if (fi < space_index) == odd { 1.0 } else { -1.0 };
                y_offset = (15.0 - fade) * 8.0 * dir;
            }
            let opacity = match appear {
                Some(a) => (0.0625 * (t - a)).min(fade_opacity),
                None => fade_opacity,
            };
            p.glyph(
                FontId::Asc72,
                c,
                v.x(320.0 - center_x + 14.4 + 57.6 * fi + x_offset, H::Center),
                v.y(240.0 + y_offset, V::Center),
                0.8 * s,
                0.8 * s,
                with_alpha(color, opacity),
            );
        }
    }

    /// "BONUS FLOOR" / "FINAL FLOOR": shown in the middle, then parked in the bottom-right corner.
    fn draw_corner_label(&self, p: &mut Painter, v: &HudView, text: &str, color: Color, t: f32) {
        let opacity = if t < 60.0 { t / 60.0 } else { 1.0 };
        let travel = ((t - 120.0) / 30.0).clamp(0.0, 1.0);
        let scale = lerp(0.5, 0.3, travel) * v.s;
        // Blend between the centre anchor and the corner anchor so the label ends in the real corner.
        let x = lerp(v.x(320.0, H::Center), v.x(500.0, H::Right), travel);
        let y = lerp(v.y(300.0, V::Center), v.y(452.0, V::Bottom), travel);
        p.text(
            FontId::Asc72,
            text,
            x,
            y,
            scale,
            scale,
            with_alpha(color, opacity),
            Align::Center,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Insets;

    fn frame() -> HudFrame {
        HudFrame {
            intro_timer: 0,
            goal_timer: 0,
            ringout_timer: 0,
            timeover_timer: 0,
            time_limit: 3600,
            stage_timer: 0,
            score: 0,
            bananas: 0,
            bananas_left: 3,
            lives: Some(3),
            speed: 0.0,
            floor: 1,
            difficulty: Difficulty::Beginner,
            is_bonus: false,
            is_final: false,
            goal: None,
            bonus_clear: false,
        }
    }

    #[test]
    fn timer_format_matches_the_reference() {
        assert_eq!(format_timer(3600), ("060".into(), "00".into()));
        assert_eq!(format_timer(3599), ("059".into(), "98".into()));
        assert_eq!(format_timer(-5), ("000".into(), "00".into()));
    }

    #[test]
    fn view_anchors_follow_the_safe_area() {
        let l = Layout::new(
            (1920, 720),
            Insets {
                left: 100.0,
                ..Default::default()
            },
            1.0,
        );
        let v = HudView::new(&l);
        assert!((v.s - 1.5).abs() < 1e-6);
        assert_eq!(v.x(0.0, H::Left), 100.0);
        assert_eq!(v.x(640.0, H::Right), 1920.0);
        assert_eq!(v.x(320.0, H::Center), 100.0 + 910.0);
        assert_eq!(v.y(480.0, V::Bottom), 720.0);
        // Portrait keeps the full 640 units across the width.
        let p = HudView::new(&Layout::new((720, 1280), Insets::default(), 1.0));
        assert!((p.s - 1.125).abs() < 1e-6);
        assert_eq!(p.y(240.0, V::Center), 640.0);
    }

    #[test]
    fn banners_follow_the_stage_phases() {
        let mut hud = Hud::new(true, 0);
        let mut f = frame();
        f.intro_timer = 360;
        hud.tick(&f);
        assert_eq!(hud.banner(), None);
        f.intro_timer = 120;
        hud.tick(&f);
        assert_eq!(hud.banner(), Some("READY"));
        f.intro_timer = 1;
        hud.tick(&f);
        f.intro_timer = 0;
        hud.tick(&f);
        assert_eq!(hud.banner(), Some("GO"));
        for _ in 0..GO_BANNER_FRAMES + READY_BANNER_FRAMES {
            hud.tick(&f);
        }
        assert_eq!(hud.banner(), None);
        f.ringout_timer = 270;
        hud.tick(&f);
        assert_eq!(hud.banner(), Some("FALL OUT"));
    }

    #[test]
    fn retry_shows_ready_immediately_and_goal_prices_the_floor() {
        let mut hud = Hud::new(false, 500);
        let mut f = frame();
        f.intro_timer = 120;
        hud.tick(&f);
        assert_eq!(hud.banner(), Some("READY"));
        let mut f = frame();
        f.stage_timer = 600;
        f.goal_timer = 360;
        f.goal = Some(GoalType::Green);
        hud.tick(&f);
        assert_eq!(hud.banner(), Some("GOAL"));
        let info = hud.goal_score.unwrap();
        assert_eq!(info.clear_score, 3000 * 100 / 60 + 10_000);
        assert!(info.time_bonus && info.floor_score == info.clear_score * 2);
    }

    /// The banner's score lines are a third statement of the goal-score rule (after the engine's and the
    /// course flow's); what it shows must be what the player is given.
    #[test]
    fn goal_banner_prices_the_floor_like_the_course_flow() {
        let rules = mb_game::Rules::default();
        for goal in [GoalType::Blue, GoalType::Green, GoalType::Red] {
            for limit in [0, 1, 60, 1800, 3600, 7200] {
                for elapsed in [
                    0, 1, 59, 60, 899, 900, 901, 1799, 1800, 1801, 3599, 3600, 9000,
                ] {
                    let mut hud = Hud::new(false, 0);
                    let mut f = frame();
                    f.time_limit = limit;
                    f.stage_timer = elapsed;
                    f.goal_timer = 360;
                    f.goal = Some(goal);
                    hud.tick(&f);
                    let left = (limit - elapsed).max(0) as u32;
                    assert_eq!(
                        hud.goal_score.unwrap().floor_score,
                        rules.goal_score(goal, left, limit as u32),
                        "{goal:?}, {left} of {limit} frames left"
                    );
                }
            }
        }
    }

    /// A goal frame: `elapsed` frames of a 60 second clock gone, through a goal of `goal`.
    fn goal_frame(goal: GoalType, elapsed: i32) -> HudFrame {
        let mut f = frame();
        f.floor = 2;
        f.stage_timer = elapsed;
        f.goal_timer = 360;
        f.goal = Some(goal);
        f
    }

    #[test]
    fn a_warp_goal_announces_its_floor_and_multiplies_the_floor_score() {
        // Beginner floor 2, green goal: the course jumps three floors, to floor 5.
        let mut hud = Hud::new(false, 0);
        hud.set_jump(Some(3));
        let f = goal_frame(GoalType::Green, 600);
        hud.tick(&f);
        assert_eq!(hud.banner(), Some("GOAL"));
        assert_eq!(hud.jump_text(), Some("JUMP TO FLOOR 5"));
        assert_eq!(hud.warp_bonus(), Some(3));
        let info = hud.goal_score.unwrap();
        // `floorScore = clearScore * warpMultiplier * timeBonus`.
        assert_eq!(info.clear_score, 3000 * 100 / 60 + 10_000);
        assert_eq!(info.floor_score, info.clear_score * 3 * 2);
        assert_eq!(hud.warp_value, "   X 3");

        // A plain goal, or a jump of one floor through a warp goal: no bonus line.
        for (goal, jump) in [
            (GoalType::Blue, Some(1)),
            (GoalType::Blue, Some(3)),
            (GoalType::Red, Some(1)),
        ] {
            let mut hud = Hud::new(false, 0);
            hud.set_jump(jump);
            hud.tick(&goal_frame(goal, 600));
            assert_eq!(hud.warp_bonus(), None, "{goal:?} {jump:?}");
            // The line is for the goals that warp, however far.
            let expect = (goal == GoalType::Red).then_some("JUMP TO FLOOR 3");
            assert_eq!(hud.jump_text(), expect, "{goal:?} {jump:?}");
        }
        // No course (practice): nothing jumps, so nothing is announced or multiplied.
        let mut hud = Hud::new(false, 0);
        hud.tick(&goal_frame(GoalType::Red, 600));
        assert_eq!((hud.jump_text(), hud.warp_bonus()), (None, None));
        // A bonus stage never shows the line; ten floors and more lose a space of padding.
        let mut hud = Hud::new(false, 0);
        hud.set_jump(Some(12));
        let mut f = goal_frame(GoalType::Red, 600);
        f.is_bonus = true;
        hud.tick(&f);
        assert_eq!(hud.jump_text(), None);
        assert_eq!(hud.warp_value, "  X 12");
    }

    #[test]
    fn the_jump_line_sways_and_leaves_trails_like_the_reference() {
        let mut hud = Hud::new(false, 0);
        hud.set_jump(Some(4));
        let f = goal_frame(GoalType::Green, 600);
        // The tick that starts the banner is already its first frame (t = 1): a trail is born and aged.
        hud.tick(&f);
        assert_eq!(hud.goal.duration - hud.goal.timer, 1);
        assert_eq!(hud.warp_trails.len(), 1);
        let first = hud.warp_trails[0];
        let (x, y) = warp_text_pos(1);
        assert_eq!((first.x, first.y), (x, y));
        assert!((x - (320.0 + 20.0 * sin_fixed(512.0))).abs() < 1e-5);
        assert!((y - (240.0 + 10.0 * sin_fixed(256.0 + 16384.0))).abs() < 1e-5);
        assert_eq!((first.add_g, first.add_b, first.life), (102.0, 134.0, 15));
        assert!((first.scale - 1.008).abs() < 1e-6 && (first.opacity - 0.46875).abs() < 1e-6);
        // One every other frame, each gone after sixteen: eight at most.
        let mut most = 0;
        for _ in 0..60 {
            hud.tick(&f);
            most = most.max(hud.warp_trails.len());
            assert!(
                hud.warp_trails
                    .iter()
                    .all(|t| t.life > 0 && t.opacity > 0.0)
            );
        }
        assert_eq!(most, 8);
        // They end with the banner.
        for _ in 0..GOAL_BANNER_FRAMES {
            hud.tick(&frame());
        }
        assert!(hud.banner().is_none() && hud.warp_trails.is_empty());
        assert_eq!(hud.jump_text(), None);
    }

    #[test]
    fn the_goal_banner_flashes_white_once_it_is_assembled() {
        // Nothing while the four copies fly in, a half sine over the next fifteen frames, nothing after.
        assert_eq!(goal_flash(0.0), 0.0);
        assert_eq!(goal_flash(29.0), 0.0);
        assert_eq!(goal_flash(30.0), 0.0);
        assert!((goal_flash(37.0) - sin_fixed(7.0 * 2184.0)).abs() < 1e-6);
        assert!(goal_flash(37.0) > 0.99 && goal_flash(38.0) > 0.99);
        assert!(goal_flash(44.0) > 0.0 && goal_flash(44.0) < 0.25);
        assert_eq!(goal_flash(45.0), 0.0);
        assert_eq!(goal_flash(200.0), 0.0);
    }

    #[test]
    fn score_counts_up_to_the_target() {
        let mut hud = Hud::new(false, 0);
        let mut f = frame();
        f.score = 100;
        hud.tick(&f);
        assert!(hud.score_display > 0 && hud.score_display < 100);
        for _ in 0..40 {
            hud.tick(&f);
        }
        assert_eq!(hud.score_display, 100);
    }

    #[test]
    fn time_over_spawns_bomb_fragments_and_the_banner() {
        let mut hud = Hud::new(false, 0);
        let mut f = frame();
        f.stage_timer = 3599;
        hud.tick(&f);
        f.stage_timer = 3600;
        f.timeover_timer = 120;
        hud.tick(&f);
        assert_eq!(hud.banner(), Some("TIME OVER"));
        assert_eq!(hud.bomb_fragments.len(), 10);
    }
}
