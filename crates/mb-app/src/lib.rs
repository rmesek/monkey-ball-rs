//! The game as a library: everything that is the same on every platform, and nothing that knows a window,
//! an event loop or an operating system. A platform driver (`mb-desktop`, `mb-web`; see
//! `docs/ARCHITECTURE.md`) owns the window and the GPU surface, feeds [`input::InputState`], carries out
//! [`host::Command`]s, hands over a [`content::Content`], a `mb_game::Storage` and an
//! [`audio::AudioBackend`], and calls [`game::GameStack::update`] and the renderer once per frame.
//!
//! * [`content`]: where game files come from (a game pack in a file, or imported game data that is
//!   prepared before it is read) and the lists of files the game needs.
//! * [`loading`]: the stage loading state (fetch, parse, build, with a progress screen).
//! * [`host`] / [`tilt`]: the seam to platform features (fullscreen, removing imported data, device tilt).
//!   A game state reaches the platform through these and through [`input`] only.
//! * [`game`]: `Game` state trait and the state stack.
//! * [`input`]: logical actions, axes, the steering vector and the pointer; drivers only feed it.
//! * [`menu`]: title menu, course / stage select, options and the course flow (root state).
//! * [`play`] + [`play_scene`] + [`particles`] + [`hud`]: one stage being played: fixed-step simulation,
//!   3D scene, the particles and shadow on top of it, HUD.
//! * [`audio`]: sounds, music and the backend trait (`audio_cpal`: device output on native targets; the
//!   browser has its own in `mb-web`). See `docs/AUDIO.md`.
//! * [`text`] / [`widgets`]: sprite fonts from the disc and the menu list widget.
//! * [`stage_scene`] / [`stage_info`] / [`anim`]: assembling an SMB1 stage into renderer objects;
//!   [`backdrop`]: what the night, storm and sunset backgrounds animate; [`mirror`]: which stages have a
//!   mirror floor and in which plane.
//! * [`import`]: copying the files the game uses out of a disc image into a game pack, a piece at a time;
//!   [`import_screen`]: the state that shows it on a native platform, the copying on a thread of its own.
//! * [`ui`]: safe-area layout and the on-screen touch controls. [`gpu`]: what every driver does the same
//!   way when it sets the GPU up. [`perf`]: frame statistics for the `?fps=1` overlay and the browser check.
//! * Behind the `dev` feature (on for this crate's tests, off in the product): `headless` (windowless
//!   driver with scripted input), `autopilot` and `image`. See `docs/GAMEPLAY.md`.

pub mod anim;
pub mod audio;
// Device output through cpal on native targets. The browser driver has its own Web Audio backend.
#[cfg(not(target_arch = "wasm32"))]
pub mod audio_cpal;
// A closed-loop player for scripted runs and tests.
#[cfg(feature = "dev")]
pub mod autopilot;
pub mod backdrop;
pub mod content;
pub mod game;
pub mod gpu;
// The windowless driver of the integration tests. It reads pixels back by blocking on the
// GPU, which the browser cannot do.
#[cfg(all(feature = "dev", not(target_arch = "wasm32")))]
pub mod headless;
pub mod host;
pub mod hud;
// PNG writer for the screenshots of scripted runs.
#[cfg(all(feature = "dev", not(target_arch = "wasm32")))]
pub mod image;
pub mod import;
// Copies on a thread of its own, which the browser does not have (its page has the import screen).
#[cfg(not(target_arch = "wasm32"))]
pub mod import_screen;
pub mod input;
pub mod loading;
pub mod menu;
pub mod mirror;
pub mod particles;
pub mod perf;
pub mod play;
pub mod play_scene;
pub mod stage_info;
pub mod stage_scene;
pub mod text;
pub mod tilt;
pub mod ui;
pub mod widgets;

/// Re-exported so drivers can implement [`audio::AudioBackend`] (its buffers are `mb_audio` sounds).
pub use mb_audio;
/// Re-exported so drivers can name `Storage` implementations without depending on `mb-game` themselves.
pub use mb_game;

/// The state a platform driver starts in once content is open: the title menu. `storage` is where the
/// profile (progress, settings) is kept; `audio` is the platform's audio system (see [`audio::Audio`]), of
/// which the driver keeps a clone to suspend it with the app.
pub fn initial_game_with_audio(
    storage: Box<dyn mb_game::Storage>,
    audio: audio::Audio,
) -> Box<dyn game::Game> {
    Box::new(menu::Menu::new(storage).with_audio(audio))
}
