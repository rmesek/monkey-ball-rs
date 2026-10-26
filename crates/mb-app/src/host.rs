//! The seam between game states and the platform for the few things only the platform can do: leaving or
//! entering fullscreen, deleting the imported game data, switching device tilt on; and for what only the
//! platform knows: what it offers ([`Caps`]) and where the game's data and saves are kept ([`Host::info`]).
//!
//! The driver owns one [`Host`] and hands it to the states in every `GameCtx` and to the platform once per
//! frame, on the game's thread. States read what the platform offers and [`Host::post`] commands; the
//! platform carries them out ([`Host::drain`]) and reports back the little state the menus show
//! ([`Host::fullscreen`]). Nothing is global: what arrives on another thread (a callback of the operating
//! system) reaches the platform first and the `Host` at the next frame.

/// What the platform offers. Each says so before the game starts; all false by default (and in tests that
/// do not set them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    /// The game data was imported into on-device storage and can be removed again.
    pub remove_data: bool,
    pub fullscreen: bool,
    /// The device reports its orientation, so tilt can steer.
    pub tilt: bool,
    /// The game can end itself, so the title offers "Quit". False where the platform closes the game (a
    /// browser tab, a phone's app switcher).
    pub quit: bool,
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

/// What the platform offers and knows, and what the states asked of it since the last frame.
#[derive(Clone, Debug, Default)]
pub struct Host {
    /// Set by the platform before the game starts.
    pub caps: Caps,
    /// What is kept on this device and where (game data, saves), one short line each. The options screen
    /// shows it, so the player knows what takes up space and can remove it.
    pub info: Vec<String>,
    /// The platform is in fullscreen now.
    pub fullscreen: bool,
    queue: Vec<Command>,
}

impl Host {
    pub fn new(caps: Caps, info: Vec<String>) -> Self {
        Self {
            caps,
            info,
            ..Self::default()
        }
    }

    /// State: ask the platform to do something. A command replaces an older one of the same kind, so the
    /// queue stays small where nothing drains it.
    pub fn post(&mut self, command: Command) {
        let kind = std::mem::discriminant(&command);
        self.queue.retain(|c| std::mem::discriminant(c) != kind);
        self.queue.push(command);
    }

    /// Platform: take what was posted since the last call.
    pub fn drain(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.queue)
    }

    /// A state's own copy: take what the platform offers and knows from `from` (at the start of a frame).
    pub fn mirror(&mut self, from: &Host) {
        self.caps = from.caps;
        self.info.clone_from(&from.info);
        self.fullscreen = from.fullscreen;
    }

    /// A state's own copy: hand what was posted on it to the real one (at the end of a frame).
    pub fn hand_over(&mut self, to: &mut Host) {
        for command in self.queue.drain(..) {
            to.post(command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_queue_once_per_kind_and_drain() {
        let mut host = Host::default();
        assert_eq!(host.caps, Caps::default());
        host.post(Command::SetFullscreen(true));
        host.post(Command::CalibrateTilt);
        host.post(Command::SetFullscreen(false));
        assert_eq!(
            host.drain(),
            [Command::CalibrateTilt, Command::SetFullscreen(false)]
        );
        assert!(host.drain().is_empty());
    }
}
