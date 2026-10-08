//! Game files from the user's own disc image: container and disc readers, LZSS, pack providers and the
//! on-device game pack. No GPU, no OS-specific code, no game data.
//!
//! * [`disc`]: GameCube disc reader (boot header, file table, `main.dol`) over any byte stream.
//! * [`rvz`]: the RVZ container (none / Zstandard).
//! * [`lzss`]: the game's `.lz` compression.
//! * [`pack`]: the `PackProvider` trait the loaders read through.
//! * [`gamepack`]: the single-file store the browser import writes and the game reads back.

pub mod disc;
pub mod gamepack;
pub mod lzss;
pub mod pack;
pub mod rvz;

#[cfg(test)]
mod test_alloc;

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: test_alloc::Tracking = test_alloc::Tracking;
