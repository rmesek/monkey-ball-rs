//! Pack providers: where the game's files come from. The trait the disc image (`disc::DiscPack`) and an
//! imported game pack (`gamepack::PackBytes`) both implement, and the one spelling of a file's name.

use std::io;

/// A file's name as the game uses it: the path on the disc with forward slashes and no leading `./` or `/`
/// (`test/st001/STAGE001.lz`).
pub fn disc_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

pub trait PackProvider: Send + Sync {
    /// The bytes of `path`; `ErrorKind::NotFound` when the source has no such file.
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>>;
    /// Whether the source has `path`, without reading it.
    fn contains(&self, path: &str) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(disc_path("./a\\b/c"), "a/b/c");
        assert_eq!(disc_path("/a/b"), "a/b");
        assert_eq!(disc_path("test/a"), "test/a");
    }
}
