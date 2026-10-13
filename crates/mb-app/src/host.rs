//! The seam between game states and the platform shell for the few things only the platform can do:
//! leaving or entering fullscreen, deleting the imported game data, switching device tilt on; and for what
//! only the platform knows: where the game's data and saves are kept ([`info`]).
//!
//! States never see the platform. They read what it offers ([`caps`], set once by the shell before the game
//! starts) and post [`Command`]s; the shell drains them once per frame ([`drain`]) and reports back the
//! little state the menus show ([`set_fullscreen`]). Everything is per thread, like the game itself.

use std::cell::{Cell, RefCell};

/// What the platform offers. All false on desktop and in tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    /// The game data was imported into on-device storage and can be removed again.
    pub remove_data: bool,
    pub fullscreen: bool,
    /// The device reports its orientation, so tilt can steer.
    pub tilt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Delete the imported game data and return to where the game image is asked for (the browser's
    /// landing page; the desktop program ends, and asks at its next start).
    RemoveGameData,
    SetFullscreen(bool),
    /// Steer by device tilt (or stop): `range_deg` of tilt is full deflection.
    Tilt {
        enabled: bool,
        range_deg: f32,
    },
    /// Take the pose the device is held in now as "level" (stage start).
    CalibrateTilt,
}

thread_local! {
    static CAPS: Cell<Caps> = const {
        Cell::new(Caps { remove_data: false, fullscreen: false, tilt: false })
    };
    static INFO: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static FULLSCREEN: Cell<bool> = const { Cell::new(false) };
    static QUEUE: RefCell<Vec<Command>> = const { RefCell::new(Vec::new()) };
}

/// Shell: say what this platform can do (before the first frame).
pub fn set_caps(caps: Caps) {
    CAPS.with(|c| c.set(caps));
}

pub fn caps() -> Caps {
    CAPS.with(Cell::get)
}

/// Shell: what is kept on this device and where (game data, saves), one short line each. The options
/// screen shows it, so the player knows what takes up space and can remove it.
pub fn set_info(lines: Vec<String>) {
    INFO.with(|i| *i.borrow_mut() = lines);
}

pub fn info() -> Vec<String> {
    INFO.with(|i| i.borrow().clone())
}

/// Shell: the page is (not) fullscreen now.
pub fn set_fullscreen(on: bool) {
    FULLSCREEN.with(|f| f.set(on));
}

pub fn is_fullscreen() -> bool {
    FULLSCREEN.with(Cell::get)
}

/// State: ask the platform to do something. Without a shell that drains the queue nothing happens (and
/// the queue stays small: commands replace an older one of the same kind).
pub fn post(command: Command) {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.retain(|c| std::mem::discriminant(c) != std::mem::discriminant(&command));
        q.push(command);
    });
}

/// Shell: take what was posted since the last call.
pub fn drain() -> Vec<Command> {
    QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_queue_once_per_kind_and_drain() {
        assert_eq!(caps(), Caps::default());
        post(Command::SetFullscreen(true));
        post(Command::CalibrateTilt);
        post(Command::SetFullscreen(false));
        assert_eq!(
            drain(),
            [Command::CalibrateTilt, Command::SetFullscreen(false)]
        );
        assert!(drain().is_empty());
        set_caps(Caps {
            fullscreen: true,
            ..Caps::default()
        });
        assert!(caps().fullscreen && !caps().tilt);
        set_caps(Caps::default());
        set_fullscreen(true);
        assert!(is_fullscreen());
        set_fullscreen(false);
    }
}
