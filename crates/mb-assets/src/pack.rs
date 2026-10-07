//! Pack providers: where content bytes come from. Mirrors WebMonkeyBall `PackProvider` /
//! `fetchWithCache` in `src/pack.ts`. Zip and HTTP backends are added once their crates are chosen.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Normalize a pack path: strip leading `./` and `/`, use forward slashes.
pub fn normalize_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

pub trait PackProvider: Send + Sync {
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>>;
}

/// Reads files below a content root directory.
pub struct DirPack {
    root: PathBuf,
}

impl DirPack {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl PackProvider for DirPack {
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
        let norm = normalize_path(path);
        if norm.split('/').any(|c| c == "..") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path escapes content root",
            ));
        }
        std::fs::read(self.root.join(norm))
    }
}

/// In-memory pack (tests, user-picked files on wasm).
#[derive(Default)]
pub struct MemoryPack {
    files: HashMap<String, Vec<u8>>,
}

impl MemoryPack {
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>) {
        self.files.insert(normalize_path(path), bytes);
    }
}

impl PackProvider for MemoryPack {
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
        let norm = normalize_path(path);
        self.files.get(&norm).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing pack entry: {norm}"),
            )
        })
    }
}

/// Caches fetched files by normalized path so repeated loads (stage + prefetch) hit the provider once.
pub struct CachedPack<P: PackProvider> {
    inner: P,
    cache: Mutex<HashMap<String, Arc<Vec<u8>>>>,
}

impl<P: PackProvider> CachedPack<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, path: &str) -> io::Result<Arc<Vec<u8>>> {
        let key = normalize_path(path);
        if let Some(hit) = self.cache.lock().unwrap().get(&key) {
            return Ok(hit.clone());
        }
        let bytes = Arc::new(self.inner.fetch(&key)?);
        self.cache.lock().unwrap().insert(key, bytes.clone());
        Ok(bytes)
    }

    /// Best-effort warm-up of upcoming files; errors are ignored like `prefetchPackSlice`.
    pub fn prefetch<'a>(&self, paths: impl IntoIterator<Item = &'a str>) {
        for p in paths {
            let _ = self.get(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(MemoryPack, Arc<AtomicUsize>);
    impl PackProvider for Counting {
        fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.fetch(path)
        }
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize_path("./a\\b/c"), "a/b/c");
        assert_eq!(normalize_path("/a/b"), "a/b");
    }

    #[test]
    fn cache_fetches_once() {
        let mut m = MemoryPack::default();
        m.insert("x/y.bin", vec![1, 2, 3]);
        let n = Arc::new(AtomicUsize::new(0));
        let c = CachedPack::new(Counting(m, n.clone()));
        assert_eq!(*c.get("./x/y.bin").unwrap(), vec![1, 2, 3]);
        assert_eq!(*c.get("x/y.bin").unwrap(), vec![1, 2, 3]);
        assert_eq!(n.load(Ordering::SeqCst), 1);
        assert!(c.get("nope").is_err());
    }

    #[test]
    fn dir_pack_rejects_traversal() {
        let d = DirPack::new(".");
        assert!(d.fetch("../secret").is_err());
    }
}
