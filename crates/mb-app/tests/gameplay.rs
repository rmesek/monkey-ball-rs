//! End-to-end tests: the real menu and gameplay states on a `GameStack`, a real `Renderer` on an offscreen
//! target, input injected through `InputState` exactly as a keyboard or gamepad would.
//!
//! They need the user's own disc image (`MB_DISC`, or `Super Monkey Ball*.rvz|iso|gcm` next to the
//! repository) and a GPU adapter (a software one is fine); without either they skip. Nothing read from the
//! disc is written anywhere.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use mb_app::autopilot::Autopilot;
use mb_app::content::{open_disc_image, Content};
use mb_app::headless::{Headless, FRAME};
use mb_app::input::{Action, Axis};
use mb_app::menu::Menu;
use mb_engine::Phase;
use mb_game::{Difficulty, MemoryStorage, Profile, Storage};
use mb_render::glam::Vec3;
use mb_render::Camera;

/// One GPU device at a time: the tests are independent but there is no point in fighting over the adapter.
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

/// A save that the test can read back while the menu owns the other handle.
#[derive(Clone, Default)]
struct SharedStorage(Rc<RefCell<MemoryStorage>>);

impl Storage for SharedStorage {
    fn read(&self, key: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.0.borrow().read(key)
    }
    fn write(&mut self, key: &str, data: &[u8]) -> std::io::Result<()> {
        self.0.borrow_mut().write(key, data)
    }
    fn remove(&mut self, key: &str) -> std::io::Result<()> {
        self.0.borrow_mut().remove(key)
    }
}

impl SharedStorage {
    fn profile(&self) -> Profile {
        Profile::load(&mut self.clone()).0
    }

    fn unlocked() -> Self {
        let mut s = Self::default();
        let mut profile = Profile::default();
        profile.settings.unlock_all = true;
        profile.save(&mut s).unwrap();
        s
    }
}

/// The disc and a headless game at the title menu, or `None` (with a note) when the machine cannot run it.
fn start(storage: &SharedStorage) -> Option<Headless> {
    let Some(disc) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return None;
    };
    let content: Content = match open_disc_image(&disc) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {}: {e}", disc.display());
            return None;
        }
    };
    let mut h = match Headless::new(&content, (640, 360), false) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped: no GPU adapter ({e})");
            return None;
        }
    };
    h.push(Box::new(Menu::new(Box::new(storage.clone()))))
        .expect("menu");
    assert!(h.step(FRAME).unwrap());
    assert_eq!(h.status(), "menu: title");
    Some(h)
}

/// Title -> Course Play -> Beginner with the keyboard actions.
fn start_beginner_course(h: &mut Headless) {
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: course select");
    h.tap(Action::Confirm).unwrap();
    let view = h.top().unwrap().play_view().expect("floor 1 is running");
    assert_eq!((view.request.stage_id, view.request.floor), (1, 1));
    assert_eq!(view.request.lives, 3);
    assert!(!view.request.retry);
}

/// `(stage id, floor, sim tick)` of the stage being played.
fn playing(h: &Headless) -> Option<(u32, u32, u32)> {
    let view = h.top()?.play_view()?;
    Some((view.request.stage_id, view.request.floor, view.sim.tick))
}

/// Step until `done` says so; panics after `limit` frames.
fn run_until(h: &mut Headless, limit: u32, what: &str, mut done: impl FnMut(&Headless) -> bool) {
    for _ in 0..limit {
        assert!(
            h.step(FRAME).unwrap(),
            "the game quit while waiting for {what}"
        );
        if done(h) {
            return;
        }
    }
    panic!(
        "{what} did not happen within {limit} frames; at {:?}",
        h.status()
    );
}

#[test]
fn course_goal_advances_to_the_next_floor_with_a_score() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let Some(mut h) = start(&storage) else {
        return;
    };
    start_beginner_course(&mut h);
    // The stage runs on the game's own trig tables, read from the disc's main.dol.
    assert!(mb_engine::tables::tables().exact);

    // The waypoint autopilot plays floor 1: bananas first, then through the goal.
    let mut pilot = None;
    let (mut saw_goal, mut bananas) = (false, 0);
    for frame in 0..4000 {
        assert!(
            h.pilot(&mut pilot, false),
            "left the stage at frame {frame}"
        );
        assert!(h.step(FRAME).unwrap());
        // Render now and then: the scene and HUD must draw at every phase without errors.
        if frame % 200 == 0 {
            let rgba = h.render().unwrap();
            assert_eq!(rgba.len(), 640 * 360 * 4);
        }
        let view = h.top().unwrap().play_view().expect("a stage is running");
        if view.request.floor == 2 {
            break;
        }
        if view.sim.phase == Phase::GoalSequence {
            saw_goal = true;
            bananas = view.sim.ball.bananas;
            assert_eq!(view.banner, Some("GOAL"));
        }
    }
    assert!(saw_goal, "the autopilot never reached the goal");
    assert_eq!(bananas, 4, "stage 1 has four bananas on the way");

    let view = h.top().unwrap().play_view().unwrap();
    assert_eq!((view.request.stage_id, view.request.floor), (2, 2));
    assert!(!view.request.retry);
    assert_eq!(view.request.lives, 3);
    // Four bananas plus the time bonus.
    assert!(view.request.score > 400, "score {}", view.request.score);
    assert_eq!(view.sim.phase, Phase::Intro);

    // Progress was saved when the floor was cleared.
    let progress = storage.profile().progress;
    let beginner = progress.course(Difficulty::Beginner);
    assert!(beginner.reached.contains(&1) && beginner.reached.contains(&2));
    assert_eq!(beginner.best_score, view.request.score);
    assert!(progress.stage_best(1).best_frames > 0);
    h.shutdown();
}

#[test]
fn fall_out_costs_a_life_and_retries_the_floor_until_game_over() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let Some(mut h) = start(&storage) else {
        return;
    };
    start_beginner_course(&mut h);

    // Holding "forward" on the keyboard rolls straight through the goal of floor 1 ...
    h.input.set_action(Action::MoveForward, true);
    run_until(&mut h, 2000, "floor 2", |h| {
        playing(h).is_some_and(|p| p.1 == 2)
    });
    // ... and straight off the edge of floor 2.
    for lives_left in [2u32, 1] {
        let mut fell = false;
        run_until(&mut h, 2000, "the retry", |h| {
            let view = h.top().unwrap().play_view().expect("still in the course");
            fell |= view.sim.phase == Phase::FallOut;
            if view.sim.phase == Phase::FallOut && (10..200).contains(&view.sim.ringout_timer) {
                assert_eq!(view.banner, Some("FALL OUT"));
            }
            view.request.lives == lives_left
        });
        assert!(fell, "the ball should have fallen out");
        let view = h.top().unwrap().play_view().unwrap();
        assert_eq!(
            (view.request.stage_id, view.request.floor),
            (2, 2),
            "same floor again"
        );
        assert!(view.request.retry);
        // Retries use the short intro.
        assert!(view.sim.intro_timer <= 120);
    }
    // The third fall is the last life.
    run_until(&mut h, 2000, "game over", |h| {
        h.status() == "menu: game over"
    });
    h.input.set_action(Action::MoveForward, false);
    h.step(FRAME).unwrap();

    // Continue: lives are back, the score starts over, same floor.
    h.tap(Action::Confirm).unwrap();
    let view = h
        .top()
        .unwrap()
        .play_view()
        .expect("continue restarts the floor");
    assert_eq!((view.request.floor, view.request.lives), (2, 3));
    assert_eq!(view.request.score, 0);
    assert!(view.request.retry);
    h.shutdown();
}

#[test]
fn practice_quit_from_pause_returns_to_the_menu() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    // Title -> Practice -> Beginner -> third stage.
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: practice");
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: stage select");
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(playing(&h).map(|p| (p.0, p.1)), Some((3, 3)));

    // Let the intro run, then pause: the simulation must stand still while paused.
    for _ in 0..150 {
        h.step(FRAME).unwrap();
    }
    h.tap(Action::Pause).unwrap();
    let tick = {
        let view = h.top().unwrap().play_view().unwrap();
        assert!(view.paused);
        view.sim.tick
    };
    for _ in 0..30 {
        h.step(FRAME).unwrap();
    }
    assert_eq!(playing(&h).unwrap().2, tick, "paused sim must not advance");
    assert_eq!(h.render().unwrap().len(), 640 * 360 * 4);

    // Continue resumes ...
    h.tap(Action::Confirm).unwrap();
    for _ in 0..10 {
        h.step(FRAME).unwrap();
    }
    let view = h.top().unwrap().play_view().unwrap();
    assert!(!view.paused && view.sim.tick > tick);

    // ... retry restarts the stage in place (practice has no lives) ...
    h.tap(Action::Pause).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    let view = h.top().unwrap().play_view().unwrap();
    assert!(!view.paused && view.sim.tick < 5 && view.request.stage_id == 3);

    // ... and quit goes back to the stage list with the stage still selected.
    h.tap(Action::Cancel).unwrap();
    assert!(h.top().unwrap().play_view().unwrap().paused);
    h.tap(Action::LookUp).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: stage select");
    assert_eq!(h.depth(), 1);
    // Nothing was recorded for a quit.
    assert_eq!(storage.profile().progress.stage_best(3).clears, 0);

    // Back out to the title with Cancel.
    h.tap(Action::Cancel).unwrap();
    assert_eq!(h.status(), "menu: practice");
    h.tap(Action::Cancel).unwrap();
    assert_eq!(h.status(), "menu: title");
    h.shutdown();
}

#[test]
fn analog_stick_and_pointer_drive_the_same_states() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let Some(mut h) = start(&storage) else {
        return;
    };
    // Click the first row (Course Play), then the first course, with the pointer.
    let layout = h.layout();
    for expected in ["menu: course select", ""] {
        let rows = mb_app::widgets::ListLayout::column(&layout, 4, 0.24);
        let target = rows.item_rect(0, 0.0).center();
        h.input.set_pointer(Some(target));
        h.input.set_pointer_down(true);
        h.step(FRAME).unwrap();
        h.input.set_pointer_down(false);
        h.step(FRAME).unwrap();
        if !expected.is_empty() {
            assert_eq!(h.status(), expected);
        }
    }
    assert_eq!(
        playing(&h).map(|p| p.0),
        Some(1),
        "clicking started floor 1"
    );

    // A gamepad stick pushed forward finishes floor 1 just like the key does.
    h.input.set_axis(Axis::MoveY, 1.0);
    run_until(&mut h, 2000, "floor 2", |h| {
        playing(h).is_some_and(|p| p.1 == 2)
    });
    h.shutdown();
}

/// Every floor of every course starts, runs its first second and draws a frame.
#[test]
fn every_course_stage_loads_and_renders() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    h.shutdown();
    let mut ids: Vec<u32> = Difficulty::ALL
        .into_iter()
        .flat_map(|d| mb_game::course::stage_list(d).into_iter().map(|e| e.id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert!(ids.len() > 100, "{} course stages", ids.len());
    // Unoptimized builds decode textures slowly: they sample the list, release builds run all of it.
    let stride = if cfg!(debug_assertions) { 9 } else { 1 };
    for id in ids.into_iter().step_by(stride) {
        let menu = Menu::new(Box::new(storage.clone()))
            .with_autostart(Some(mb_app::menu::Autostart::Stage(id)));
        h.push(Box::new(menu))
            .unwrap_or_else(|e| panic!("stage {id}: {e}"));
        for _ in 0..60 {
            assert!(h.step(FRAME).unwrap_or_else(|e| panic!("stage {id}: {e}")));
        }
        let view = h
            .top()
            .unwrap()
            .play_view()
            .unwrap_or_else(|| panic!("stage {id} is not running"));
        assert_eq!(view.request.stage_id, id);
        assert_eq!(view.sim.tick, 59, "stage {id}");
        let rgba = h.render().unwrap();
        // Not a blank frame: the HUD and stage give it more than one colour.
        let (pixels, _) = rgba.as_chunks::<4>();
        assert!(
            pixels.iter().any(|p| *p != pixels[0]),
            "stage {id} rendered a flat frame"
        );
        h.shutdown();
    }
}

/// What the rendered frames of a stage showed of the particles and the shadow.
#[derive(Default)]
struct Seen {
    streaks: bool,
    stars: bool,
    flash: bool,
    sparkles: bool,
    confetti: bool,
    shadow_on: bool,
    shadow_off: bool,
    /// Most draw calls the particles took in one frame.
    peak_draws: usize,
}

/// Render the frame and compare what the renderer drew with what the simulation holds.
fn look(h: &mut Headless, seen: &mut Seen) {
    use mb_engine::BallEffectKind as Kind;
    h.render().unwrap();
    let stats = h.renderer().stats;
    let view = h.top().unwrap().play_view().expect("a stage is running");
    let rt = &view.sim.runtime;
    let has = |kind: Kind| rt.effects.iter().any(|e| e.kind == kind);
    // The shadow follows the ball: none while it is not there (the fly-in).
    let visible = view.sim.ball.flags & mb_engine::constants::ball_flags::INVISIBLE == 0;
    assert_eq!(view.shadow, visible, "tick {}", view.sim.tick);
    seen.shadow_on |= view.shadow;
    seen.shadow_off |= !view.shadow;
    if rt.effects.is_empty() && rt.confetti.is_empty() {
        assert_eq!((stats.instances, stats.quads), (0, 0));
        assert_eq!((stats.instance_draws, stats.quad_draws), (0, 0));
    }
    // Every piece of confetti is an instance, in one draw per colour.
    if !rt.confetti.is_empty() {
        assert!(stats.instances >= rt.confetti.len(), "{stats:?}");
        assert!(stats.instance_draws >= 5, "{stats:?}");
        seen.confetti = true;
    }
    // A star is two instanced draws (its model has two meshes), a flash one.
    if has(Kind::ColiStar) && stats.instance_draws >= 2 {
        seen.stars = true;
    }
    if has(Kind::ColiFlash) && stats.instances > rt.confetti.len() {
        seen.flash = true;
    }
    let sparkles = rt.effects.iter().filter(|e| e.kind == Kind::Levitate);
    let sparkles = sparkles.count();
    if sparkles > 0 {
        // Four quads each, on top of whatever streaks there are.
        assert!(stats.quads >= 4 * sparkles, "{stats:?}");
        seen.sparkles = true;
    } else if has(Kind::Coli) && stats.quads > 0 {
        assert_eq!(stats.quad_draws, 1, "all streaks are one draw");
        seen.streaks = true;
    }
    let draws = stats.instance_draws + stats.quad_draws;
    assert!(draws <= 12, "particles took {draws} draws: {stats:?}");
    seen.peak_draws = seen.peak_draws.max(draws);
}

/// Sparks while rolling, stars and a flash where the ball lands, confetti and sparkles at the goal, and
/// the shadow under the ball: the simulation has them and the renderer draws them, in a handful of draws.
/// A pause shows them standing still.
#[test]
fn particles_and_the_shadow_are_drawn_after_a_collision_and_a_goal() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let Some(mut h) = start(&storage) else {
        return;
    };
    start_beginner_course(&mut h);
    let mut pilot = None;
    let mut seen = Seen::default();
    let mut paused_once = false;
    for frame in 0..4000 {
        assert!(
            h.pilot(&mut pilot, false),
            "left the stage at frame {frame}"
        );
        assert!(h.step(FRAME).unwrap());
        let view = h.top().unwrap().play_view().expect("a stage is running");
        if view.request.floor == 2 {
            break;
        }
        let busy = view.sim.runtime.effects.len() >= 20 && view.sim.phase == Phase::Playing;
        look(&mut h, &mut seen);
        if busy && !paused_once {
            paused_once = true;
            // Pause: the same particles are queued for every frame, so the picture stands still and
            // nothing is uploaded for them.
            h.tap(Action::Pause).unwrap();
            h.render().unwrap();
            let before = h.renderer().stats;
            assert!(before.quads >= 10 && before.instances > 0, "{before:?}");
            for _ in 0..5 {
                h.step(FRAME).unwrap();
                assert!(h.top().unwrap().play_view().unwrap().paused);
                h.render().unwrap();
                let now = h.renderer().stats;
                assert_eq!(
                    (now.quads, now.instances, now.quad_draws, now.instance_draws),
                    (
                        before.quads,
                        before.instances,
                        before.quad_draws,
                        before.instance_draws
                    )
                );
                assert_eq!(now.upload_bytes, 0, "a paused frame changes nothing");
            }
            h.tap(Action::Pause).unwrap();
            assert!(!h.top().unwrap().play_view().unwrap().paused);
        }
    }
    assert!(paused_once, "never saw twenty effects at once");
    assert!(seen.streaks, "no streak was drawn while rolling");
    assert!(
        seen.flash && seen.stars,
        "no flash or star where the ball landed"
    );
    assert!(
        seen.confetti && seen.sparkles,
        "no confetti or sparkle at the goal"
    );
    assert!(seen.shadow_on && seen.shadow_off);
    // Five confetti colours, the two meshes of the star, glow, flash, streaks and sparkles.
    assert!(
        (6..=11).contains(&seen.peak_draws),
        "peak {}",
        seen.peak_draws
    );
    h.shutdown();
}

/// Ticks from the start of the stage being played until its intro is over.
fn intro_ticks(h: &mut Headless) -> u32 {
    run_until(h, 500, "the end of the intro", |h| {
        h.top().unwrap().play_view().unwrap().sim.phase != Phase::Intro
    });
    h.top().unwrap().play_view().unwrap().sim.tick
}

/// Holding the action button runs the fly-in of a first attempt at twice the speed, as in the reference:
/// Enter or the A button of a gamepad (the confirm action), Space or Z, or a finger on the play area. A key
/// still held from the menu does not count until it has been let go, and the short intro of a retry is
/// never shortened.
#[test]
fn the_action_button_fast_forwards_the_first_fly_in_only() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    let start_stage = |h: &mut Headless| {
        h.shutdown();
        let menu = Menu::new(Box::new(storage.clone()))
            .with_autostart(Some(mb_app::menu::Autostart::Stage(1)));
        h.push(Box::new(menu)).unwrap();
        h.step(FRAME).unwrap();
        let view = h.top().unwrap().play_view().expect("stage 1 is running");
        assert!(!view.request.retry && view.sim.tick <= 1);
    };

    // Nothing held: the full fly-in.
    start_stage(&mut h);
    assert_eq!(intro_ticks(&mut h), 360);

    // Each way of holding the action: everything above the last 120 frames takes half the ticks.
    for action in [Action::Primary, Action::Confirm] {
        start_stage(&mut h);
        h.input.set_action(action, true);
        let ticks = intro_ticks(&mut h);
        h.input.set_action(action, false);
        assert!((240..=242).contains(&ticks), "{action:?}: {ticks} ticks");
    }

    // A finger on the play area, away from the stick and the pause button, is the action too.
    start_stage(&mut h);
    let layout = h.layout();
    let mut touches = mb_app::ui::TouchControls::new();
    let spot = (320.0, 100.0);
    let (began, ended) = (
        mb_app::ui::TouchPhase::Started,
        mb_app::ui::TouchPhase::Ended,
    );
    touches.touch(1, began, spot, &layout, &mut h.input);
    assert!(h.input.primary_held());
    let ticks = intro_ticks(&mut h);
    touches.touch(1, ended, spot, &layout, &mut h.input);
    assert!(!h.input.primary_held());
    assert!((240..=242).contains(&ticks), "touch: {ticks} ticks");

    // Enter still down from choosing the stage is not the player holding the action ...
    h.shutdown();
    h.push(Box::new(Menu::new(Box::new(storage.clone()))))
        .unwrap();
    h.step(FRAME).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: stage select");
    h.input.set_action(Action::Confirm, true);
    h.step(FRAME).unwrap();
    assert_eq!(playing(&h).map(|p| p.0), Some(1));
    for _ in 0..100 {
        h.step(FRAME).unwrap();
    }
    let view = h.top().unwrap().play_view().unwrap();
    assert_eq!(view.sim.intro_timer, 360 - view.sim.tick as i32);
    // ... until it is let go and pressed again.
    h.input.set_action(Action::Confirm, false);
    h.step(FRAME).unwrap();
    h.input.set_action(Action::Confirm, true);
    let before = h.top().unwrap().play_view().unwrap().sim.intro_timer;
    for _ in 0..10 {
        h.step(FRAME).unwrap();
    }
    let view = h.top().unwrap().play_view().unwrap();
    assert_eq!(view.sim.intro_timer, before - 20);
    h.input.set_action(Action::Confirm, false);

    // A retry (pause, Retry) has the short intro, and holding the action changes nothing about it.
    let retry_ticks = |h: &mut Headless, hold: bool| {
        h.tap(Action::Pause).unwrap();
        h.tap(Action::LookDown).unwrap();
        h.tap(Action::Confirm).unwrap();
        let view = h.top().unwrap().play_view().unwrap();
        assert!(!view.paused && view.sim.tick < 5);
        if hold {
            h.input.set_action(Action::Primary, true);
        }
        let ticks = intro_ticks(h);
        h.input.set_action(Action::Primary, false);
        ticks
    };
    let plain = retry_ticks(&mut h, false);
    assert_eq!(plain, 120);
    assert_eq!(retry_ticks(&mut h, true), plain);
    h.shutdown();
}

/// Mean brightness (0..=255) of the square of `2 * half + 1` pixels around `at`.
fn brightness(rgba: &[u8], size: (u32, u32), at: (i32, i32), half: i32) -> f32 {
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for y in at.1 - half..=at.1 + half {
        for x in at.0 - half..=at.0 + half {
            if x < 0 || y < 0 || x >= size.0 as i32 || y >= size.1 as i32 {
                continue;
            }
            let o = (y as usize * size.0 as usize + x as usize) * 4;
            sum += (rgba[o] as f32 + rgba[o + 1] as f32 + rgba[o + 2] as f32) / 3.0;
            n += 1.0;
        }
    }
    sum / n.max(1.0)
}

/// The pixel a world point lands on, when it is on screen.
fn pixel_of(camera: &Camera, size: (u32, u32), p: Vec3) -> Option<(i32, i32)> {
    let aspect = size.0 as f32 / size.1 as f32;
    let clip = camera.projection(aspect, camera.near, camera.far) * camera.view * p.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let (x, y) = (clip.x / clip.w, clip.y / clip.w);
    let px = ((x * 0.5 + 0.5) * size.0 as f32) as i32;
    let py = ((0.5 - y * 0.5) * size.1 as f32) as i32;
    (px >= 8 && py >= 8 && px < size.0 as i32 - 8 && py < size.1 as i32 - 8).then_some((px, py))
}

/// The ball's shadow is on the floor in play, whichever way the camera looks: the same frame is rendered
/// with the shadow and without it, and the floor beside the ball, where the light throws the shadow, is
/// darker with it. A spot away from the ball does not change.
#[test]
fn the_shadow_darkens_the_floor_beside_the_ball_from_every_side() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    for stage in [1u32, 2] {
        h.shutdown();
        let menu = Menu::new(Box::new(storage.clone()))
            .with_autostart(Some(mb_app::menu::Autostart::Stage(stage)));
        h.push(Box::new(menu)).unwrap();
        // The fly-in, the drop, and a second and a half for the ball to lie still.
        run_until(&mut h, 1000, "the ball at rest", |h| {
            let view = h.top().unwrap().play_view().unwrap();
            view.sim.phase == Phase::Playing && view.sim.stage_timer_frames >= 90
        });
        let (ball, radius) = {
            let view = h.top().unwrap().play_view().unwrap();
            assert!(view.shadow, "stage {stage}: the shadow is on in play");
            assert!(view.sim.ball.speed < 0.01, "stage {stage}: the ball rests");
            let p = view.sim.ball.pos;
            (
                Vec3::new(p.x as f32, p.y as f32, p.z as f32),
                view.sim.ball.curr_radius as f32,
            )
        };
        let size = h.size();
        let light = h.renderer().lighting.direction_to_light();
        // Where the light's ray through the ball's centre meets the floor the ball stands on, and a spot
        // of the shadow further out, clear of the ball's own footprint.
        let centre = ball - light * (radius / light.y);
        let out = Vec3::new(-light.x, 0.0, -light.z).normalize();
        let spot = centre + out * 0.3;
        let away = Vec3::new(ball.x, ball.y - radius, ball.z) - out * 1.6;
        // Whatever the frame queued for itself (HUD, particles) goes with the first render.
        h.render().unwrap();
        let shadow = mb_render::Shadow::projected(ball, radius, &h.renderer().lighting);
        assert!(shadow.is_some());
        let mut looked = 0;
        for step in 0..8 {
            let yaw = step as f32 * std::f32::consts::TAU / 8.0;
            for (dist, height) in [(4.0f32, 2.6f32), (2.5, 3.5)] {
                let eye = ball + Vec3::new(yaw.sin() * dist, height, yaw.cos() * dist);
                let camera = Camera::look_at(eye, ball, Vec3::Y);
                let (Some(at), Some(far)) =
                    (pixel_of(&camera, size, spot), pixel_of(&camera, size, away))
                else {
                    continue;
                };
                // From some sides the ball itself is in front of the spot; its shells would tint both
                // pictures alike and hide most of the difference.
                let (Some(middle), Some(top)) = (
                    pixel_of(&camera, size, ball),
                    pixel_of(&camera, size, ball + Vec3::Y * radius),
                ) else {
                    continue;
                };
                let reach = (top.1 - middle.1).abs() as f32 * 1.15 + 4.0;
                let apart = (((at.0 - middle.0).pow(2) + (at.1 - middle.1).pow(2)) as f32).sqrt();
                let covered = apart <= reach;
                if covered {
                    continue;
                }
                h.renderer_mut().set_shadow(shadow);
                let with = h.render_with(&camera);
                h.renderer_mut().set_shadow(None);
                let without = h.render_with(&camera);
                h.renderer_mut().set_shadow(shadow);
                let (lit, shaded) = (
                    brightness(&without, size, at, 2),
                    brightness(&with, size, at, 2),
                );
                assert!(
                    shaded < lit * 0.85 && lit - shaded > 12.0,
                    "stage {stage}, yaw {step}/8 from {dist} away: the floor at {at:?} is {lit} \
                     without the shadow and {shaded} with it"
                );
                let (a, b) = (
                    brightness(&without, size, far, 2),
                    brightness(&with, size, far, 2),
                );
                assert!(
                    (a - b).abs() < 1.0,
                    "stage {stage}, yaw {step}/8: {a} / {b} away from the ball"
                );
                looked += 1;
            }
        }
        // And through the game's own camera, as the player sees it at rest after GO.
        let camera = h.top().unwrap().camera();
        h.renderer_mut().set_shadow(shadow);
        let with = h.render_with(&camera);
        h.renderer_mut().set_shadow(None);
        let without = h.render_with(&camera);
        h.renderer_mut().set_shadow(shadow);
        let darker = (with.as_chunks::<4>().0.iter())
            .zip(without.as_chunks::<4>().0)
            .filter(|(a, b)| {
                let sum = |p: &[u8; 4]| p[0] as i32 + p[1] as i32 + p[2] as i32;
                sum(b) - sum(a) >= 30
            })
            .count();
        eprintln!("stage {stage}: {darker} pixels of 640 x 360 are darker in the game's own view");
        assert!(
            darker >= 150,
            "stage {stage}: only {darker} pixels are shaded"
        );
        eprintln!("stage {stage}: the shadow shows in {looked} of 16 views");
        assert!(
            looked >= 10,
            "stage {stage}: only {looked} views saw the spot"
        );
    }
    h.shutdown();
}

/// Start `stage` in practice, replacing whatever runs.
fn start_practice_stage(h: &mut Headless, storage: &SharedStorage, stage: u32) {
    h.shutdown();
    let menu = Menu::new(Box::new(storage.clone()))
        .with_autostart(Some(mb_app::menu::Autostart::Stage(stage)));
    h.push(Box::new(menu))
        .unwrap_or_else(|e| panic!("stage {stage}: {e}"));
    assert!(h.step(FRAME).unwrap());
    let view = h.top().unwrap().play_view().expect("the stage is running");
    assert_eq!(view.request.stage_id, stage);
}

/// Render what the last step drew and, when the developer names a directory in `MB_SHOT_DIR` (outside
/// the repository), save it there as `<name>.png`. Nothing is written otherwise.
fn shot(h: &mut Headless, name: &str) -> Vec<u8> {
    let rgba = h.render().unwrap();
    if let Some(dir) = std::env::var_os("MB_SHOT_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = h.size();
        mb_app::image::write_image(&dir.join(format!("{name}.png")), w, h, &rgba).unwrap();
    }
    rgba
}

/// Through the green goal of Beginner floor 2 in course play: the goal banner says `JUMP TO FLOOR 5` and
/// shows the warp bonus of the three floors the course script jumps, and the course goes on at floor 5.
/// In practice the same goal announces nothing, because nothing jumps.
#[test]
fn a_green_goal_announces_the_floor_it_jumps_to() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    // The same run twice: as floor 2 of the course, and as a practice stage.
    for course in [true, false] {
        if course {
            h.shutdown();
            h.push(Box::new(Menu::new(Box::new(storage.clone()))))
                .unwrap();
            h.step(FRAME).unwrap();
            start_beginner_course(&mut h);
            h.input.set_action(Action::MoveForward, true);
            run_until(&mut h, 2000, "floor 2", |h| {
                playing(h).is_some_and(|p| p.1 == 2)
            });
            h.input.set_action(Action::MoveForward, false);
        } else {
            start_practice_stage(&mut h, &storage, 2);
        }
        let mode = if course { "course" } else { "practice" };
        // What only a stage's first drawn frame creates is out of the way before the banner.
        h.render().unwrap();
        let mut pilot: Option<Autopilot> = None;
        let (mut banner_frames, mut rendered) = (0, 0);
        let mut ended = false;
        for frame in 0..6000 {
            let Some(view) = h.top().unwrap().play_view() else {
                ended = true;
                break;
            };
            if view.request.stage_id != 2 {
                ended = true;
                break;
            }
            if view.sim.tick == 0 || pilot.is_none() {
                pilot = Autopilot::for_goal(view.stage, mb_stage::GoalType::Green);
                assert!(pilot.is_some(), "stage 2 has a green goal");
                // The green goal stands on a ledge below the start: off the back edge of the start
                // platform first (it ends at z = 25.5), then through the goal.
                let over_the_edge = mb_engine::math::Vec3::new(0.0, 0.0, 26.6);
                pilot.as_mut().unwrap().targets.push(over_the_edge);
            }
            let (x, y) = pilot.as_mut().unwrap().axes(view.sim);
            h.input.set_axis(Axis::MoveX, x);
            h.input.set_axis(Axis::MoveY, y);
            assert!(h.step(FRAME).unwrap(), "frame {frame}");
            let Some(view) = h.top().unwrap().play_view() else {
                ended = true;
                break;
            };
            if view.request.stage_id != 2 || view.sim.phase != Phase::GoalSequence {
                continue;
            }
            let hit = view.sim.goal_hit.expect("a goal was entered");
            assert_eq!(hit.goal_type, mb_stage::GoalType::Green);
            assert_eq!(view.banner, Some("GOAL"));
            if course {
                assert_eq!(view.jump_text, Some("JUMP TO FLOOR 5"));
                assert_eq!(view.warp_bonus, Some(3));
            } else {
                assert_eq!((view.jump_text, view.warp_bonus), (None, None));
            }
            banner_frames += 1;
            // The banner, its white flash, the swaying line with its trails and the score lines draw
            // at every stage of the sequence, with nothing made for them on the GPU.
            if matches!(banner_frames, 10 | 38 | 100 | 140) {
                let before = h.renderer().counters();
                let rgba = shot(&mut h, &format!("green_goal_{mode}_{banner_frames:03}"));
                assert_eq!(rgba.len(), 640 * 360 * 4);
                assert_eq!(
                    h.renderer().counters(),
                    before,
                    "the banner created something"
                );
                rendered += 1;
            }
        }
        h.input.set_axis(Axis::MoveX, 0.0);
        h.input.set_axis(Axis::MoveY, 0.0);
        assert!(ended, "{mode}: the stage did not end");
        assert!(banner_frames > 100, "{mode}: {banner_frames} goal frames");
        assert_eq!(rendered, 4);
        if course {
            // Three floors on: the bonus stage of floor 5.
            let view = h.top().unwrap().play_view().expect("the course goes on");
            assert_eq!((view.request.stage_id, view.request.floor), (91, 5));
            assert_eq!((view.jump_text, view.warp_bonus), (None, None));
        } else {
            assert_eq!(h.status(), "menu: practice clear");
        }
    }
    h.shutdown();
}

/// The night background's windows are flipbooks: every created window shows the frame the stage timer
/// names, a new one every other tick, by switching objects that exist since the load.
#[test]
fn night_windows_flip_through_their_frames_with_the_stage_timer() {
    use mb_app::backdrop::{night_window_frame, FlipbookKind, NIGHT_WINDOW_LISTS};
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    start_practice_stage(&mut h, &storage, 21);
    h.render().unwrap();
    let created = (h.renderer().counters(), h.renderer().object_count());
    let mut seen = std::collections::BTreeSet::new();
    let mut last_timer = -1;
    for frame in 0..520 {
        assert!(h.step(FRAME).unwrap());
        let view = h.top().unwrap().play_view().unwrap();
        let books = &view.scene.scene.flipbooks;
        // Stage 21: the windows of the four diners that are shown (two more diners are not).
        assert_eq!(books.len(), 23);
        let timer = view.sim.runtime.timer_frames;
        let anim = view.scene.anim_frames();
        assert!(
            (anim - timer as f32).abs() <= 1.0,
            "{anim} at timer {timer}"
        );
        for book in books {
            assert_eq!(book.kind, FlipbookKind::NightWindow);
            let frames = book.frames.len();
            assert!(NIGHT_WINDOW_LISTS.iter().any(|l| l.1 == frames));
            assert!(
                book.frames.iter().all(Option::is_some),
                "a frame is missing"
            );
            assert_eq!(book.shown(), Some(night_window_frame(anim, frames)));
        }
        // The timer stands at zero through the fly-in, then every frame shows for two ticks.
        if timer != last_timer {
            last_timer = timer;
            seen.insert(books[0].shown().unwrap());
        }
        if frame % 100 == 0 {
            h.render().unwrap();
            assert!(h.renderer().stats.draws > 0);
        }
    }
    h.render().unwrap();
    assert_eq!(
        (h.renderer().counters(), h.renderer().object_count()),
        created,
        "flipping created something"
    );
    let frames = h.top().unwrap().play_view().unwrap().scene.scene.flipbooks[0]
        .frames
        .len();
    assert_eq!(seen.len(), frames, "every frame of the first window showed");
    assert!(last_timer > 100);
    // A pause holds the picture.
    h.tap(Action::Pause).unwrap();
    let shown = |h: &Headless| -> Vec<Option<usize>> {
        let view = h.top().unwrap().play_view().unwrap();
        let books = &view.scene.scene.flipbooks;
        books.iter().map(|b| b.shown()).collect()
    };
    let before = shown(&h);
    for _ in 0..10 {
        h.step(FRAME).unwrap();
    }
    assert_eq!(shown(&h), before);
    // Stage 28 has its diners among the foreground objects, whose flipbooks the reference does not play.
    start_practice_stage(&mut h, &storage, 28);
    let view = h.top().unwrap().play_view().unwrap();
    assert!(view.scene.scene.flipbooks.is_empty());
    h.shutdown();
}

/// The storm background's fires are flipbooks of 32 frames, each fire four frames per letter of its
/// offset ahead, standing where the stagedef puts them and turned about Y to the camera.
#[test]
fn storm_fires_flip_and_face_the_camera() {
    use mb_app::backdrop::{camera_rot_y, storm_fire_frame, FlipbookKind, STORM_FIRE_FRAMES};
    use mb_render::glam::Mat4;
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    start_practice_stage(&mut h, &storage, 81);
    h.render().unwrap();
    let created = (h.renderer().counters(), h.renderer().object_count());
    let mut yaws = std::collections::BTreeSet::new();
    let mut frames_seen = std::collections::BTreeSet::new();
    for _ in 0..450 {
        assert!(h.step(FRAME).unwrap());
        let game = h.top().unwrap();
        let view = game.play_view().unwrap();
        let rot_y = camera_rot_y(&game.camera());
        yaws.insert((rot_y * 100.0).round() as i32);
        let anim = view.scene.anim_frames();
        let books = &view.scene.scene.flipbooks;
        // Eight braziers are shown, two more are not.
        assert_eq!(books.len(), 8);
        let mut offsets = std::collections::BTreeSet::new();
        for book in books {
            let FlipbookKind::StormFire { pos, frame_offset } = book.kind else {
                panic!("a window in the storm");
            };
            offsets.insert(frame_offset);
            assert_eq!(book.frames.len(), STORM_FIRE_FRAMES);
            assert!(
                book.frames.iter().all(Option::is_some),
                "a frame is missing"
            );
            let frame = storm_fire_frame(anim, frame_offset);
            assert_eq!(book.shown(), Some(frame));
            let object = book.frames[frame].unwrap();
            let expect = Mat4::from_translation(pos) * Mat4::from_rotation_y(rot_y);
            let m = h.renderer().transform(object);
            assert!(m.abs_diff_eq(expect, 1e-5), "{m} is not {expect}");
        }
        assert!(
            offsets.len() >= 4,
            "the fires burn out of step: {offsets:?}"
        );
        frames_seen.insert(books[0].shown().unwrap());
    }
    h.render().unwrap();
    assert_eq!(
        (h.renderer().counters(), h.renderer().object_count()),
        created,
        "the fires created something"
    );
    assert!(
        yaws.len() > 20,
        "the fly-in turns the camera: {}",
        yaws.len()
    );
    assert_eq!(frames_seen.len(), STORM_FIRE_FRAMES);
    h.shutdown();
}

/// The sunset's ground and cloud layers drift: not while the stage timer stands (the fly-in), evenly
/// once it runs, from the same offsets every time the stage is loaded or retried, and faster with a new
/// heading when fewer than 660 frames are left on the clock.
#[test]
fn sunset_clouds_drift_and_hurry_with_the_clock() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    let offsets = |h: &Headless| -> Vec<[f32; 2]> {
        let view = h.top().unwrap().play_view().unwrap();
        let clouds = view.scene.clouds();
        (0..8).map_while(|i| clouds.offset(i)).collect()
    };
    let step = |a: &[f32; 2], b: &[f32; 2]| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    start_practice_stage(&mut h, &storage, 5);
    let start_offsets = offsets(&h);
    // `SUN_GROUND` and `SUN_CLOUD_A`.
    assert_eq!(start_offsets.len(), 2, "{start_offsets:?}");
    assert_ne!(start_offsets[0], start_offsets[1]);
    // The fly-in holds the stage timer at zero until 120 frames are left: nothing moves.
    run_until(&mut h, 400, "the last two seconds of the intro", |h| {
        h.top().unwrap().play_view().unwrap().sim.intro_timer <= 121
    });
    assert_eq!(offsets(&h), start_offsets);
    for _ in 0..60 {
        h.step(FRAME).unwrap();
    }
    let moved = offsets(&h);
    for (was, now) in start_offsets.iter().zip(&moved) {
        // Sixty frames at 0.9 to 1.1 times a repeat in 660 frames.
        let d = step(was, now) / 60.0 * 660.0;
        assert!((0.88..=1.12).contains(&d), "drifted at {d} of the speed");
    }
    h.render().unwrap();
    let before = h.renderer().counters();
    h.step(FRAME).unwrap();
    h.render().unwrap();
    assert!(h.renderer().stats.upload_bytes > 0, "the drift is uploaded");
    assert_eq!(h.renderer().counters(), before);
    // Another load of the stage starts from the same numbers (a generator of its own, seeded by stage).
    start_practice_stage(&mut h, &storage, 5);
    assert_eq!(offsets(&h), start_offsets);
    // And so does a retry in place.
    for _ in 0..300 {
        h.step(FRAME).unwrap();
    }
    assert_ne!(offsets(&h), start_offsets);
    h.tap(Action::Pause).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    let view = h.top().unwrap().play_view().unwrap();
    assert!(view.sim.tick < 5 && !view.paused);
    assert_eq!(offsets(&h), start_offsets);

    // Hurry-up: let the clock of a sunset stage run down with the ball at rest. Not every stage keeps a
    // ball that nobody steers, so take the first that does.
    let mut hurried_on = None;
    for stage in [5u32, 6, 7, 8, 9, 15, 16, 18] {
        start_practice_stage(&mut h, &storage, stage);
        let mut speeds = (0.0f32, 0.0f32);
        let mut last = offsets(&h);
        for _ in 0..3700 {
            assert!(h.step(FRAME).unwrap());
            let Some(view) = h.top().unwrap().play_view() else {
                break;
            };
            if !matches!(view.sim.phase, Phase::Playing | Phase::Intro) {
                break;
            }
            let left = view.sim.cfg.time_limit_frames - view.sim.stage_timer_frames;
            // The change comes with `HURRY UP!`: the scene's clock (`MkbTime`, from the stage timer)
            // is one frame behind the HUD's, so it has 659 where the HUD has 658.
            assert_eq!(view.scene.clouds().hurried(), left < 659, "{left} left");
            let now = offsets(&h);
            let d = step(&last[0], &now[0]);
            last = now;
            if left == 700 {
                speeds.0 = d;
            }
            if left == 500 {
                speeds.1 = d;
                hurried_on = Some(stage);
            }
        }
        if hurried_on.is_some() {
            // Two and a half seconds after the change the layer moves about twice as fast.
            let ratio = speeds.1 / speeds.0;
            assert!((1.6..=2.5).contains(&ratio), "stage {stage}: {speeds:?}");
            break;
        }
    }
    assert!(
        hurried_on.is_some(),
        "no sunset stage kept the ball that long"
    );
    h.shutdown();
}

/// Stage 92: the floor that ripples is the model of the stage's NaomiLib archive the reference names,
/// waved on the stage timer, and the ball that rides the collision ripple rests on that drawn surface.
#[test]
fn the_bonus_wave_floor_ripples_under_the_ball() {
    use mb_render::WaveParams;
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    start_practice_stage(&mut h, &storage, 92);
    h.render().unwrap();
    let created = (h.renderer().counters(), h.renderer().object_count());
    let wave = WaveParams::BONUS_WAVE;
    let (mut contacts, mut deepest) = (0, 0.0f32);
    let mut lifts = std::collections::BTreeSet::new();
    for frame in 0..900 {
        // A slow circle of the stick, so the ball crosses crests and troughs.
        let a = frame as f32 * 0.01;
        h.input.set_axis(Axis::MoveX, a.cos() * 0.5);
        h.input.set_axis(Axis::MoveY, a.sin() * 0.5);
        assert!(h.step(FRAME).unwrap());
        let Some(view) = h.top().unwrap().play_view() else {
            break;
        };
        assert_eq!(view.scene.scene.wave_objects.len(), 1);
        let timer = view.sim.runtime.timer_frames;
        assert!((view.scene.anim_frames() - timer as f32).abs() <= 1.0);
        if view.sim.phase != Phase::Playing || view.sim.stage_timer_frames < 60 {
            continue;
        }
        let ball = &view.sim.ball;
        let p = Vec3::new(ball.pos.x as f32, ball.pos.y as f32, ball.pos.z as f32);
        if p.x.abs() > 9.5 || p.z.abs() > 9.5 {
            continue;
        }
        // The drawn surface under the ball, as the vertex shader lifts it at this tick.
        let flat = Vec3::new(p.x, 0.0, p.z);
        let (surface, normal) = wave.displace(flat, Vec3::Y, timer as f32);
        lifts.insert((surface.y * 20.0).round() as i32);
        let gap = (p - surface).dot(normal) - ball.curr_radius as f32;
        // Thrown off a crest the ball is above the surface; it is never inside it.
        assert!(
            gap > -0.06,
            "tick {timer}: the ball is {gap} into the floor"
        );
        contacts += (gap.abs() < 0.03) as u32;
        deepest = deepest.min(gap);
        if matches!(frame, 500 | 620) {
            shot(&mut h, &format!("bonus_wave_{frame}"));
        }
    }
    h.input.set_axis(Axis::MoveX, 0.0);
    h.input.set_axis(Axis::MoveY, 0.0);
    eprintln!("stage 92: {contacts} ticks on the drawn surface, deepest {deepest}");
    assert!(
        contacts > 150,
        "the ball touched the drawn surface {contacts} times"
    );
    assert!(
        lifts.len() > 6,
        "the surface under the ball moved: {lifts:?}"
    );
    h.render().unwrap();
    assert_eq!(
        (h.renderer().counters(), h.renderer().object_count()),
        created,
        "the wave created something"
    );
    // No other stage has the floor.
    start_practice_stage(&mut h, &storage, 91);
    let view = h.top().unwrap().play_view().unwrap();
    assert!(view.scene.scene.wave_objects.is_empty());
    h.shutdown();
}

/// A fall-out takes 270 ticks; with the action button down it ends on its 60th (the reference's
/// `canSkip` of `updateRingout`). Either way it is the same lost life and the same retry of the floor.
#[test]
fn the_action_button_cuts_a_fall_out_short_at_the_same_cost() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = start(&SharedStorage::default()) else {
        return;
    };
    let mut results = Vec::new();
    for hold in [false, true] {
        let storage = SharedStorage::default();
        h.shutdown();
        h.push(Box::new(Menu::new(Box::new(storage.clone()))))
            .unwrap();
        h.step(FRAME).unwrap();
        start_beginner_course(&mut h);
        // Forward through the goal of floor 1 and off the edge of floor 2.
        h.input.set_action(Action::MoveForward, true);
        run_until(&mut h, 2000, "floor 2", |h| {
            playing(h).is_some_and(|p| p.1 == 2)
        });
        let score = h.top().unwrap().play_view().unwrap().request.score;
        let (mut fell_at, mut last) = (None, 0);
        let mut banner = false;
        for _ in 0..2000 {
            assert!(h.step(FRAME).unwrap());
            let view = h.top().unwrap().play_view().expect("still in the course");
            if view.request.lives == 2 {
                break;
            }
            if view.sim.phase == Phase::FallOut {
                fell_at.get_or_insert(view.sim.tick);
                last = view.sim.tick;
                banner |= view.banner == Some("FALL OUT");
                // The button goes down with the fall and stays down: the first 59 ticks ignore it.
                if hold {
                    h.input.set_action(Action::Primary, true);
                }
            }
        }
        h.input.set_action(Action::Primary, false);
        h.input.set_action(Action::MoveForward, false);
        let fell_at = fell_at.expect("the ball fell out");
        assert!(banner, "FALL OUT was on screen");
        // One life less, the same floor again with the short intro, the score untouched, and saved.
        let view = h.top().unwrap().play_view().unwrap();
        assert_eq!((view.request.stage_id, view.request.floor), (2, 2));
        assert_eq!(view.request.lives, 2);
        assert!(view.request.retry && view.sim.intro_timer <= 120);
        assert_eq!(view.sim.phase, Phase::Intro);
        assert_ne!(view.banner, Some("FALL OUT"), "the old banner is gone");
        let saved = storage.profile().progress;
        results.push((
            last - fell_at,
            view.request.score - score,
            view.request.bananas,
            saved.course(Difficulty::Beginner).best_score,
        ));
    }
    // The last tick of the sequence ends the state, so the last one seen is the one before it.
    assert_eq!(results[0].0, 269, "an unshortened fall-out is 270 ticks");
    assert_eq!(results[1].0, 59, "a shortened one ends on its 60th tick");
    assert_eq!(results[0].1, results[1].1, "the same score either way");
    assert_eq!(results[0].2, results[1].2);
    assert_eq!(results[0].3, results[1].3);
    h.shutdown();
}
