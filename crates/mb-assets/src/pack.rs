//! Pack providers: where content bytes come from. Mirrors WebMonkeyBall's `PackProvider` in `src/pack.ts`.

use std::collections::HashMap;
use std::io;

/// Normalize a pack path: strip leading `./` and `/`, use forward slashes.
pub fn normalize_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

/// Disc path of a content-root path: `smb1_content/test/x` and `test/x` both name `test/x`.
pub fn disc_path(path: &str) -> String {
    let norm = normalize_path(path);
    match norm.split_once('/') {
        Some((first, rest)) if first.ends_with("_content") => rest.to_string(),
        _ => norm,
    }
}

pub trait PackProvider: Send + Sync {
    /// The bytes of `path`; `ErrorKind::NotFound` when the source has no such file.
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>>;
    /// Whether the source has `path`, without reading it.
    fn contains(&self, path: &str) -> bool;
}

/// In-memory pack keyed by disc path (tests; the browser's prepared files).
#[derive(Default)]
pub struct MemoryPack {
    files: HashMap<String, Vec<u8>>,
}

impl MemoryPack {
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>) {
        self.files.insert(disc_path(path), bytes);
    }

    pub fn remove(&mut self, path: &str) -> Option<Vec<u8>> {
        self.files.remove(&disc_path(path))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Total size of the files held.
    pub fn bytes(&self) -> usize {
        self.files.values().map(Vec::len).sum()
    }
}

impl PackProvider for MemoryPack {
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
        let key = disc_path(path);
        self.files.get(&key).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing pack entry: {key}"),
            )
        })
    }

    fn contains(&self, path: &str) -> bool {
        self.files.contains_key(&disc_path(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize_path("./a\\b/c"), "a/b/c");
        assert_eq!(normalize_path("/a/b"), "a/b");
        assert_eq!(disc_path("smb1_content/test/a"), "test/a");
        assert_eq!(disc_path("./test/a"), "test/a");
    }

    #[test]
    fn memory_pack_serves_by_disc_path() {
        let mut m = MemoryPack::default();
        m.insert("smb1_content/x/y.bin", vec![1, 2, 3]);
        assert_eq!(m.fetch("x/y.bin").unwrap(), vec![1, 2, 3]);
        assert!(m.contains("./x/y.bin") && !m.contains("nope"));
        assert_eq!((m.len(), m.bytes()), (1, 3));
        assert_eq!(m.fetch("nope").unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(m.remove("x/y.bin"), Some(vec![1, 2, 3]));
        assert!(m.is_empty());
    }
}
