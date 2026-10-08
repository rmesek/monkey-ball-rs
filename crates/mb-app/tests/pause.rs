//! The pause menu's rules, on the real states: Retry is offered only while the attempt is undecided, never
//! skips a bonus stage, and quitting a course asks first.
//!
//! Like `gameplay.rs` these need the user's own disc image (`MB_DISC`, or `Super Monkey Ball*.rvz|iso|gcm`
//! next to the repository) and a GPU adapter; without either they skip. Nothing read from the disc is
//! written anywhere.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use mb_app::content::{open_disc_image, Content};
use mb_app::headless::{Headless, FRAME};
use mb_app::input::{Action, Axis};
use mb_app::menu::{Autostart, Menu};
use mb_engine::Phase;
use mb_game::{Difficulty, MemoryStorage, PlayMode, Profile, Storage};

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
}

/// A headless game with `menu` on the stack, or `None` (with a note) when the machine cannot run it.
fn start(menu: Menu) -> Option<Headless> {
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
    h.push(Box::new(menu)).expect("menu");
    assert!(h.step(FRAME).unwrap());
    Some(h)
}

/// Step until a stage is being played (stage loading may take a few frames).
fn wait_for_stage(h: &mut Headless) {
    for _ in 0..600 {
        if h.top().and_then(|g| g.play_view()).is_some() {
            return;
        }
        assert!(h.step(FRAME).unwrap());
    }
    panic!("no stage started; at {:?}", h.status());
}

struct View {
    stage: u32,
    floor: u32,
    lives: u32,
    score: u64,
    retry: bool,
    mode: PlayMode,
    phase: Phase,
    tick: u32,
    paused: bool,
}

fn view(h: &Headless) -> View {
    let v = h
        .top()
        .and_then(|g| g.play_view())
        .unwrap_or_else(|| panic!("no stage is running; at {:?}", h.status()));
    View {
        stage: v.request.stage_id,
        floor: v.request.floor,
        lives: v.request.lives,
        score: v.request.score,
        retry: v.request.retry,
        mode: v.request.mode,
        phase: v.sim.phase,
        tick: v.sim.tick,
        paused: v.paused,
    }
}

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

fn in_phase(h: &Headless, phase: Phase) -> bool {
    h.top()
        .and_then(|g| g.play_view())
        .is_some_and(|v| v.sim.phase == phase)
}

/// The paused state as the window title reports it: `" | paused"`, `" | quit?"` or nothing.
fn overlay(h: &Headless) -> &'static str {
    let s = h.status();
    if s.ends_with(" | quit?") {
        "quit?"
    } else if s.ends_with(" | paused") {
        "paused"
    } else {
        ""
    }
}

/// Stop steering: the stick returns to the centre and no movement key is held.
fn hands_off(h: &mut Headless) {
    h.input.set_axis(Axis::MoveX, 0.0);
    h.input.set_axis(Axis::MoveY, 0.0);
    h.input.set_action(Action::MoveForward, false);
}

#[test]
fn course_retry_costs_a_life_while_playing_and_quit_asks_first() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let menu = Menu::new(Box::new(storage.clone()))
        .with_autostart(Some(Autostart::Course(Difficulty::Beginner)));
    let Some(mut h) = start(menu) else { return };
    wait_for_stage(&mut h);
    let v = view(&h);
    assert_eq!(
        (v.stage, v.floor, v.lives, v.mode),
        (1, 1, 3, PlayMode::Course)
    );

    // Pause during the fly-in: Retry is offered and costs a life, as its label says.
    h.tap(Action::Pause).unwrap();
    assert_eq!(overlay(&h), "paused");
    assert_eq!(view(&h).phase, Phase::Intro);
    assert_eq!(h.render().unwrap().len(), 640 * 360 * 4);
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    wait_for_stage(&mut h);
    let v = view(&h);
    assert_eq!(
        (v.stage, v.floor, v.lives),
        (1, 1, 2),
        "retry = one life, same floor"
    );
    assert!(v.retry && !v.paused);

    // And the same in the middle of play.
    run_until(&mut h, 400, "play", |h| in_phase(h, Phase::Playing));
    h.tap(Action::Pause).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    wait_for_stage(&mut h);
    assert_eq!(view(&h).lives, 1);

    // Quit: two rows down, then a question with the safe answer selected.
    run_until(&mut h, 400, "play", |h| in_phase(h, Phase::Playing));
    h.tap(Action::Pause).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "quit?");
    assert_eq!(h.render().unwrap().len(), 640 * 360 * 4);
    let tick = view(&h).tick;
    // Confirm on the default answer keeps playing (back at the pause menu, still paused).
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "paused");
    // Escape / Back from the question also returns to the pause menu instead of resuming.
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "quit?");
    h.tap(Action::Cancel).unwrap();
    assert_eq!(overlay(&h), "paused");
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "quit?");
    h.tap(Action::Pause).unwrap();
    assert_eq!(overlay(&h), "paused");
    for _ in 0..20 {
        h.step(FRAME).unwrap();
    }
    assert_eq!(
        view(&h).tick,
        tick,
        "nothing ran while the question was open"
    );

    // Answering "quit course" leaves the stage and the run.
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "quit?");
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    for _ in 0..5 {
        h.step(FRAME).unwrap();
    }
    assert!(h.top().unwrap().play_view().is_none());
    assert_eq!(h.status(), "menu: title");
    h.shutdown();
}

#[test]
fn pausing_in_the_goal_sequence_cannot_turn_the_goal_into_a_lost_life() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let menu = Menu::new(Box::new(storage.clone()))
        .with_autostart(Some(Autostart::Course(Difficulty::Beginner)));
    let Some(mut h) = start(menu) else { return };
    wait_for_stage(&mut h);

    // Holding "forward" rolls straight through the goal of floor 1.
    h.input.set_action(Action::MoveForward, true);
    run_until(&mut h, 2000, "the goal", |h| {
        in_phase(h, Phase::GoalSequence)
    });
    hands_off(&mut h);
    h.step(FRAME).unwrap();

    // Pausing still works ...
    h.tap(Action::Pause).unwrap();
    assert!(view(&h).paused);
    assert_eq!(view(&h).phase, Phase::GoalSequence);
    let tick = view(&h).tick;
    assert_eq!(h.render().unwrap().len(), 640 * 360 * 4);
    // ... but Retry cannot be reached: one step down from Continue lands on Quit, whose Confirm opens the
    // question (Retry would have ended the stage instead).
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "quit?");
    assert_eq!(view(&h).tick, tick);
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "paused");
    // Up from Quit skips Retry again, back to Continue.
    h.tap(Action::LookUp).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "");
    assert!(!view(&h).paused);

    // The goal counts: next floor, all three lives, a score.
    run_until(&mut h, 1000, "floor 2", |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.request.floor == 2)
    });
    let v = view(&h);
    assert_eq!((v.stage, v.lives), (2, 3));
    assert!(v.score > 0 && !v.retry);
    assert!(storage.profile().progress.stage_best(1).clears == 1);
    h.shutdown();
}

#[test]
fn pausing_in_the_fall_out_cannot_dodge_or_double_its_cost() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let menu = Menu::new(Box::new(storage.clone()))
        .with_autostart(Some(Autostart::Course(Difficulty::Beginner)));
    let Some(mut h) = start(menu) else { return };
    wait_for_stage(&mut h);

    // Holding "forward" rolls through the goal of floor 1 and straight off the edge of floor 2.
    h.input.set_action(Action::MoveForward, true);
    run_until(&mut h, 2000, "floor 2", |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.request.floor == 2)
    });
    run_until(&mut h, 2000, "the fall-out", |h| {
        in_phase(h, Phase::FallOut)
    });
    hands_off(&mut h);
    h.step(FRAME).unwrap();
    h.tap(Action::Pause).unwrap();
    assert!(view(&h).paused);
    // Retry is skipped: down from Continue is Quit.
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(overlay(&h), "quit?");
    h.tap(Action::Cancel).unwrap();
    h.tap(Action::Cancel).unwrap();
    assert!(!view(&h).paused);

    // The fall-out plays out and costs exactly one life.
    run_until(&mut h, 1000, "the retry", |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.request.retry)
    });
    let v = view(&h);
    assert_eq!((v.stage, v.floor, v.lives), (2, 2, 2));
    h.shutdown();
}

/// A bonus stage in practice: Retry restarts it in place. In course play the same row is disabled on a
/// bonus stage, because the course treats a failed bonus stage as done and a "retry" would skip it. That
/// half of the rule is covered by `play::tests::retry_is_offered_only_while_the_attempt_is_undecided`
/// and, for the disabled row itself, by the goal-sequence test above: the first bonus floor of a course
/// is four floors in, further than these tests can steer.
#[test]
fn practice_retry_restarts_a_bonus_stage_and_quit_does_not_ask() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let menu = Menu::new(Box::new(storage.clone())).with_autostart(Some(Autostart::Stage(91)));
    let Some(mut h) = start(menu) else { return };
    wait_for_stage(&mut h);
    let v = view(&h);
    assert_eq!((v.stage, v.mode), (91, PlayMode::Practice));
    assert!(mb_game::stages::is_bonus_stage(v.stage));

    run_until(&mut h, 400, "play", |h| in_phase(h, Phase::Playing));
    for _ in 0..30 {
        h.step(FRAME).unwrap();
    }
    h.tap(Action::Pause).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    let v = view(&h);
    assert_eq!(v.stage, 91, "still the bonus stage");
    assert!(!v.paused && v.tick < 5 && v.phase == Phase::Intro);

    // Practice has nothing to lose: Quit leaves at once.
    h.tap(Action::Pause).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    for _ in 0..5 {
        h.step(FRAME).unwrap();
    }
    assert!(h.top().unwrap().play_view().is_none());
    assert!(h.status().starts_with("menu:"), "{}", h.status());
    h.shutdown();
}
