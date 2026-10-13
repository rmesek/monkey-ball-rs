//! End-to-end tests: the real menu and gameplay states on a `GameStack`, a real `Renderer` on an offscreen
//! target, input injected through `InputState` exactly as a keyboard or gamepad would.
//!
//! They need the user's own disc image (`MB_DISC`, or `Super Monkey Ball*.rvz|iso|gcm` next to the
//! repository) and a GPU adapter (a software one is fine); without either they skip. Nothing read from the
//! disc is written anywhere.

mod common;

use common::find_disc;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use mb_app::autopilot::Autopilot;
use mb_app::content::{Content, open_disc_image};
use mb_app::headless::{FRAME, Headless};
use mb_app::input::{Action, Axis};
use mb_app::menu::Menu;
use mb_engine::Phase;
use mb_game::{Difficulty, MemoryStorage, Profile, Storage};
use mb_render::Camera;
use mb_render::glam::Vec3;

/// One GPU device at a time: the tests are independent but there is no point in fighting over the adapter.
static GPU: Mutex<()> = Mutex::new(());

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
    use mb_app::backdrop::{FlipbookKind, NIGHT_WINDOW_LISTS, night_window_frame};
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
    use mb_app::backdrop::{FlipbookKind, STORM_FIRE_FRAMES, camera_rot_y, storm_fire_frame};
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

/// A save with every stage open and the Reflections option as given.
fn storage_with_reflections(on: bool) -> SharedStorage {
    let mut storage = SharedStorage::default();
    let mut profile = Profile::default();
    profile.settings.unlock_all = true;
    profile.settings.reflections = on;
    profile.save(&mut storage).unwrap();
    storage
}

/// A mirror model of a stage as its file has it: its vertices and bounding sphere, in its own space.
struct MirrorModel {
    points: Vec<Vec3>,
    centre: Vec3,
    radius: f32,
}

/// The mirror models of `stage` in the order of the scene's mirror surfaces (level by level), read
/// from the stage's files; for stage 92, the floor of the wave.
fn mirror_models(content: &Content, stage: u32) -> Vec<MirrorModel> {
    use mb_app::mirror::{StageMirror, stage_mirror};
    use mb_app::stage_scene::StageAssets;
    let assets = StageAssets::load(content, stage, None).unwrap();
    match stage_mirror(&assets) {
        StageMirror::None(_) => Vec::new(),
        StageMirror::Wavy(_) => {
            let naomi = assets.naomi.as_ref().expect("the wave is a NaomiLib stage");
            let name = mb_app::backdrop::BONUS_WAVE_MODEL_NAME;
            let model = naomi.obj.model_by_name(name).expect("the wave's floor");
            vec![MirrorModel {
                points: Vec::new(),
                centre: Vec3::from(model.bound_sphere_center),
                radius: model.bound_sphere_radius,
            }]
        }
        StageMirror::Flat { entries, plan, .. } => {
            let (gma, _) = assets.stage_gma.as_ref().expect("a stage archive");
            let surfaces = plan.levels.iter().flat_map(|l| l.surfaces.iter());
            surfaces
                .map(|i| {
                    let model = gma.model(&entries[*i].name).expect("the mirror model");
                    let meshes = model.meshes().unwrap();
                    let positions = meshes.iter().flat_map(|m| m.vertices.positions.iter());
                    MirrorModel {
                        points: positions.map(|p| Vec3::from(*p)).collect(),
                        centre: Vec3::from(model.bound_sphere_center),
                        radius: model.bound_sphere_radius,
                    }
                })
                .collect()
        }
    }
}

/// The mirror surfaces of the stage being played, level by level, and how many levels there are.
fn mirror_surfaces(h: &Headless) -> (Vec<mb_render::ObjectId>, usize) {
    let view = h.top().unwrap().play_view().unwrap();
    match &view.scene.scene.mirror {
        Some(mirror) => (mirror.surfaces().collect(), mirror.levels.len()),
        None => (Vec::new(), 0),
    }
}

/// How far the vertices of the mirror models, as they are drawn, are from the plane the renderer
/// mirrors in: the largest distance, leaving out one vertex in a hundred (`mirror::STRAY_VERTICES`).
fn off_the_plane(h: &Headless, models: &[MirrorModel], surfaces: &[mb_render::ObjectId]) -> f32 {
    let plane = h.renderer().mirror().expect("a mirror").plane;
    let origin = plane.transform_point3(Vec3::ZERO);
    let normal = plane.transform_vector3(Vec3::Y).normalize();
    let mut worst = 0.0f32;
    for (model, object) in models.iter().zip(surfaces) {
        let shown = h.renderer().transform(*object);
        let gap = |p: &Vec3| normal.dot(shown.transform_point3(*p) - origin).abs();
        let mut gaps: Vec<f32> = model.points.iter().map(gap).collect();
        gaps.sort_by(f32::total_cmp);
        let kept = gaps.len() - gaps.len() / mb_app::mirror::STRAY_VERTICES;
        worst = worst.max(gaps[..kept].last().copied().unwrap_or(0.0));
    }
    worst
}

/// Every stage whose stagedef names mirror models has a mirror floor, like the bonus wave; a sample of
/// them of every kind is played here (`tests/mirrors.rs` holds the whole list). The mirror is set up as
/// the reference does it (mode, opacity, reflection-only mirror models, what is left out), in the plane
/// the mirror models lie in: at 144 frames a second, with the engine moving the anim groups, every
/// mirror model stays in the plane the renderer mirrors in, also where the plane rides on a group
/// (stage 103) and where the models ride on many groups that slide along it (78, 106, 121, 123).
/// Nothing is created on the GPU after the load. Stages that name no mirror model have none.
#[test]
fn mirror_floors_lie_in_the_plane_of_their_mirror_models() {
    use mb_app::mirror::{MirrorKind, MirrorPlane};
    use mb_app::play_scene::mat4_from_mtx;
    use mb_app::stage_scene::ItemKind;
    use mb_render::glam::Mat4;
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    let content = open_disc_image(find_disc().unwrap()).unwrap();
    let half = (h.size().0 / 2, h.size().1 / 2);
    // (stage, mirror models, anim groups they ride on, the plane rides on a group)
    for (stage, count, groups, rides) in [
        (92u32, 1usize, 0usize, false),
        (102, 1, 1, false),
        (103, 1, 1, true),
        (91, 1, 1, false),
        (48, 1, 1, false),
        (74, 1, 1, false),
        (77, 10, 1, false),
        (78, 5, 5, false),
        (106, 16, 15, false),
        (113, 1, 1, false),
        (121, 24, 24, false),
        (123, 21, 21, false),
    ] {
        let models = mirror_models(&content, stage);
        start_practice_stage(&mut h, &storage, stage);
        h.render().unwrap();
        let created = (
            h.renderer().counters(),
            h.renderer().object_count(),
            h.renderer().texture_count(),
        );
        assert!(h.renderer().object_capacity() >= h.renderer().object_count());
        let set = *h.renderer().mirror().expect("the renderer has a mirror");
        let (surfaces, levels) = mirror_surfaces(&h);
        assert_eq!((surfaces.len(), levels), (count, 1), "stage {stage}");
        assert_eq!(models.len(), count, "stage {stage}");
        let plane = {
            let view = h.top().unwrap().play_view().unwrap();
            assert!(view.request.settings.reflections, "on by default");
            let scene = &view.scene.scene;
            let mirror = scene.mirror.as_ref().expect("the stage has a mirror");
            // `AnimGroup.prepareToRender`: a model shows in the capture only with flag bit 2;
            // `BgObjectInst`: a background or foreground object with flag bit 7 does not.
            let stagedef = view.stage;
            let in_mirror = (stagedef.anim_groups.iter())
                .flat_map(|g| g.models.iter())
                .filter(|m| m.flags & 4 != 0);
            let in_mirror: Vec<&str> = in_mirror.map(|m| m.name.as_str()).collect();
            let drawn = |name: &&&str| !scene.missing_models.iter().any(|m| m == **name);
            let models = scene.items.iter().filter(|it| it.kind == ItemKind::Model);
            let hidden_models = models.count() - in_mirror.iter().filter(drawn).count();
            assert_eq!(in_mirror.is_empty(), stage != 113, "stage {stage}");
            let listed = |list: &[mb_stage::BgObject], sources: &[usize]| {
                let shown = sources.iter().filter_map(|i| list.get(*i));
                shown.filter(|o| o.flags & 0x80 != 0).count()
            };
            let hidden = hidden_models
                + listed(&stagedef.bg_objects, &scene.bg_sources)
                + listed(&stagedef.fg_objects, &scene.fg_sources);
            assert_eq!(mirror.left_out.len(), hidden, "stage {stage}");
            let mirrors = scene.items.iter().filter(|it| it.kind == ItemKind::Mirror);
            let on: std::collections::BTreeSet<usize> = mirrors.map(|it| it.group).collect();
            assert_eq!(on.len(), groups, "stage {stage}");
            if stage == 92 {
                // `WAVY_MIRROR_PLANE_Y`, `WAVY_MIRROR_ALPHA`; the reference's distortion is left out.
                assert_eq!(mirror.kind, MirrorKind::Wavy);
                assert_eq!(surfaces, scene.wave_objects);
                let at = Mat4::from_translation(Vec3::new(0.0, 0.02, 0.0));
                assert_eq!(mirror.levels[0].plane, MirrorPlane::Fixed(at));
                assert_eq!(set.alpha, 96.0 / 255.0);
            } else {
                assert_eq!(mirror.kind, MirrorKind::Flat, "stage {stage}");
                assert_eq!(set.alpha, 1.0);
                assert_eq!(on.contains(&0), !matches!(stage, 103 | 113));
            }
            assert_eq!(mirror.levels[0].plane.moves(), rides, "stage {stage}");
            mirror.levels[0].plane
        };
        // The target is half the frame each way, colour and depth: reported apart from the textures.
        assert_eq!(h.renderer().mirror_size(), Some(half));
        assert_eq!(h.renderer().mirror_bytes(), (half.0 * half.1 * 8) as usize);

        // Through the fly-in into play, at a display rate that is no multiple of the tick rate, so
        // that frames fall between ticks.
        let (mut drawn, mut between, mut worst) = (0, 0, 0.0f32);
        let mut moved = std::collections::BTreeSet::new();
        for frame in 0..1100 {
            assert!(h.step(1.0 / 144.0).unwrap());
            let now = h.renderer().mirror().expect("still set").plane;
            if rides {
                // The plane is where its mirror model is drawn: the same blend of the same two ticks,
                // which is not the newest tick's pose while the group moves.
                let shown = h.renderer().transform(surfaces[0]);
                let expect = plane.at(|_| shown);
                assert!(now.abs_diff_eq(expect, 1e-4), "stage {stage}");
                let view = h.top().unwrap().play_view().unwrap();
                let tick = mat4_from_mtx(&view.sim.runtime.anim_groups[1].transform);
                between += !shown.abs_diff_eq(tick, 1e-6) as u32;
                moved.insert((now.y_axis.x * 2000.0).round() as i32);
            } else {
                assert_eq!(now, plane.at(|_| Mat4::IDENTITY), "stage {stage}");
            }
            // Every mirror model is in the plane, wherever the engine has moved its group.
            if frame % 5 == 0 {
                worst = worst.max(off_the_plane(&h, &models, &surfaces));
            }
            if frame % 25 == 0 {
                h.render().unwrap();
                let stats = h.renderer().stats;
                // The capture and the surfaces are counted apart from the frame's own draws.
                if stats.mirror_draws > 0 {
                    assert!(stats.mirror_surface_draws > 0, "stage {stage}: {stats:?}");
                    assert!(stats.mirror_draws > stats.mirror_surface_draws);
                    assert_eq!(
                        stats.total_draws(),
                        stats.draws + stats.quad_draws + stats.sprite_draws + stats.mirror_draws
                    );
                    drawn += 1;
                }
            }
        }
        eprintln!(
            "stage {stage}: {count} mirror model(s) at most {worst:.4} from the plane; the mirror \
             showed in {drawn} of 44 frames"
        );
        assert!(
            worst < 0.025,
            "stage {stage}: a mirror model is {worst} off"
        );
        assert!(
            drawn >= 8,
            "stage {stage}: the mirror showed in {drawn} frames"
        );
        if rides {
            assert!(between > 300, "the plane was blended in {between} frames");
            assert!(moved.len() > 20, "the plane moved: {}", moved.len());
        }
        h.render().unwrap();
        assert_eq!(
            (
                h.renderer().counters(),
                h.renderer().object_count(),
                h.renderer().texture_count(),
            ),
            created,
            "stage {stage}: the mirror created something"
        );
        // Leaving the stage frees the mirror with it.
        h.shutdown();
        assert!(h.renderer().mirror().is_none());
        assert_eq!(h.renderer().mirror_bytes(), 0);
    }
    // Stages whose stagedef names no mirror model have none.
    for stage in [1u32, 2, 5, 81, 107, 122] {
        start_practice_stage(&mut h, &storage, stage);
        assert!(mirror_models(&content, stage).is_empty(), "stage {stage}");
        assert_eq!(mirror_surfaces(&h), (Vec::new(), 0), "stage {stage}");
        assert!(h.renderer().mirror().is_none(), "stage {stage}");
        for _ in 0..60 {
            h.step(FRAME).unwrap();
        }
        h.render().unwrap();
        let stats = h.renderer().stats;
        assert_eq!(
            (stats.mirror_draws, stats.mirror_surface_draws),
            (0, 0),
            "stage {stage}"
        );
        assert_eq!(
            (h.renderer().mirror_size(), h.renderer().mirror_bytes()),
            (None, 0)
        );
    }
    h.shutdown();
}

/// Stage 110 has mirror floors at two heights. The frame mirrors in the higher one while the camera
/// is above it and in the lower one below (the reference's `cameraPos[1] >= -9.0`), and only the
/// mirror model of that floor is drawn. The scene is posed directly here, since the lower floor is at
/// the end of the stage.
#[test]
fn a_stage_with_two_mirror_floors_mirrors_in_the_one_under_the_camera() {
    use mb_app::play_scene::PlayScene;
    use mb_app::stage_scene::{LoadTimings, StageAssets};
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut h) = start(&SharedStorage::default()) else {
        return;
    };
    h.shutdown();
    let content = open_disc_image(find_disc().unwrap()).unwrap();
    let models = mirror_models(&content, 110);
    assert_eq!(models.len(), 2);
    let assets = StageAssets::load(&content, 110, None).unwrap();
    let mut scene = PlayScene::build(h.renderer_mut(), assets, LoadTimings::default(), true);
    let sim = mb_engine::Sim::new(
        &scene.assets.stagedef,
        mb_engine::SimConfig::first_attempt(110),
    );
    let mirror = scene.scene.mirror.clone().expect("a mirror");
    assert_eq!(mirror.levels.len(), 2);
    let heights: Vec<f32> = (mirror.levels.iter())
        .map(|l| l.plane.at(|_| mb_render::glam::Mat4::IDENTITY).w_axis.y)
        .collect();
    assert!((heights[0] + 8.99).abs() < 1e-3 && (heights[1] + 42.99).abs() < 1e-3);
    scene.sync(&sim);
    // What only the first frame of a session makes (the frame's depth texture, the mirror's target).
    scene.apply(h.renderer_mut(), 1.0);
    h.render_with(&Camera::look_at(Vec3::ONE, Vec3::ZERO, Vec3::Y));
    // A camera above each floor's mirror model, looking down at it; then back up again.
    for (level, pass) in [(0usize, 0), (1, 1), (0, 2), (1, 3)] {
        let floor = scene.scene.mirror.as_ref().unwrap().levels[level].surfaces[0];
        let middle = (h.renderer().transform(floor)).transform_point3(models[level].centre);
        let eye = middle + Vec3::new(0.0, 6.0, 9.0);
        let camera = Camera::look_at(eye, middle, Vec3::Y);
        scene.apply(h.renderer_mut(), 1.0);
        scene.face_camera(h.renderer_mut(), &camera);
        assert_eq!(scene.mirror_level(), level, "pass {pass}");
        let plane = h.renderer().mirror().unwrap().plane;
        assert!(
            (plane.w_axis.y - heights[level]).abs() < 1e-4,
            "pass {pass}"
        );
        let before = h.renderer().counters();
        h.render_with(&camera);
        let stats = h.renderer().stats;
        // The floor under the camera shows its reflection; the other floor's model is not drawn.
        let meshes = stats.mirror_surface_draws;
        assert!(meshes > 0 && stats.mirror_draws > meshes, "{stats:?}");
        assert_eq!(h.renderer().counters(), before, "pass {pass}");
        // From far above both floors, looking down the stage: still only one of them.
        let high = Camera::look_at(Vec3::new(0.0, 60.0, 0.0), middle, Vec3::Z);
        scene.face_camera(h.renderer_mut(), &high);
        assert_eq!(scene.mirror_level(), 0);
        scene.sync(&sim);
    }
    scene.unload(h.renderer_mut());
    assert!(h.renderer().mirror().is_none());
}

/// Where the ray through the middle of pixel `at` meets the plane through `origin` facing `normal`.
fn plane_hit(
    camera: &Camera,
    size: (u32, u32),
    at: (u32, u32),
    origin: Vec3,
    normal: Vec3,
) -> Option<Vec3> {
    let aspect = size.0 as f32 / size.1 as f32;
    let clip = camera.projection(aspect, camera.near, camera.far) * camera.view;
    let x = (at.0 as f32 + 0.5) / size.0 as f32 * 2.0 - 1.0;
    let y = 1.0 - (at.1 as f32 + 0.5) / size.1 as f32 * 2.0;
    let world = clip.inverse();
    let near = world.project_point3(Vec3::new(x, y, 0.0));
    let far = world.project_point3(Vec3::new(x, y, 1.0));
    let dir = far - near;
    let along = normal.dot(dir);
    if along.abs() < 1e-9 {
        return None;
    }
    let t = normal.dot(origin - near) / along;
    (t > 0.0).then(|| near + dir * t)
}

/// The reflection is really in the picture: the same frames of a sample of mirror stages (the bonus
/// wave, the two of the first mirrors, two bonus floors, night, ice, storm, space and master floors)
/// are played and drawn with the Reflections option on and off, and they differ, by many pixels, on
/// the mirror models and nowhere else. The option's whole cost is the mirror's draws. On a stage
/// without a mirror the option changes no pixel and no draw.
#[test]
fn the_mirror_floor_shows_a_reflection_that_the_option_turns_off() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let (on, off) = (
        storage_with_reflections(true),
        storage_with_reflections(false),
    );
    let Some(mut h) = start(&on) else {
        return;
    };
    let content = open_disc_image(find_disc().unwrap()).unwrap();
    let size = h.size();
    // The fly-in, GO, and play with the ball on the floor.
    let ticks = [150u32, 300, 420, 560];
    for stage in [92u32, 102, 103, 91, 94, 21, 72, 77, 82, 105, 121, 129, 1, 5] {
        let models = mirror_models(&content, stage);
        let mirrored = !models.is_empty();
        assert_eq!(mirrored, !matches!(stage, 1 | 5));

        // The frames with the option on: the picture, the camera, and where the mirror is.
        let mut frames = Vec::new();
        start_practice_stage(&mut h, &on, stage);
        for at in ticks {
            run_until(&mut h, 2000, "the tick", |h| {
                playing(h).is_some_and(|p| p.2 >= at)
            });
            let rgba = shot(&mut h, &format!("mirror_{stage:03}_tick{at}_on"));
            let stats = h.renderer().stats;
            let camera = h.top().unwrap().camera();
            let plane = h.renderer().mirror().map(|m| m.plane);
            let (surfaces, _) = mirror_surfaces(&h);
            // The bounding sphere of each mirror model, where it is drawn.
            let spheres: Vec<(Vec3, f32)> = (models.iter().zip(&surfaces))
                .map(|(m, o)| {
                    let shown = h.renderer().transform(*o);
                    (shown.transform_point3(m.centre), m.radius)
                })
                .collect();
            frames.push((rgba, stats, camera, plane, spheres));
        }
        // The same frames with it off.
        start_practice_stage(&mut h, &off, stage);
        assert!(h.renderer().mirror().is_none(), "stage {stage}: off");
        assert_eq!(mirror_surfaces(&h), (Vec::new(), 0));
        let (mut shown, mut most) = (0, 0);
        for (at, (with, stats, camera, plane, spheres)) in ticks.into_iter().zip(frames) {
            run_until(&mut h, 2000, "the tick", |h| {
                playing(h).is_some_and(|p| p.2 >= at)
            });
            let without = shot(&mut h, &format!("mirror_{stage:03}_tick{at}_off"));
            let plain = h.renderer().stats;
            assert_eq!(plain.mirror_draws, 0, "stage {stage}");
            assert_eq!(h.renderer().mirror_bytes(), 0);
            // What the option costs is the mirror's draws and nothing else.
            assert_eq!(
                stats.total_draws() - stats.mirror_draws,
                plain.total_draws(),
                "stage {stage} tick {at}"
            );
            // Per pixel, the largest difference of a colour channel.
            let differs = |i: usize| {
                let (a, b) = (&with[i * 4..i * 4 + 3], &without[i * 4..i * 4 + 3]);
                let channels = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b));
                channels.max().unwrap_or(0)
            };
            let pixels = (size.0 * size.1) as usize;
            let changed: Vec<usize> = (0..pixels).filter(|i| differs(*i) > 0).collect();
            let strong = changed.iter().filter(|i| differs(**i) >= 16).count();
            let Some(plane) = plane else {
                assert!(!mirrored, "stage {stage}");
                assert_eq!(stats.mirror_draws, 0, "stage {stage}");
                assert!(
                    changed.is_empty(),
                    "stage {stage} tick {at}: {} pixels differ without a mirror",
                    changed.len()
                );
                continue;
            };
            eprintln!(
                "stage {stage} tick {at}: {} pixels differ ({strong} by 16 or more of 255); \
                 {} draws + {} for the mirror ({} of them its surface)",
                changed.len(),
                plain.total_draws(),
                stats.mirror_draws,
                stats.mirror_surface_draws,
            );
            if stats.mirror_draws == 0 {
                assert!(changed.is_empty(), "stage {stage} tick {at}");
                continue;
            }
            shown += 1;
            most = most.max(changed.len());
            // Every pixel that changed looks at a mirror model: its ray meets the mirror's plane
            // inside one. (The floor of the wave rises and falls about its plane, hence the slack.)
            let origin = plane.transform_point3(Vec3::ZERO);
            let normal = plane.transform_vector3(Vec3::Y).normalize();
            let slack = if stage == 92 { 2.5 } else { 0.3 };
            for i in &changed {
                let pixel = ((i % size.0 as usize) as u32, (i / size.0 as usize) as u32);
                let hit = plane_hit(&camera, size, pixel, origin, normal);
                let inside = |p: Vec3| spheres.iter().any(|(c, r)| (p - *c).length() <= r + slack);
                assert!(
                    hit.is_some_and(inside),
                    "stage {stage} tick {at}: pixel {pixel:?} changed off the mirror ({hit:?})"
                );
            }
        }
        if mirrored {
            assert!(
                shown >= 2,
                "stage {stage}: the mirror showed in {shown} frames"
            );
            // The mirror of a night stage is a strip of floor far from the start; elsewhere it is
            // most of the floor.
            let least = if stage == 21 { 1000 } else { 5000 };
            assert!(most >= least, "stage {stage}: at most {most} pixels differ");
        }
    }
    h.shutdown();
}

/// The Reflections row of the options screen, through the real states: chosen with the keys and with a
/// finger, it turns the option off and on, the change is saved at once, and a menu started again on the
/// same save has it. A stage loaded with it off has no mirror; loaded with it on again, it has.
#[test]
fn the_reflections_option_toggles_saves_and_survives_a_restart() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    assert!(storage.profile().settings.reflections, "on by default");
    // Title -> Options; the row is the fifth (unlock, stick curve, two volumes, reflections).
    let open_options = |h: &mut Headless| {
        h.tap(Action::LookDown).unwrap();
        h.tap(Action::LookDown).unwrap();
        h.tap(Action::Confirm).unwrap();
        assert_eq!(h.status(), "menu: options");
    };
    open_options(&mut h);
    for _ in 0..4 {
        h.tap(Action::LookDown).unwrap();
    }
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: options");
    assert!(!storage.profile().settings.reflections, "saved when chosen");
    assert!(
        storage.profile().settings.unlock_all,
        "nothing else changed"
    );
    shot(&mut h, "options_reflections_off");

    // A restart of the states on the same save: stage 102 is loaded without its mirror.
    let play_102 = |h: &mut Headless| {
        start_practice_stage(h, &storage, 102);
        for _ in 0..120 {
            h.step(FRAME).unwrap();
        }
        h.render().unwrap();
        let view = h.top().unwrap().play_view().unwrap();
        let on = view.request.settings.reflections;
        assert_eq!(view.scene.scene.mirror.is_some(), on);
        assert_eq!(h.renderer().mirror().is_some(), on);
        assert_eq!(h.renderer().stats.mirror_draws > 0, on);
        assert_eq!(h.renderer().mirror_bytes() > 0, on);
        on
    };
    assert!(!play_102(&mut h));

    // Back on with a finger, on a screen the size of a phone held upright (every row is in view).
    h.shutdown();
    h.resize((540, 1200));
    h.push(Box::new(Menu::new(Box::new(storage.clone()))))
        .unwrap();
    h.step(FRAME).unwrap();
    open_options(&mut h);
    let layout = h.layout();
    let rows = mb_app::widgets::ListLayout::column(&layout, 7, 0.24);
    assert!(rows.is_visible(4, 0.0) && rows.max_scroll() == 0.0);
    assert!(rows.item_h >= layout.min_tap(), "a full tap target");
    let spot = rows.item_rect(4, 0.0).center();
    let mut touches = mb_app::ui::TouchControls::new();
    let (began, ended) = (
        mb_app::ui::TouchPhase::Started,
        mb_app::ui::TouchPhase::Ended,
    );
    touches.touch(1, began, spot, &layout, &mut h.input);
    h.step(FRAME).unwrap();
    touches.touch(1, ended, spot, &layout, &mut h.input);
    h.step(FRAME).unwrap();
    assert_eq!(h.status(), "menu: options");
    assert!(storage.profile().settings.reflections, "a tap turned it on");
    shot(&mut h, "options_reflections_on_portrait");
    // The stick or d-pad of a gamepad and its A button are the same actions as the keys: the row the
    // tap chose is the selected one, and choosing it turns the option off and on again.
    for expect in [false, true] {
        h.tap(Action::Confirm).unwrap();
        assert_eq!(storage.profile().settings.reflections, expect);
    }
    h.resize((640, 360));
    assert!(play_102(&mut h));
    h.shutdown();
}

/// The options screen on a phone held upright (412 x 915): the storage note is wrapped into two lines,
/// and both are drawn at one size. (Each line used to be fitted by itself, so the short second line,
/// `MAY CLEAR IT)`, came out several times as large as the long first one.)
#[test]
fn the_storage_note_of_the_options_screen_is_one_size_on_a_phone() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::default();
    let Some(mut h) = start(&storage) else {
        return;
    };
    h.shutdown();
    h.resize((412, 915));
    let note = "Game data: 144 MB in this browser (the browser may clear it)";
    mb_app::host::set_info(vec![note.into()]);
    h.push(Box::new(Menu::new(Box::new(storage.clone()))))
        .unwrap();
    h.step(FRAME).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: options");
    let rgba = shot(&mut h, "options_storage_note_412x915");
    mb_app::host::set_info(Vec::new());
    let (w, height) = (412usize, 915usize);
    assert_eq!(rgba.len(), w * height * 4);
    // The note is the only pale blue text above the list; the title is yellow.
    let pale = |p: &[u8; 4]| p[0] > 150 && p[1] > 170 && p[2] > 200;
    let row = |y: usize| {
        let pixels = rgba[y * w * 4..(y + 1) * w * 4].as_chunks::<4>().0.iter();
        let lit: Vec<usize> = (pixels.enumerate())
            .filter(|(_, p)| pale(p))
            .map(|(x, _)| x)
            .collect();
        (lit.first().copied()).zip(lit.last().copied())
    };
    // Bands of rows with such pixels between the Back button and the list: (first row, last row,
    // leftmost, rightmost).
    let rows = mb_app::widgets::ListLayout::column(&h.layout(), 8, 0.24 + 2.0 * 0.055);
    let below_back = mb_app::widgets::back_button_rect(&h.layout());
    let top = (below_back.y + below_back.h) as usize + 4;
    let mut bands: Vec<(usize, usize, usize, usize)> = Vec::new();
    for y in top..rows.area.y as usize {
        let Some((left, right)) = row(y) else {
            continue;
        };
        match bands.last_mut() {
            Some(b) if b.1 + 1 >= y => *b = (b.0, y, b.2.min(left), b.3.max(right)),
            _ => bands.push((y, y, left, right)),
        }
    }
    assert_eq!(bands.len(), 2, "two lines of the note: {bands:?}");
    let tall = |b: &(usize, usize, usize, usize)| (b.1 - b.0 + 1) as f32;
    let wide = |b: &(usize, usize, usize, usize)| (b.3 - b.2 + 1) as f32;
    // The same letters' height (a line with a bracket reaches a little further up and down) ...
    let ratio = tall(&bands[1]) / tall(&bands[0]);
    assert!((0.7..=1.4).contains(&ratio), "line heights {bands:?}");
    // ... the long line fits the screen, and the short one is much the narrower of the two.
    assert!(wide(&bands[0]) <= w as f32 * 0.92, "{bands:?}");
    assert!(wide(&bands[1]) < wide(&bands[0]) * 0.5, "{bands:?}");
    h.resize((640, 360));
    h.shutdown();
}

/// The capture is drawn only where the mirror surfaces in view can show it, and only with what falls
/// there. That changes no pixel: on mirror stages of every kind, through the game's own camera in the
/// fly-in and in play and through cameras all round the ball, high and low, the frame is the same,
/// byte for byte, as with the whole reflected view captured, for fewer draw calls.
#[test]
fn cutting_the_capture_to_the_mirror_changes_no_pixel() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let storage = SharedStorage::unlocked();
    let Some(mut h) = start(&storage) else {
        return;
    };
    let (mut frames, mut saved, mut whole_draws) = (0, 0i64, 0i64);
    for stage in [92u32, 102, 103, 91, 21, 77, 89, 110, 121] {
        start_practice_stage(&mut h, &storage, stage);
        for at in [150u32, 330, 450] {
            run_until(&mut h, 2000, "the tick", |h| {
                playing(h).is_some_and(|p| p.2 >= at)
            });
            // What the frame queued for itself (HUD, particles) goes with this one.
            h.render().unwrap();
            let ball = {
                let view = h.top().unwrap().play_view().unwrap();
                let p = view.sim.ball.pos;
                Vec3::new(p.x as f32, p.y as f32, p.z as f32)
            };
            let mut cameras = vec![h.top().unwrap().camera()];
            for step in 0..6 {
                let yaw = step as f32 * std::f32::consts::TAU / 6.0 + 0.3;
                let (dist, height) = [(6.0f32, 1.2f32), (14.0, 6.0), (30.0, 22.0)][step % 3];
                let eye = ball + Vec3::new(yaw.sin() * dist, height, yaw.cos() * dist);
                cameras.push(Camera::look_at(eye, ball, Vec3::Y));
            }
            for (i, camera) in cameras.iter().enumerate() {
                h.renderer_mut().set_mirror_capture_whole(true);
                let whole = h.render_with(camera);
                let all = h.renderer().stats;
                h.renderer_mut().set_mirror_capture_whole(false);
                let cut = h.render_with(camera);
                let stats = h.renderer().stats;
                assert!(
                    cut == whole,
                    "stage {stage} tick {at} camera {i}: the picture changed"
                );
                assert_eq!(stats.draws, all.draws);
                // (A mirror model whose bounding sphere reaches into the view while the model itself
                // does not is no longer drawn at all.)
                assert!(stats.mirror_surface_draws <= all.mirror_surface_draws);
                // (A sphere test is on the safe side, so now and then the cut keeps one object more.)
                frames += (all.mirror_draws > 0) as u32;
                saved += all.mirror_draws as i64 - stats.mirror_draws as i64;
                whole_draws += all.mirror_draws as i64;
            }
        }
    }
    eprintln!(
        "{frames} frames with a mirror in view: the cut saved {saved} of {whole_draws} capture draws"
    );
    assert!(frames >= 120, "{frames} frames showed a mirror");
    assert!(saved * 10 >= whole_draws, "saved {saved} of {whole_draws}");
    h.shutdown();
}
