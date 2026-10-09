//! Regression budgets (`docs/PARITY_PLAN.md`, "Regression budgets as tests"): what a frame and a stage may
//! cost, counted rather than timed so the tests do not depend on the load of the machine.
//!
//! * draw calls per stage outside the particles equal the recorded 0.1.0 table; the particles add at most 12;
//! * no GPU object is created while a stage is played;
//! * the textures of the heaviest stage stay within 1 MB of what 0.1.0 uploaded.
//!
//! The pack's size and file count are in `tests/pack.rs`. Like the other tests these need the developer's
//! own disc image and a GPU adapter, and skip without either.
//!
//! `MB_BUDGET_PRINT=1 cargo test -p mb-app --release --test budgets -- --nocapture` prints the tables as
//! measured instead of comparing them, for the day a number is changed on purpose.

use std::path::PathBuf;
use std::sync::Mutex;

use mb_app::content::{course_stage_ids, open_disc_image};
use mb_app::headless::{Headless, FRAME};
use mb_app::menu::{Autostart, Menu};
use mb_engine::Phase;
use mb_game::Difficulty;
use mb_render::FrameStats;

static GPU: Mutex<()> = Mutex::new(());

fn find_disc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MB_DISC") {
        return Some(PathBuf::from(p));
    }
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    std::fs::read_dir(parent).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let image = [".rvz", ".iso", ".gcm"].iter().any(|e| name.ends_with(e));
        (name.starts_with("super monkey ball") && image).then(|| entry.path())
    })
}

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
    h.shutdown();
    let menu = Menu::new(mb_app::headless::unlocked_storage())
        .with_autostart(Some(Autostart::Stage(stage)));
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

/// The draw calls of a frame that are not particles: stage, backdrop, ball, HUD. This is the path 0.1.0
/// had, so it is what the table below holds.
fn base_draws(stats: &FrameStats) -> usize {
    stats.total_draws() - stats.instance_draws - stats.quad_draws
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

/// Most draw calls the particles may add to a frame (sparks, stars, flash, sparkle, glow, confetti).
const PARTICLE_DRAW_BUDGET: usize = 12;

#[test]
fn every_stage_draws_what_it_did_in_0_1_0_outside_the_particles() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    let ids = course_stage_ids();
    let mut measured = Vec::new();
    for id in ids.iter().copied() {
        start_stage(&mut h, id);
        let mut counts = [0usize; 2];
        for (slot, at) in [INTRO_TICK, PLAY_TICK].into_iter().enumerate() {
            while tick(&h) < at {
                assert!(h.step(FRAME).unwrap(), "stage {id}");
                assert!(h.top().unwrap().play_view().is_some(), "stage {id} ended");
            }
            let stats = render(&mut h);
            let particles = stats.instance_draws + stats.quad_draws;
            assert!(
                particles <= PARTICLE_DRAW_BUDGET,
                "stage {id} tick {at}: {stats:?}"
            );
            counts[slot] = base_draws(&stats);
            if printing() {
                // Model draws alone, the one figure 0.1.0's own driver reports.
                eprintln!("MODELS {id} {at} {}", stats.draws - stats.instance_draws);
            }
        }
        measured.push((id, counts[0], counts[1]));
    }
    h.shutdown();
    if printing() {
        for row in measured.chunks(6) {
            let cells: Vec<String> = row.iter().map(|r| format!("{r:?},")).collect();
            println!("    {}", cells.join(" "));
        }
        return;
    }
    assert_eq!(
        measured.len(),
        DRAWS_0_1_0.len(),
        "the table has a row for every course stage"
    );
    let mut over = Vec::new();
    for (now, then) in measured.iter().zip(DRAWS_0_1_0) {
        if now != then {
            over.push(format!("stage {}: {:?}, 0.1.0 had {:?}", now.0, now, then));
        }
    }
    assert!(over.is_empty(), "draw calls changed:\n{}", over.join("\n"));
}

/// Play floor 1 of a course with the autopilot, from the fly-in to the end of the goal sequence, rendering
/// every frame; `each` sees every frame's statistics.
fn play_floor_one(h: &mut Headless, d: Difficulty, mut each: impl FnMut(&Headless, &FrameStats)) {
    h.shutdown();
    let menu =
        Menu::new(mb_app::headless::unlocked_storage()).with_autostart(Some(Autostart::Course(d)));
    h.push(Box::new(menu)).unwrap();
    assert!(h.step(FRAME).unwrap());
    let stage = h.top().unwrap().play_view().unwrap().request.stage_id;
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
/// the first frames of the fly-in through the landing burst, the sparks and the goal with its confetti.
#[test]
fn no_gpu_object_is_created_while_a_stage_is_played() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = headless() else {
        return;
    };
    let mut frames = 0;
    let mut start = None;
    let (mut effects, mut confetti) = (0, 0);
    play_floor_one(&mut h, Difficulty::Beginner, |h, _| {
        frames += 1;
        let r = h.renderer();
        let now = (r.counters(), r.texture_count(), r.object_count());
        // The first frames may still create what only a first frame needs.
        if frames == 3 {
            start = Some(now);
        }
        if let Some(start) = start {
            assert_eq!(now, start, "frame {frames} created something");
        }
        let rt = &h.top().unwrap().play_view().unwrap().sim.runtime;
        effects = effects.max(rt.effects.len());
        confetti = confetti.max(rt.confetti.len());
    });
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
