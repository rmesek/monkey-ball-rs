//! SMB1 stagedef parser (collision triangles, grid, goals, bumpers, bananas, animations).
//!
//! Input is the *decompressed* `STAGENNN.lz` payload (decompress with `mb_assets::lzss`). Std-only.
//! See `docs/STAGEDEF.md` for the layout.

mod parse;
mod types;

pub use parse::*;
pub use types::*;

#[cfg(test)]
mod tests;
