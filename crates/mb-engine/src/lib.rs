//! Deterministic SMB1 simulation core: a port of WebMonkeyBall's `physics.ts`, `collision.ts`, the runtime part
//! of `stage.ts`, `math.ts`, `animation.ts`, `rng.ts`, `determinism.ts`, `sim_hash.ts`, `effects.ts`, the
//! gameplay camera and the per-tick flow of `game.ts`. Renderer- and platform-agnostic (std only, no globals except the trig tables).
//!
//! Numerics follow the reference exactly (see [`math`]); bit-exact replays additionally need the game's own trig
//! tables, loaded with [`Tables::from_dol`] and installed once via [`install_tables`] before the first [`Sim`].
//! See `docs/ENGINE.md`.

pub mod animation;
pub mod ball;
pub mod camera;
pub mod collision;
pub mod constants;
pub mod determinism;
pub mod effects;
pub mod geom;
pub mod hash;
pub mod math;
pub mod objects;
pub mod rng;
pub mod sim;
pub mod stage_runtime;
pub mod tables;
pub mod world;

pub use ball::{Ball, BallAudio, GoalHit};
pub use camera::GameplayCamera;
pub use determinism::{BUTTON_PRIMARY_ACTION, QuantizedInput};
pub use effects::{BallEffect, BallEffectKind, ConfettiParticle};
pub use hash::{hash_sim_state, hash_sim_state_visual};
pub use sim::{Outcome, Phase, Sim, SimConfig, SimEvent};
pub use stage_runtime::StageRuntime;
pub use tables::{Tables, install_tables};
pub use world::World;
