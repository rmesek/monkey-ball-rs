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

use mb_app::content::{open_disc_image, Content};
use mb_app::headless::{Headless, FRAME};
use mb_app::input::{Action, Axis};
use mb_app::menu::Menu;
use mb_engine::Phase;
use mb_game::{Difficulty, MemoryStorage, Profile, Storage};

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
