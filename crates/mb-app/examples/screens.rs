//! Debug helper: play through a scripted session headlessly and save PNGs of the screens that the
//! `--autoplay` run does not reach (course HUD, pause, fall-out, game over, time over, touch controls,
//! portrait layout). Look at them after changing HUD or menu code.
//!
//! `cargo run --release -p mb-app --example screens -- <disc> <out dir (outside the repo)> [WxH] [stage ids]`
//!
//! With a comma separated list of stage ids only those stages are captured, one second into play.

use std::path::{Path, PathBuf};

use mb_app::content::open_disc_image;
use mb_app::headless::{unlocked_storage, Headless, FRAME};
use mb_app::input::{Action, TouchStick};
use mb_app::menu::{Autostart, Menu};
use mb_engine::Phase;
use mb_game::MemoryStorage;

fn shot(h: &mut Headless, dir: &Path, name: &str) -> Result<(), String> {
    let path = dir.join(format!("{name}.png"));
    h.save_png(&path)?;
    println!("{}", path.display());
    Ok(())
}

/// Step until `done`, at most `limit` frames.
fn until(
    h: &mut Headless,
    limit: u32,
    mut done: impl FnMut(&Headless) -> bool,
) -> Result<(), String> {
    for _ in 0..limit {
        h.step(FRAME)?;
        if done(h) {
            return Ok(());
        }
    }
    Err(format!("gave up waiting at {:?}", h.status()))
}

fn phase(h: &Headless) -> Option<Phase> {
    Some(h.top()?.play_view()?.sim.phase)
}

fn course_session(
    content: &mb_app::content::Content,
    size: (u32, u32),
    dir: &Path,
) -> Result<(), String> {
    let mut h = Headless::new(content, size, false)?;
    h.push(Box::new(Menu::new(Box::new(MemoryStorage::new()))))?;
    h.step(FRAME)?;
    h.tap(Action::Confirm)?;
    h.step(FRAME)?;
    shot(&mut h, dir, "course_select")?;
    h.tap(Action::Confirm)?;
    // Floor 1: forward through the goal, with a pause on the way.
    h.input.set_action(Action::MoveForward, true);
    until(&mut h, 600, |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.sim.stage_timer_frames == 60)
    })?;
    shot(&mut h, dir, "course_play")?;
    h.tap(Action::Pause)?;
    h.tap(Action::LookDown)?;
    shot(&mut h, dir, "pause")?;
    h.tap(Action::Pause)?;
    until(&mut h, 2000, |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.sim.goal_timer > 0 && v.sim.goal_timer < 150)
    })?;
    shot(&mut h, dir, "goal_score")?;
    // Floor 2: the floor intro, then off the edge three times.
    until(&mut h, 2000, |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.request.floor == 2 && v.sim.intro_timer == 230)
    })?;
    shot(&mut h, dir, "floor_intro")?;
    until(&mut h, 2000, |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.sim.phase == Phase::FallOut && v.sim.ringout_timer == 200)
    })?;
    shot(&mut h, dir, "fall_out")?;
    until(&mut h, 6000, |h| h.status() == "menu: game over")?;
    h.input.set_action(Action::MoveForward, false);
    h.step(FRAME)?;
    shot(&mut h, dir, "game_over")?;
    h.shutdown();
    Ok(())
}

fn practice_session(
    content: &mb_app::content::Content,
    size: (u32, u32),
    dir: &Path,
    tag: &str,
    touch: bool,
) -> Result<(), String> {
    let mut h = Headless::new(content, size, false)?;
    let menu = Menu::new(unlocked_storage());
    h.push(Box::new(menu))?;
    h.input.set_touch_active(touch);
    h.step(FRAME)?;
    shot(&mut h, dir, &format!("{tag}_title"))?;
    h.tap(Action::LookDown)?;
    h.tap(Action::Confirm)?;
    h.tap(Action::LookDown)?;
    h.tap(Action::LookDown)?;
    h.tap(Action::Confirm)?;
    for _ in 0..12 {
        h.tap(Action::LookDown)?;
    }
    h.step(FRAME)?;
    shot(&mut h, dir, &format!("{tag}_stage_select"))?;
    h.shutdown();

    // Straight into a stage: idle until the clock runs out.
    let mut h = Headless::new(content, size, false)?;
    let menu = Menu::new(unlocked_storage()).with_autostart(Some(Autostart::Stage(1)));
    h.push(Box::new(menu))?;
    h.input.set_touch_active(touch);
    until(&mut h, 600, |h| phase(h) == Some(Phase::Playing))?;
    if touch {
        let l = h.layout();
        let origin = l.stick_rest();
        let r = l.stick_radius();
        h.input.set_touch_stick(Some(TouchStick {
            origin,
            pos: (origin.0 + r * 0.5, origin.1 - r * 0.7),
        }));
    }
    for _ in 0..40 {
        h.step(FRAME)?;
    }
    shot(&mut h, dir, &format!("{tag}_play"))?;
    until(&mut h, 4000, |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.sim.cfg.time_limit_frames - v.sim.stage_timer_frames == 200)
    })?;
    shot(&mut h, dir, &format!("{tag}_hurry"))?;
    until(&mut h, 4000, |h| {
        h.top()
            .and_then(|g| g.play_view())
            .is_some_and(|v| v.sim.timeover_timer == 80)
    })?;
    shot(&mut h, dir, &format!("{tag}_time_over"))?;
    h.shutdown();
    Ok(())
}

/// One frame of each listed stage, a second after GO with the stick held forward.
fn stage_shots(
    content: &mb_app::content::Content,
    size: (u32, u32),
    dir: &Path,
    ids: &[u32],
) -> Result<(), String> {
    let mut h = Headless::new(content, size, false)?;
    for id in ids {
        let menu = Menu::new(unlocked_storage()).with_autostart(Some(Autostart::Stage(*id)));
        h.push(Box::new(menu))?;
        h.input.set_action(Action::MoveForward, true);
        until(&mut h, 600, |h| {
            h.top()
                .and_then(|g| g.play_view())
                .is_some_and(|v| v.sim.stage_timer_frames >= 60 || v.sim.phase != Phase::Intro)
        })?;
        until(&mut h, 200, |h| {
            h.top()
                .and_then(|g| g.play_view())
                .is_none_or(|v| v.sim.stage_timer_frames >= 60 || v.sim.phase != Phase::Playing)
        })?;
        shot(&mut h, dir, &format!("stage_{id:03}"))?;
        h.input.set_action(Action::MoveForward, false);
        h.shutdown();
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(disc), Some(dir)) = (args.first(), args.get(1)) else {
        return Err("usage: screens <disc> <out dir> [WxH]".into());
    };
    let size = match args.get(2).and_then(|s| s.split_once('x')) {
        Some((w, h)) => (
            w.parse().map_err(|_| "bad width")?,
            h.parse().map_err(|_| "bad height")?,
        ),
        None => (1280, 720),
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let content = open_disc_image(disc).map_err(|e| format!("{disc}: {e}"))?;
    if let Some(list) = args.get(3) {
        let ids: Vec<u32> = list
            .split(',')
            .map(|s| s.trim().parse().map_err(|_| format!("bad stage id {s:?}")))
            .collect::<Result<_, _>>()?;
        return stage_shots(&content, size, &dir, &ids);
    }
    course_session(&content, size, &dir)?;
    practice_session(&content, size, &dir, "practice", false)?;
    // Phone in portrait with touch controls.
    practice_session(&content, (size.1, size.0), &dir, "portrait_touch", true)?;
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
