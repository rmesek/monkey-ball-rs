//! Where save data lives. Desktop uses [`FileStorage`] (a directory of small files); the browser and
//! Android shells will implement [`Storage`] over `localStorage` / app-private files. Keys are short ASCII
//! names (`profile`); values are opaque bytes.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

pub trait Storage {
    /// `Ok(None)` when the key was never written.
    fn read(&self, key: &str) -> io::Result<Option<Vec<u8>>>;
    fn write(&mut self, key: &str, data: &[u8]) -> io::Result<()>;
    fn remove(&mut self, key: &str) -> io::Result<()>;
}

fn valid_key(key: &str) -> io::Result<()> {
    if !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
        && !key.starts_with('.')
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("bad storage key {key:?}"),
        ))
    }
}

/// Process-local storage (tests, and the fallback when no writable location exists).
#[derive(Default, Debug)]
pub struct MemoryStorage {
    map: HashMap<String, Vec<u8>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Storage for MemoryStorage {
    fn read(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
        valid_key(key)?;
        Ok(self.map.get(key).cloned())
    }
    fn write(&mut self, key: &str, data: &[u8]) -> io::Result<()> {
        valid_key(key)?;
        self.map.insert(key.to_string(), data.to_vec());
        Ok(())
    }
    fn remove(&mut self, key: &str) -> io::Result<()> {
        valid_key(key)?;
        self.map.remove(key);
        Ok(())
    }
}

/// One file per key inside a directory. Writes go to a temp file first and are renamed into place, so a
/// crash mid-write never leaves a half-written save.
#[derive(Debug, Clone)]
pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Per-user data directory: `MB_SAVE_DIR`, else `%APPDATA%\monkey-ball-rs`,
    /// `~/Library/Application Support/monkey-ball-rs` or `$XDG_DATA_HOME|~/.local/share/monkey-ball-rs`.
    pub fn default_dir() -> Option<PathBuf> {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        if let Some(d) = var("MB_SAVE_DIR") {
            return Some(PathBuf::from(d));
        }
        if cfg!(windows) {
            return var("APPDATA").map(|d| PathBuf::from(d).join("monkey-ball-rs"));
        }
        if cfg!(target_os = "macos") {
            return var("HOME").map(|d| {
                PathBuf::from(d)
                    .join("Library/Application Support")
                    .join("monkey-ball-rs")
            });
        }
        var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("monkey-ball-rs"))
    }

    fn path(&self, key: &str) -> io::Result<PathBuf> {
        valid_key(key)?;
        Ok(self.dir.join(key))
    }
}

impl Storage for FileStorage {
    fn read(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.path(key)?) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn write(&mut self, key: &str, data: &[u8]) -> io::Result<()> {
        let path = self.path(key)?;
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!("{key}.tmp"));
        std::fs::write(&tmp, data)?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(_) => {
                // Some platforms refuse to rename over an existing file.
                let _ = std::fs::remove_file(&path);
                std::fs::rename(&tmp, &path)
            }
        }
    }

    fn remove(&mut self, key: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(key)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exercise(s: &mut dyn Storage) {
        assert_eq!(s.read("a").unwrap(), None);
        s.write("a", b"one").unwrap();
        s.write("a", b"two").unwrap();
        assert_eq!(s.read("a").unwrap().as_deref(), Some(&b"two"[..]));
        s.remove("a").unwrap();
        s.remove("a").unwrap();
        assert_eq!(s.read("a").unwrap(), None);
        for bad in ["", "../x", "a/b", ".hidden", "a b"] {
            assert!(s.write(bad, b"x").is_err(), "{bad:?}");
        }
    }

    #[test]
    fn memory_backend() {
        exercise(&mut MemoryStorage::new());
    }

    #[test]
    fn file_backend() {
        let dir = std::env::temp_dir().join(format!("mb-game-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        exercise(&mut FileStorage::new(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
