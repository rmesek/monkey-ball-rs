//! Application shell: windowing, input, game flow, platform glue.
//!
//! * [`content`]: where game files come from (a disc image read in place, or imported game data that is
//!   prepared before it is read) and the lists of files the game needs.
//! * [`loading`]: the stage loading state (fetch, parse, build, with a progress screen).
//! * [`host`] / [`tilt`]: the seam to platform features (fullscreen, removing imported data, device tilt).
//! * [`game`]: `Game` state trait and the state stack.
//! * [`input`]: logical actions, axes, the steering vector and the pointer; shells only feed it.
//! * [`menu`]: title menu, course / stage select, options and the course flow (root state).
//! * [`play`] + [`play_scene`] + [`particles`] + [`hud`]: one stage being played: fixed-step simulation,
//!   3D scene, the particles and shadow on top of it, HUD.
//! * [`audio`]: sounds, music and the backend trait (`audio_cpal`: native device output). See `docs/AUDIO.md`.
//! * [`text`] / [`widgets`]: sprite fonts from the disc and the menu list widget.
//! * [`stage_scene`] / [`stage_info`] / [`anim`]: assembling an SMB1 stage into renderer objects.
//! * The drivers, one per target: [`app`] is the desktop window (blocking GPU setup, vsync pacing, native
//!   gamepads, a mouse pointer) and `shell` is the browser canvas (asynchronous GPU setup, frames from
//!   `requestAnimationFrame`, platform hooks for `mb-web`). They share everything above them: the state
//!   stack, `InputState`, the touch controls, the renderer.
//! * Behind the `dev` feature (on for this crate's tests, off in the product): `headless` (windowless
//!   driver with scripted input), `autopilot` and `image`. See `docs/GAMEPLAY.md`.

pub mod anim;
// The desktop driver; the browser's is `shell`.
#[cfg(not(target_arch = "wasm32"))]
pub mod app;
pub mod audio;
// Device output through cpal on desktop. The browser shell has its own Web Audio backend.
#[cfg(not(target_arch = "wasm32"))]
pub mod audio_cpal;
// A closed-loop player for scripted runs and tests.
#[cfg(feature = "dev")]
pub mod autopilot;
pub mod content;
pub mod game;
pub mod gpu;
// The windowless driver of the integration tests and of `--autoplay`. It reads pixels back by blocking on the
// GPU, which the browser cannot do.
#[cfg(all(feature = "dev", not(target_arch = "wasm32")))]
pub mod headless;
pub mod host;
pub mod hud;
// PNG writer for the screenshots of scripted runs.
#[cfg(all(feature = "dev", not(target_arch = "wasm32")))]
pub mod image;
pub mod import;
pub mod input;
pub mod loading;
pub mod menu;
pub mod particles;
pub mod perf;
pub mod play;
pub mod play_scene;
// The browser driver (`mb-web` supplies the platform hooks).
#[cfg(target_arch = "wasm32")]
pub mod shell;
pub mod stage_info;
pub mod stage_scene;
pub mod text;
pub mod tilt;
pub mod ui;
pub mod widgets;

/// Re-exported so shells can implement [`audio::AudioBackend`] (its buffers are `mb_audio` sounds).
pub use mb_audio;
/// Re-exported so shells can name `Storage` implementations without depending on `mb-game` themselves.
pub use mb_game;

/// The state every platform shell starts in once content is open: the title menu. `storage` is where the
/// profile (progress, settings) is kept: `mb_game::FileStorage` on desktop, the platform's own elsewhere.
pub fn initial_game(storage: Box<dyn mb_game::Storage>) -> Box<dyn game::Game> {
    Box::new(menu::Menu::new(storage))
}

/// [`initial_game`] with sound: `audio` is the platform's audio system (see [`audio::Audio`]); the shell
/// keeps a clone to suspend it with the app.
pub fn initial_game_with_audio(
    storage: Box<dyn mb_game::Storage>,
    audio: audio::Audio,
) -> Box<dyn game::Game> {
    Box::new(menu::Menu::new(storage).with_audio(audio))
}
