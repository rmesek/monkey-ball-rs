//! Regression budgets (`docs/PARITY_PLAN.md`, "Regression budgets as tests"): what a frame and a stage may
//! cost, counted rather than timed so the tests do not depend on the load of the machine.
//!
//! * draw calls per stage outside the particles equal the recorded 0.1.0 table plus what was added on
//!   purpose since (`DRAWS_ADDED`); the particles add at most 12;
//! * no GPU object is created while a stage is played;
//! * the textures of the heaviest stage stay within 1 MB of what 0.1.0 uploaded;
//! * a mirror floor is drawn on the 37 stages that have one and on no other: its draws and the bytes of
//!   its target are recorded apart from the rest, and with the Reflections option off those stages cost
//!   what they did.
//!
//! The pack's size and file count are in `tests/pack.rs`. Like the other tests these need the developer's
//! own disc image and a GPU adapter, and skip without either.
//!
//! `MB_BUDGET_PRINT=1 cargo test -p mb-app --release --test budgets -- --nocapture` prints the tables as
//! measured instead of comparing them, for the day a number is changed on purpose.

mod common;

use common::find_disc;

use std::sync::Mutex;

use mb_app::content::{course_stage_ids, open_disc_image};
use mb_app::headless::{Headless, FRAME};
use mb_app::input::Axis;
use mb_app::menu::{Autostart, Menu};
use mb_engine::Phase;
use mb_game::{Difficulty, MemoryStorage, Profile};
use mb_render::{FrameStats, GpuCounters};

static GPU: Mutex<()> = Mutex::new(());

/// A headless game with nothing on its stack, or `None` (with a note) when the machine cannot run it.
fn headless() -> Option<Headless> {
    let Some(disc) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return None;
    };
    let content = match open_disc_image(&disc) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {}: {e}", disc.display());
            return None;
        }
    };
    match Headless::new(&content, (640, 360), false) {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("skipped: no GPU adapter ({e})");
            None
        }
    }
}

fn printing() -> bool {
    std::env::var_os("MB_BUDGET_PRINT").is_some()
}

/// Start `stage` in practice (first attempt) and run its first frame.
fn start_stage(h: &mut Headless, stage: u32) {
    start_stage_with(h, stage, true)
}

/// [`start_stage`] on a save with the Reflections option as given (it is on unless the player turns it
/// off).
fn start_stage_with(h: &mut Headless, stage: u32, reflections: bool) {
    h.shutdown();
    let mut storage = MemoryStorage::new();
    let mut profile = Profile::default();
    profile.settings.unlock_all = true;
    profile.settings.reflections = reflections;
    profile.save(&mut storage).unwrap();
    let menu = Menu::new(Box::new(storage)).with_autostart(Some(Autostart::Stage(stage)));
    h.push(Box::new(menu))
        .unwrap_or_else(|e| panic!("stage {stage}: {e}"));
    assert!(h.step(FRAME).unwrap());
    let view = h.top().unwrap().play_view();
    assert_eq!(view.map(|v| v.request.stage_id), Some(stage));
}

fn tick(h: &Headless) -> u32 {
    h.top().unwrap().play_view().unwrap().sim.tick
}

/// Render the frame and return its statistics.
fn render(h: &mut Headless) -> FrameStats {
    h.render().unwrap();
    h.renderer().stats
}

/// The draw calls of a frame that are neither particles nor the mirror's: stage, backdrop, ball, HUD.
/// This is the path 0.1.0 had, so it is what the table below holds. (A mirror model is drawn with the
/// reflection only, so it has no draw among these.)
fn base_draws(stats: &FrameStats) -> usize {
    stats.total_draws() - stats.instance_draws - stats.quad_draws - stats.mirror_draws
}

/// Show the title menu and draw it, as every session does before its first stage: what only the first
/// frame of a window can make (the frame's depth texture) exists from then on.
fn show_title(h: &mut Headless) {
    h.shutdown();
    h.push(Box::new(Menu::new(mb_app::headless::unlocked_storage())))
        .expect("menu");
    assert!(h.step(FRAME).unwrap());
    render(h);
}

/// Tick of the fly-in at which the first count is taken, and of play (the intro of a first attempt is 360
/// ticks; nobody steers) for the second.
const INTRO_TICK: u32 = 60;
const PLAY_TICK: u32 = 420;

/// `(stage, draws at INTRO_TICK, draws at PLAY_TICK)` for every course stage at 640 x 360, as version 0.1.0
/// drew them: the count of everything but particles, which did not exist then. Counts are not game data.
/// A stage that draws more than this got slower on WebGL2, where the cost of a frame is its draw calls.
#[rustfmt::skip]
const DRAWS_0_1_0: &[(u32, usize, usize)] = &[
    (1, 316, 288), (2, 429, 364), (3, 343, 226), (4, 267, 278), (91, 222, 232), (5, 304, 276),
    (6, 352, 221), (7, 439, 433), (8, 326, 304), (9, 309, 308), (11, 293, 210), (12, 412, 357),
    (13, 309, 263), (14, 294, 305), (15, 297, 295), (16, 291, 273), (17, 335, 338), (18, 389, 383),
    (92, 215, 225), (21, 565, 564), (22, 323, 268), (23, 344, 280), (24, 490, 425), (25, 298, 238),
    (26, 314, 259), (27, 274, 197), (28, 494, 198), (29, 354, 267), (93, 386, 396), (31, 498, 496),
    (32, 254, 253), (33, 291, 246), (34, 365, 372), (35, 441, 400), (36, 272, 247), (37, 363, 325),
    (38, 344, 291), (39, 327, 302), (40, 361, 274), (41, 294, 211), (42, 333, 320), (43, 274, 259),
    (44, 351, 199), (45, 522, 521), (46, 424, 245), (47, 449, 307), (48, 497, 244), (51, 588, 521),
    (52, 329, 335), (53, 293, 248), (54, 426, 372), (55, 452, 413), (56, 275, 230), (57, 613, 414),
    (58, 361, 373), (59, 250, 253), (61, 208, 215), (62, 167, 175), (63, 256, 264), (64, 291, 213),
    (65, 218, 194), (66, 265, 170), (67, 183, 190), (68, 237, 245), (69, 203, 210), (94, 218, 198),
    (71, 327, 271), (72, 495, 360), (73, 345, 326), (74, 374, 323), (75, 485, 483), (76, 605, 602),
    (77, 474, 344), (78, 464, 365), (79, 404, 267), (95, 235, 245), (81, 156, 278), (82, 256, 354),
    (83, 189, 289), (84, 272, 295), (85, 346, 338), (86, 376, 363), (87, 131, 253), (88, 178, 283),
    (89, 744, 846), (90, 174, 267), (101, 493, 470), (102, 372, 308), (103, 429, 364), (104, 375, 308),
    (105, 404, 393), (106, 498, 368), (107, 442, 382), (108, 550, 388), (109, 591, 502), (110, 383, 312),
    (111, 403, 343), (112, 395, 382), (113, 431, 341), (121, 378, 401), (122, 417, 415), (123, 393, 401),
    (124, 331, 236), (125, 541, 537), (126, 308, 287), (127, 329, 328), (128, 319, 327), (129, 323, 331),
    (130, 348, 356),
];

/// `(stage, draws added at INTRO_TICK, at PLAY_TICK)`: what a stage draws on purpose beyond the 0.1.0 table,
/// recorded when it was added. Every other stage draws exactly its 0.1.0 count.
///
/// * Stage 92: the rippling floor (`SHAPE_STAGE134`, one mesh), which 0.1.0 did not draw at all.
/// * Night stages (21 to 27, 29, 45 to 47): one draw for the current frame of every window flipbook in
///   view, of the 23 to 33 a stage has. Stages 28 and 48 keep their diners among the foreground objects,
///   whose flipbooks the reference does not play, and are unchanged.
/// * Storm stages (81 to 90): one draw for the current frame of every fire in view, of 8.
#[rustfmt::skip]
const DRAWS_ADDED: &[(u32, usize, usize)] = &[
    (92, 1, 1), (21, 23, 16), (22, 4, 5), (23, 16, 10), (24, 7, 11), (25, 7, 10), (26, 14, 11), (27, 15, 9),
    (29, 13, 10), (45, 14, 9), (46, 28, 16), (47, 29, 18), (81, 0, 2), (82, 0, 2), (83, 0, 2), (84, 3, 4),
    (85, 3, 4), (86, 4, 4), (87, 0, 2), (88, 0, 2), (89, 0, 2), (90, 0, 2),
];

/// The most a stage may draw: its 0.1.0 row plus what [`DRAWS_ADDED`] records for it.
fn ceiling_of_0_1_0(row: &(u32, usize, usize)) -> (u32, usize, usize) {
    let added = DRAWS_ADDED.iter().find(|a| a.0 == row.0);
    let (intro, play) = added.map_or((0, 0), |a| (a.1, a.2));
    (row.0, row.1 + intro, row.2 + play)
}

/// `(stage, draws at INTRO_TICK, draws at PLAY_TICK)` as the stages are drawn now, outside particles
/// and mirrors: the same things as in [`DRAWS_0_1_0`] and [`DRAWS_ADDED`], in fewer calls. Since the
/// meshes of a model that share pipeline and material are uploaded as one (`mb-render`,
/// `Renderer::make_meshes`: the two halves of every shape, one per culled face, and shapes of one
/// material), a stage draws 15 to 50 % fewer calls than 0.1.0 did. Recorded then; a stage that draws
/// more than its row got slower, and one that draws more than 0.1.0 breaks the budget of the plan.
#[rustfmt::skip]
const DRAWS: &[(u32, usize, usize)] = &[
    (1, 213, 196), (2, 263, 234), (3, 232, 159), (4, 182, 189), (91, 155, 165), (5, 202, 186),
    (6, 239, 160), (7, 332, 330), (8, 219, 205), (9, 205, 208), (11, 197, 155), (12, 267, 235),
    (13, 208, 186), (14, 203, 210), (15, 195, 198), (16, 186, 174), (17, 235, 239), (18, 290, 288),
    (92, 153, 163), (21, 417, 411), (22, 236, 198), (23, 256, 205), (24, 381, 337), (25, 223, 184),
    (26, 242, 200), (27, 213, 152), (28, 319, 113), (29, 261, 200), (93, 293, 303), (31, 333, 333),
    (32, 156, 158), (33, 177, 152), (34, 222, 229), (35, 276, 253), (36, 166, 154), (37, 213, 193),
    (38, 207, 182), (39, 208, 196), (40, 242, 185), (41, 194, 152), (42, 221, 218), (43, 185, 181),
    (44, 232, 147), (45, 404, 400), (46, 331, 198), (47, 352, 243), (48, 352, 175), (51, 394, 358),
    (52, 189, 195), (53, 179, 154), (54, 278, 252), (55, 290, 269), (56, 167, 143), (57, 439, 312),
    (58, 218, 228), (59, 158, 161), (61, 137, 144), (62, 107, 117), (63, 187, 197), (64, 175, 142),
    (65, 134, 130), (66, 168, 114), (67, 119, 126), (68, 144, 154), (69, 134, 141), (94, 154, 144),
    (71, 190, 176), (72, 255, 193), (73, 200, 195), (74, 214, 188), (75, 260, 262), (76, 364, 367),
    (77, 248, 205), (78, 253, 216), (79, 219, 153), (95, 166, 176), (81, 117, 187), (82, 186, 234),
    (83, 146, 202), (84, 200, 201), (85, 239, 232), (86, 253, 243), (87, 105, 175), (88, 133, 191),
    (89, 383, 444), (90, 133, 179), (101, 305, 300), (102, 229, 200), (103, 236, 206), (104, 232, 200),
    (105, 244, 248), (106, 322, 248), (107, 291, 262), (108, 377, 270), (109, 373, 328), (110, 235, 206),
    (111, 252, 227), (112, 233, 235), (113, 271, 226), (121, 252, 274), (122, 289, 288), (123, 261, 268),
    (124, 234, 149), (125, 415, 418), (126, 180, 174), (127, 197, 197), (128, 188, 195), (129, 179, 186),
    (130, 199, 206),
];

/// The count a stage is held to: its row of [`DRAWS`].
fn expected_draws(stage: u32) -> (u32, usize, usize) {
    let row = DRAWS.iter().find(|row| row.0 == stage);
    *row.unwrap_or_else(|| panic!("no row for stage {stage}"))
}

/// Most draw calls the particles may add to a frame (sparks, stars, flash, sparkle, glow, confetti).
const PARTICLE_DRAW_BUDGET: usize = 12;

/// `(stage, mirror draws at INTRO_TICK, at PLAY_TICK)` with the Reflections option on, in course order:
/// every stage that has a mirror floor (`tests/mirrors.rs` says which and why), and what it adds to
/// their frames (`FrameStats::mirror_draws`: the scene once more from below the floor, less the stage's
/// models that the reference leaves out, plus one draw per mesh of the mirror models in view). Recorded
/// when the mirrors were added. Not part of the tables above: with the option off, and on every other
/// stage, it is zero.
#[rustfmt::skip]
const MIRROR_DRAWS: &[(u32, usize, usize)] = &[
    (91, 135, 144), (92, 134, 143), (21, 240, 300), (93, 274, 283), (48, 313, 138), (62, 75, 77),
    (94, 134, 127), (71, 114, 149), (72, 182, 166), (74, 65, 92), (75, 171, 218), (76, 249, 330),
    (77, 108, 180), (78, 138, 174), (95, 46, 46), (82, 117, 189), (83, 43, 131), (85, 50, 175),
    (86, 65, 98), (89, 323, 398), (101, 141, 197), (102, 139, 140), (103, 144, 142), (104, 105, 105),
    (105, 152, 209), (106, 208, 162), (108, 85, 129), (109, 164, 176), (110, 113, 125), (111, 136, 170),
    (112, 94, 124), (113, 156, 180), (121, 48, 71), (123, 31, 50), (125, 269, 288), (128, 34, 44),
    (129, 34, 89),
];

/// Bytes of the mirror's target under the 640 x 360 frame of these tests: half the frame each way,
/// colour and depth at four bytes a pixel. Not texture bytes (`Renderer::texture_bytes`, the budget of
/// stage 109 below): it is a render target that exists only on the mirror stages, and it follows the
/// frame's size (1.84 MB under 1280 x 720, 4.15 MB under 1920 x 1080).
const MIRROR_TARGET_BYTES: usize = 320 * 180 * 8;

/// The mirror exists exactly on the stages of [`MIRROR_DRAWS`], with a target of the recorded size.
fn assert_mirror_target(h: &Headless, stage: u32, reflections: bool) {
    let r = h.renderer();
    // (When the tables are printed rather than compared, the stages are taken as they come.)
    let listed = MIRROR_DRAWS.iter().any(|row| row.0 == stage);
    let expected = reflections && (listed || (printing() && r.mirror().is_some()));
    assert_eq!(r.mirror().is_some(), expected, "stage {stage}");
    let bytes = if expected { MIRROR_TARGET_BYTES } else { 0 };
    assert_eq!(r.mirror_bytes(), bytes, "stage {stage}");
    assert_eq!(r.mirror_size().is_some(), expected, "stage {stage}");
}

/// The draw calls of one frame, by what they are for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Frame {
    /// [`base_draws`]: what the 0.1.0 table holds.
    base: usize,
    /// `FrameStats::mirror_draws`.
    mirror: usize,
}

/// Start `stage` and count the draws of its frames at [`INTRO_TICK`] and [`PLAY_TICK`].
fn measure_stage(h: &mut Headless, id: u32, reflections: bool) -> [Frame; 2] {
    start_stage_with(h, id, reflections);
    assert_mirror_target(h, id, reflections);
    let mut frames = [Frame::default(); 2];
    for (slot, at) in [INTRO_TICK, PLAY_TICK].into_iter().enumerate() {
        while tick(h) < at {
            assert!(h.step(FRAME).unwrap(), "stage {id}");
            assert!(h.top().unwrap().play_view().is_some(), "stage {id} ended");
        }
        let stats = render(h);
        let particles = stats.instance_draws + stats.quad_draws;
        assert!(
            particles <= PARTICLE_DRAW_BUDGET,
            "stage {id} tick {at}: {stats:?}"
        );
        frames[slot] = Frame {
            base: base_draws(&stats),
            mirror: stats.mirror_draws,
        };
        // No mirror, no mirror draws: on the stages without one, and on the others with the option off.
        if h.renderer().mirror().is_none() {
            assert_eq!(
                (stats.mirror_draws, stats.mirror_surface_draws),
                (0, 0),
                "stage {id} tick {at}"
            );
        }
        if printing() {
            // Model draws alone, the one figure 0.1.0's own driver reports.
            let models = stats.draws - stats.instance_draws;
            eprintln!("MODELS {id} {at} {models}");
        }
    }
    assert_mirror_target(h, id, reflections);
    frames
}

#[test]
fn every_stage_draws_what_it_did_in_0_1_0_outside_the_particles() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    // As in a session: the title menu was on screen before the first stage.
    show_title(&mut h);
    let ids = course_stage_ids();
    let mut measured = Vec::new();
    let mut mirrors = Vec::new();
    for id in ids.iter().copied() {
        let frames = measure_stage(&mut h, id, true);
        measured.push((id, frames[0].base, frames[1].base));
        if h.renderer().mirror().is_some() {
            mirrors.push((id, frames[0].mirror, frames[1].mirror));
        }
    }
    h.shutdown();
    if printing() {
        for row in measured.chunks(6) {
            let cells: Vec<String> = row.iter().map(|r| format!("{r:?},")).collect();
            println!("    {}", cells.join(" "));
        }
        println!("    mirror draws:");
        for row in mirrors.chunks(6) {
            let cells: Vec<String> = row.iter().map(|r| format!("{r:?},")).collect();
            println!("    {}", cells.join(" "));
        }
        return;
    }
    // What the mirror costs each stage that has one, the dearest first: the draws of the frame without
    // it and with it, in the fly-in and in play, and the larger of the two increases.
    let mut costs: Vec<(usize, String)> = (mirrors.iter())
        .map(|row| {
            let base = measured.iter().find(|m| m.0 == row.0).unwrap();
            let percent = |base: usize, more: usize| (more * 100 + base / 2) / base.max(1);
            let worst = percent(base.1, row.1).max(percent(base.2, row.2));
            let line = format!(
                "stage {:3}: fly-in {:3} -> {:3} (+{:3} %), play {:3} -> {:3} (+{:3} %)",
                row.0,
                base.1,
                base.1 + row.1,
                percent(base.1, row.1),
                base.2,
                base.2 + row.2,
                percent(base.2, row.2)
            );
            (worst, line)
        })
        .collect();
    costs.sort_by_key(|c| std::cmp::Reverse(c.0));
    eprintln!(
        "{} stages with a mirror floor; its target is {MIRROR_TARGET_BYTES} bytes",
        costs.len()
    );
    for (_, line) in &costs {
        eprintln!("{line}");
    }
    // One more pass over about the scene less the stage's own models, cut to what the mirror models
    // in view can show: the mirror never doubles the draws of a frame.
    assert!(
        costs.iter().all(|c| c.0 < 100),
        "a mirror more than doubles a frame"
    );
    assert_eq!(mirrors, MIRROR_DRAWS, "mirror draws changed");
    assert_eq!(
        measured.len(),
        DRAWS_0_1_0.len(),
        "the table has a row for every course stage"
    );
    let mut over = Vec::new();
    for (now, then) in measured.iter().zip(DRAWS_0_1_0) {
        let expected = expected_draws(now.0);
        if *now != expected {
            over.push(format!(
                "stage {}: {now:?}, expected {expected:?} (0.1.0 had {then:?})",
                now.0
            ));
        }
        // Never more than 0.1.0 drew for the same picture.
        let ceiling = ceiling_of_0_1_0(then);
        assert!(
            now.0 == ceiling.0 && now.1 <= ceiling.1 && now.2 <= ceiling.2,
            "stage {}: {now:?} is more than 0.1.0's {ceiling:?}",
            now.0
        );
    }
    assert!(over.is_empty(), "draw calls changed:\n{}", over.join("\n"));
    let total = |rows: &mut dyn Iterator<Item = (usize, usize)>| {
        rows.fold((0, 0), |sum, row| (sum.0 + row.0, sum.1 + row.1))
    };
    let now = total(&mut measured.iter().map(|m| (m.1, m.2)));
    let then = total(&mut DRAWS_0_1_0.iter().map(|m| (m.1, m.2)));
    eprintln!(
        "all stages, fly-in / play: {} / {} draws; 0.1.0 drew {} / {}",
        now.0, now.1, then.0, then.1
    );
}

/// Play floor 1 of a course with the autopilot, from the fly-in to the end of the goal sequence, rendering
/// every frame; `each` sees every frame's statistics.
fn play_floor_one(h: &mut Headless, d: Difficulty, each: impl FnMut(&Headless, &FrameStats)) {
    play_floor_one_from(h, d, |_| {}, each)
}

/// [`play_floor_one`]; `loaded` sees the game when the stage is loaded and nothing of it is drawn yet.
fn play_floor_one_from(
    h: &mut Headless,
    d: Difficulty,
    loaded: impl FnOnce(&Headless),
    mut each: impl FnMut(&Headless, &FrameStats),
) {
    h.shutdown();
    let menu =
        Menu::new(mb_app::headless::unlocked_storage()).with_autostart(Some(Autostart::Course(d)));
    h.push(Box::new(menu)).unwrap();
    assert!(h.step(FRAME).unwrap());
    let stage = h.top().unwrap().play_view().unwrap().request.stage_id;
    loaded(h);
    let mut pilot = None;
    let mut goal = false;
    for frame in 0..6000 {
        assert!(h.pilot(&mut pilot, true), "stage {stage}: left at {frame}");
        assert!(h.step(FRAME).unwrap());
        let Some(view) = h.top().unwrap().play_view() else {
            break;
        };
        // The next floor is another load: stop where this one ends.
        if view.request.stage_id != stage {
            break;
        }
        goal |= view.sim.phase == Phase::GoalSequence;
        let stats = render(h);
        each(h, &stats);
    }
    assert!(
        goal,
        "the autopilot did not reach the goal of stage {stage}"
    );
}

/// A landing, rolling at speed and a goal on the first floor of two courses: the particles never take
/// more than their budget, and at their peak they take what was measured when they were built (five
/// confetti colours, the two meshes of the star, glow, flash, streaks, sparkles).
#[test]
fn particles_add_at_most_twelve_draws_at_their_peak() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    for d in [Difficulty::Beginner, Difficulty::Advanced] {
        let (mut peak, mut most_instances, mut most_quads) = (0, 0, 0);
        play_floor_one(&mut h, d, |_, stats| {
            let particles = stats.instance_draws + stats.quad_draws;
            assert!(particles <= PARTICLE_DRAW_BUDGET, "{d:?}: {stats:?}");
            peak = peak.max(particles);
            most_instances = most_instances.max(stats.instances);
            most_quads = most_quads.max(stats.quads);
        });
        eprintln!(
            "{d:?} floor 1: particles peak at {peak} draws, {most_instances} instances, {most_quads} quads"
        );
        assert!(peak >= 6, "{d:?}: the goal alone is six draws, saw {peak}");
        assert!(most_instances >= 160, "{d:?}: confetti is 160 pieces");
        assert!(most_quads >= 4, "{d:?}: no streak or sparkle");
    }
    h.shutdown();
}

/// Nothing is created on the GPU while a stage is played: not by the stage, the HUD or the particles, from
/// the first frame of the fly-in through the landing burst, the sparks and the goal with its confetti.
///
/// Everything the HUD draws with is made when the stage is entered (`UiAssets::warm`), so the count is
/// taken when the load ends and holds from the first frame drawn.
///
/// The buffer all objects share is sized by the load too (`Renderer::reserve_objects`), so this holds
/// for the first stage of a session. Only the depth texture of a window that never drew a frame is made
/// by the first frame; the title menu, which every session shows before a stage, has drawn one.
#[test]
fn no_gpu_object_is_created_while_a_stage_is_played() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    let mut frames = 0;
    let mut start = None;
    let (mut effects, mut confetti) = (0, 0);
    show_title(&mut h);
    let mut loaded = None;
    play_floor_one_from(
        &mut h,
        Difficulty::Beginner,
        |h| loaded = Some(h.renderer().counters()),
        |h, _| {
            frames += 1;
            let r = h.renderer();
            let now = (r.counters(), r.texture_count(), r.object_count());
            if frames == 1 {
                start = Some(now);
            }
            if let Some(start) = start {
                assert_eq!(now, start, "frame {frames} created something");
            }
            let rt = &h.top().unwrap().play_view().unwrap().sim.runtime;
            effects = effects.max(rt.effects.len());
            confetti = confetti.max(rt.confetti.len());
        },
    );
    // The first frame made nothing either: the count is the one the load ended with.
    assert_eq!(
        start.unwrap().0,
        loaded.unwrap(),
        "the first frame created something"
    );
    assert!(frames > 400, "{frames} frames");
    assert!(
        effects >= 20 && confetti >= 160,
        "{effects} effects, {confetti} confetti"
    );
    h.shutdown();
}

/// Decoded texture bytes version 0.1.0 uploaded for stage 109, the heaviest (`docs/WEB.md`: 16.6 MB).
const STAGE_109_TEXTURE_BYTES_0_1_0: usize = 16_617_412;
/// What the textures of a stage may grow by over all of 0.2.0.
const TEXTURE_GROWTH_BUDGET: usize = 1_000_000;

#[test]
fn the_heaviest_stage_uploads_no_more_texture_bytes_than_its_budget() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    start_stage(&mut h, 109);
    let load = mb_app::perf::last_load().expect("a stage was loaded");
    assert_eq!(load.stage_id, 109);
    let stats = h.renderer().load_stats;
    assert_eq!(stats.texture_bytes, load.texture_bytes);
    eprintln!(
        "stage 109: {} textures, {} bytes ({:.2} MB); 0.1.0 had {} bytes",
        stats.textures,
        stats.texture_bytes,
        stats.texture_bytes as f64 / 1e6,
        STAGE_109_TEXTURE_BYTES_0_1_0
    );
    h.shutdown();
    if printing() {
        return;
    }
    assert!(
        stats.texture_bytes >= STAGE_109_TEXTURE_BYTES_0_1_0,
        "fewer bytes than 0.1.0: lower the recorded figure"
    );
    let growth = stats.texture_bytes - STAGE_109_TEXTURE_BYTES_0_1_0;
    assert!(
        growth <= TEXTURE_GROWTH_BUDGET,
        "stage 109 uploads {growth} texture bytes more than 0.1.0"
    );
}

/// The window and fire flipbooks bring textures 0.1.0 never uploaded (a frame each: about 2.2 MB for the
/// night background, 2.1 MB for the storm). They must not make a night or storm stage the heaviest: no
/// such stage may upload more than the budget of stage 109 allows.
#[test]
fn flipbook_stages_upload_less_than_the_heaviest_stage_may() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    let limit = STAGE_109_TEXTURE_BYTES_0_1_0 + TEXTURE_GROWTH_BUDGET;
    let mut most = (0, 0);
    for stage in DRAWS_ADDED.iter().map(|row| row.0) {
        start_stage(&mut h, stage);
        let bytes = h.renderer().load_stats.texture_bytes;
        if printing() {
            println!("stage {stage}: {bytes} texture bytes");
        }
        assert!(
            bytes <= limit,
            "stage {stage} uploads {bytes} texture bytes"
        );
        if bytes > most.1 {
            most = (stage, bytes);
        }
    }
    eprintln!(
        "heaviest stage with a flipbook or the wave: {} with {} bytes ({:.2} MB)",
        most.0,
        most.1,
        most.1 as f64 / 1e6
    );
    h.shutdown();
}

/// With the Reflections option off every mirror stage is what it was before the mirrors: the draws of
/// the table, no mirror draw, no mirror model, no target, the same textures. With it on, the frame's own
/// draws are the same and the mirror's come on top.
#[test]
fn with_reflections_off_the_mirror_stages_cost_what_they_did() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    show_title(&mut h);
    for row in MIRROR_DRAWS {
        let stage = row.0;
        let expected = expected_draws(stage);
        let off = measure_stage(&mut h, stage, false);
        let (objects, textures) = (h.renderer().object_count(), h.renderer().load_stats);
        assert_eq!((stage, off[0].base, off[1].base), expected);
        for frame in off {
            assert_eq!(frame.mirror, 0, "stage {stage}");
        }
        let view = h.top().unwrap().play_view().unwrap();
        assert!(view.scene.scene.mirror.is_none(), "stage {stage}");
        assert!(!view.request.settings.reflections);

        // On: the same frame's own draws, and the mirror's on top of them.
        let on = measure_stage(&mut h, stage, true);
        assert_eq!((stage, on[0].base, on[1].base), expected);
        assert_eq!(
            (on[0].mirror, on[1].mirror),
            (row.1, row.2),
            "stage {stage}"
        );
        // What the option adds besides draws: an object for each mirror model (the wave's floor is
        // there anyway), no texture, and the target, which is no texture of the stage.
        let view = h.top().unwrap().play_view().unwrap();
        let mirror = view.scene.scene.mirror.as_ref().expect("a mirror");
        let models = if stage == 92 {
            0
        } else {
            mirror.surfaces().count()
        };
        assert!(stage == 92 || models > 0, "stage {stage}");
        assert_eq!(
            h.renderer().object_count() - objects,
            models,
            "stage {stage}"
        );
        assert_eq!(
            h.renderer().load_stats.texture_bytes,
            textures.texture_bytes,
            "stage {stage}"
        );
    }
    h.shutdown();
}

fn created(after: GpuCounters, before: GpuCounters) -> GpuCounters {
    GpuCounters {
        buffers: after.buffers - before.buffers,
        textures: after.textures - before.textures,
        pipelines: after.pipelines - before.pipelines,
        bind_groups: after.bind_groups - before.bind_groups,
        samplers: after.samplers - before.samplers,
    }
}

/// A mirror stage creates nothing on the GPU while it is played either: the mirror's buffers, bind
/// groups and target are made by the load, the plane that rides on an anim group (stage 103) and the
/// tilt of the stage only change uniforms. Played here: the wave, the two first mirrors, a bonus floor,
/// an ice floor with ten mirror models, and a space and a master floor with mirror models on 15 and 24
/// anim groups. A resize of the window replaces the frame's depth texture and
/// the mirror's target (two textures and the bind group that reads them) in the first frame after it,
/// and play after that creates nothing again.
#[test]
fn no_gpu_object_is_created_while_a_mirror_stage_is_played_or_after_a_resize() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    // Steer in a slow circle, so the stage tilts and the ball moves over the mirror.
    let play = |h: &mut Headless, frames: u32, what: &str| {
        let start = (
            h.renderer().counters(),
            h.renderer().texture_count(),
            h.renderer().object_count(),
        );
        let (mut mirrored, mut uploaded) = (0, 0);
        for frame in 0..frames {
            let a = frame as f32 * 0.02;
            h.input.set_axis(Axis::MoveX, a.cos() * 0.4);
            h.input.set_axis(Axis::MoveY, a.sin() * 0.4);
            assert!(h.step(FRAME).unwrap(), "{what}");
            assert!(h.top().unwrap().play_view().is_some(), "{what}: left");
            let stats = render(h);
            mirrored += (stats.mirror_draws > 0) as u32;
            uploaded += stats.upload_bytes;
            let r = h.renderer();
            let now = (r.counters(), r.texture_count(), r.object_count());
            assert_eq!(now, start, "{what}: frame {frame} created something");
        }
        h.input.set_axis(Axis::MoveX, 0.0);
        h.input.set_axis(Axis::MoveY, 0.0);
        assert!(uploaded > 0, "{what}: nothing moved");
        mirrored
    };
    show_title(&mut h);
    for stage in [92u32, 102, 103, 91, 77, 106, 121] {
        assert!(MIRROR_DRAWS.iter().any(|row| row.0 == stage));
        start_stage(&mut h, stage);
        // The load made the mirror with its target; the first frame makes nothing.
        let loaded = h.renderer().counters();
        assert_mirror_target(&h, stage, true);
        render(&mut h);
        assert_eq!(
            h.renderer().counters(),
            loaded,
            "stage {stage}: first frame"
        );
        let frames = 520;
        let mirrored = play(&mut h, frames, &format!("stage {stage}"));
        assert!(
            mirrored > frames / 2,
            "stage {stage}: the mirror showed in {mirrored} of {frames} frames"
        );

        // The window is resized: the next frame replaces what depends on the frame's size.
        let before = h.renderer().counters();
        h.resize((800, 450));
        assert!(h.step(FRAME).unwrap());
        render(&mut h);
        let made = created(h.renderer().counters(), before);
        let expect = GpuCounters {
            textures: 3,
            bind_groups: 1,
            ..GpuCounters::default()
        };
        assert_eq!(made, expect, "stage {stage}: the resize");
        assert_eq!(h.renderer().mirror_size(), Some((400, 225)));
        assert_eq!(h.renderer().mirror_bytes(), 400 * 225 * 8);
        let mirrored = play(&mut h, 200, &format!("stage {stage} after the resize"));
        // (Where the ball has rolled off by now, the mirror may be out of sight.)
        assert!(
            !matches!(stage, 92 | 102 | 103) || mirrored > 0,
            "stage {stage}: {mirrored}"
        );
        // Back to the size of these tests, with a frame drawn at it, as a window always has: the
        // target a load makes is sized for the frame last drawn.
        h.resize((640, 360));
        assert!(h.step(FRAME).unwrap());
        render(&mut h);
    }
    h.shutdown();
}
