//! Shared by the disc-backed tests of this crate.

use std::path::PathBuf;

/// The developer's own disc image: `MB_DISC`, or `Super Monkey Ball*.rvz|iso|gcm` in the directory that
/// contains the repository. The tests skip without one.
pub fn find_disc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MB_DISC") {
        return Some(PathBuf::from(p));
    }
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    std::fs::read_dir(parent).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let image = [".rvz", ".iso", ".gcm"].iter().any(|e| name.ends_with(e));
        (name.starts_with("super monkey ball") && image).then(|| entry.path())
    })
}
