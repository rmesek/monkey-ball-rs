//! Application shell: windowing, input, game flow, platform glue.
//!
//! * [`content`]: where game files come from (disc image / directory).
//! * [`game`]: `Game` state trait and the state stack.
//! * [`input`]: logical actions, axes, the steering vector and the pointer; shells only feed it.
//! * [`menu`]: title menu, course / stage select, options and the course flow (root state).
//! * [`play`] + [`play_scene`] + [`hud`]: one stage being played: fixed-step simulation, 3D scene, HUD.
//! * [`text`] / [`widgets`]: sprite fonts from the disc and the menu list widget.
//! * [`stage_scene`] / [`stage_info`] / [`anim`]: assembling an SMB1 stage into renderer objects.
//! * [`viewer`]: free-fly stage viewer (developer tool).
//! * [`app`] (desktop window), [`shell`] (web / mobile window) and [`headless`] (offscreen, scripted) are
//!   the drivers. See `docs/GAMEPLAY.md`.

pub mod anim;
// The desktop driver blocks on GPU setup and polls native gamepads; the browser uses `shell`.
#[cfg(not(target_arch = "wasm32"))]
pub mod app;
pub mod autopilot;
pub mod content;
pub mod game;
pub mod gpu;
// Offscreen capture reads pixels back by blocking on the GPU, which the browser cannot do.
#[cfg(not(target_arch = "wasm32"))]
pub mod headless;
pub mod hud;
pub mod image;
pub mod input;
pub mod menu;
pub mod play;
pub mod play_scene;
pub mod shell;
pub mod stage_info;
pub mod stage_scene;
pub mod text;
pub mod ui;
pub mod viewer;
pub mod widgets;

/// Re-exported so shells can name `Storage` implementations without depending on `mb-game` themselves.
pub use mb_game;

/// The state every platform shell starts in once content is open: the title menu. `storage` is where the
/// profile (progress, settings) is kept: `mb_game::FileStorage` on desktop, the platform's own elsewhere.
pub fn initial_game(storage: Box<dyn mb_game::Storage>) -> Box<dyn game::Game> {
    Box::new(menu::Menu::new(storage))
}
