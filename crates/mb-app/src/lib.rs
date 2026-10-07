//! Application shell: windowing, input, game flow, platform glue.
//!
//! * [`content`]: where game files come from (disc image / directory; pickers later).
//! * [`game`]: `Game` state trait and the state stack (menus, play, pause ...).
//! * [`input`]: logical actions and axes fed by keyboard today, gamepad/touch later.
//! * [`stage_scene`] / [`stage_info`]: assembling an SMB1 stage into renderer objects.
//! * [`viewer`]: the first state, a free-fly stage viewer.
//! * [`app`] (windowed) and [`headless`] (offscreen capture) are the two drivers.

pub mod anim;
pub mod app;
pub mod content;
pub mod game;
pub mod gpu;
pub mod headless;
pub mod image;
pub mod input;
pub mod shell;
pub mod stage_info;
pub mod stage_scene;
pub mod ui;
pub mod viewer;
