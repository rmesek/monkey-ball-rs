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
