//! Where save data lives. Desktop uses [`FileStorage`] (a directory of small files); the browser shell
//! implements [`Storage`] over the browser's own stores. Keys are short ASCII names (`profile`); values are
//! opaque bytes.
//!
//! Both go through [`SlotStorage`], which makes every value survive a sudden exit: a value is kept in two
//! slots, each record carrying a generation number and a checksum, and a write always replaces the older
//! slot. Whatever instant the process dies at, a read afterwards finds either the previous complete value
//! or the new complete one, on any backend, with no need for an atomic rename.

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
        use std::io::Write;
        let path = self.path(key)?;
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!("{key}.tmp"));
        {
            // On disk before it is renamed into place, so a power cut cannot leave an empty file there.
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(data)?;
            file.sync_all()?;
        }
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

const SLOT_MAGIC: &[u8; 4] = b"MBSL";
const SLOT_HEADER: usize = 20;
const SLOTS: [&str; 2] = ["a", "b"];

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(0x0100_0193)
    })
}

/// A slot record: magic, generation, length and checksum of the value, then the value.
fn encode_slot(generation: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SLOT_HEADER + data.len());
    out.extend(SLOT_MAGIC);
    out.extend(generation.to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(fnv1a(data).to_le_bytes());
    out.extend(data);
    out
}

/// The generation and value of a record, or `None` when it is torn, truncated or not a record at all.
fn decode_slot(record: &[u8]) -> Option<(u64, &[u8])> {
    if record.len() < SLOT_HEADER || &record[..4] != SLOT_MAGIC {
        return None;
    }
    let generation = u64::from_le_bytes(record[4..12].try_into().ok()?);
    let len = u32::from_le_bytes(record[12..16].try_into().ok()?) as usize;
    let sum = u32::from_le_bytes(record[16..20].try_into().ok()?);
    let data = &record[SLOT_HEADER..];
    (data.len() == len && fnv1a(data) == sum).then_some((generation, data))
}

/// Crash-safe values over any [`Storage`]: see the module documentation.
///
/// `key` is stored as `key.a` and `key.b`. A plain `key` written by an older build is still read (and
/// replaced by slots at the next write).
#[derive(Debug, Default, Clone)]
pub struct SlotStorage<S: Storage> {
    inner: S,
}

impl<S: Storage> SlotStorage<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// The newest complete record of `key`: its slot index, generation and value. Also says whether every
    /// slot holds something that is not a complete record. (One such slot is a write that was cut short,
    /// which is expected after a crash; both cannot come from a crash and means the store itself is
    /// damaged.)
    #[allow(clippy::type_complexity)]
    fn newest(&self, key: &str) -> io::Result<(Option<(usize, u64, Vec<u8>)>, bool)> {
        let mut best: Option<(usize, u64, Vec<u8>)> = None;
        let mut torn = 0;
        let mut last_error = None;
        for (i, slot) in SLOTS.iter().enumerate() {
            match self.inner.read(&format!("{key}.{slot}")) {
                Ok(None) => {}
                Ok(Some(record)) => match decode_slot(&record) {
                    Some((generation, data)) => {
                        if best.as_ref().is_none_or(|b| generation > b.1) {
                            best = Some((i, generation, data.to_vec()));
                        }
                    }
                    None => torn += 1,
                },
                Err(e) => last_error = Some(e),
            }
        }
        let damaged = torn == SLOTS.len();
        match (best, last_error) {
            // One readable, complete slot is enough.
            (Some(b), _) => Ok((Some(b), damaged)),
            (None, Some(e)) => Err(e),
            (None, None) => Ok((None, damaged)),
        }
    }
}

impl<S: Storage> Storage for SlotStorage<S> {
    fn read(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
        valid_key(key)?;
        match self.newest(key)? {
            (Some((_, _, data)), _) => Ok(Some(data)),
            (None, damaged) => match self.inner.read(key)? {
                Some(legacy) => Ok(Some(legacy)),
                None if damaged => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the saved value is damaged",
                )),
                None => Ok(None),
            },
        }
    }

    fn write(&mut self, key: &str, data: &[u8]) -> io::Result<()> {
        valid_key(key)?;
        // Never the slot that holds the newest complete value: that one stays until this write is whole.
        let (slot, generation) = match self.newest(key) {
            Ok((Some((i, generation, _)), _)) => (1 - i, generation + 1),
            _ => (0, 1),
        };
        self.inner.write(
            &format!("{key}.{}", SLOTS[slot]),
            &encode_slot(generation, data),
        )?;
        // A value from before slots existed is superseded now.
        let _ = self.inner.remove(key);
        Ok(())
    }

    fn remove(&mut self, key: &str) -> io::Result<()> {
        valid_key(key)?;
        let mut result = self.inner.remove(key);
        for slot in SLOTS {
            result = result.and(self.inner.remove(&format!("{key}.{slot}")));
        }
        result
    }
}

impl<S: Storage + ?Sized> Storage for Box<S> {
    fn read(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
        (**self).read(key)
    }
    fn write(&mut self, key: &str, data: &[u8]) -> io::Result<()> {
        (**self).write(key, data)
    }
    fn remove(&mut self, key: &str) -> io::Result<()> {
        (**self).remove(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A backend that dies in the middle of an operation: operation number `fail_at` (counting writes and
    /// removes from zero) leaves only the first `keep` bytes of what it was writing (a remove does
    /// nothing), and it and everything after it fail. The map is shared, so a "restart" can look at what
    /// was left behind.
    struct Dying {
        map: Rc<RefCell<HashMap<String, Vec<u8>>>>,
        ops: usize,
        fail_at: usize,
        keep: usize,
    }

    impl Storage for Dying {
        fn read(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
            Ok(self.map.borrow().get(key).cloned())
        }
        fn write(&mut self, key: &str, data: &[u8]) -> io::Result<()> {
            let n = self.ops;
            self.ops += 1;
            if n < self.fail_at {
                self.map.borrow_mut().insert(key.to_string(), data.to_vec());
                return Ok(());
            }
            if n == self.fail_at {
                let torn = data[..self.keep.min(data.len())].to_vec();
                self.map.borrow_mut().insert(key.to_string(), torn);
            }
            Err(io::Error::other("power cut"))
        }
        fn remove(&mut self, key: &str) -> io::Result<()> {
            let n = self.ops;
            self.ops += 1;
            if n < self.fail_at {
                self.map.borrow_mut().remove(key);
                return Ok(());
            }
            Err(io::Error::other("power cut"))
        }
    }

    /// Save a sequence of values and die at every possible point of every write, with every possible
    /// amount of the interrupted write on "disk": after a restart the value read back is the last one
    /// whose save returned, or the one being saved, never anything else and never an error.
    #[test]
    fn a_crash_at_any_point_leaves_the_old_or_the_new_value() {
        let values: Vec<Vec<u8>> = (0..5u8)
            .map(|i| (0..20 + i * 7).map(|b| b ^ (i * 31)).collect())
            .collect();
        let mut crashes = 0;
        for fail_at in 0..12 {
            let longest = SLOT_HEADER + values.iter().map(Vec::len).max().unwrap();
            for keep in 0..=longest {
                let map = Rc::new(RefCell::new(HashMap::new()));
                let mut storage = SlotStorage::new(Dying {
                    map: map.clone(),
                    ops: 0,
                    fail_at,
                    keep,
                });
                let mut committed: Option<&Vec<u8>> = None;
                let mut in_flight: Option<&Vec<u8>> = None;
                for v in &values {
                    match storage.write("profile", v) {
                        Ok(()) => committed = Some(v),
                        Err(_) => {
                            in_flight = Some(v);
                            crashes += 1;
                            break;
                        }
                    }
                }
                // Restart on what is left.
                let after = SlotStorage::new(Dying {
                    map,
                    ops: 0,
                    fail_at: usize::MAX,
                    keep: 0,
                });
                let read = after
                    .read("profile")
                    .unwrap_or_else(|e| panic!("op {fail_at}, {keep} bytes kept: {e}"));
                let ok = read.as_ref() == committed
                    || (in_flight.is_some() && read.as_ref() == in_flight);
                assert!(ok, "op {fail_at}, {keep} bytes kept: read {read:?}");
                // And saving works again afterwards.
                let mut after = after;
                after.write("profile", b"next").unwrap();
                assert_eq!(
                    after.read("profile").unwrap().as_deref(),
                    Some(&b"next"[..])
                );
            }
        }
        assert!(crashes > 300, "only {crashes} crash points were exercised");
    }

    #[test]
    fn slots_alternate_and_survive_damage_to_either() {
        let mut s = SlotStorage::new(MemoryStorage::new());
        assert_eq!(s.read("k").unwrap(), None);
        s.write("k", b"one").unwrap();
        s.write("k", b"two").unwrap();
        s.write("k", b"three").unwrap();
        assert_eq!(s.read("k").unwrap().as_deref(), Some(&b"three"[..]));
        // The older slot still holds the value before.
        let older = s.inner.read("k.b").unwrap().unwrap();
        assert_eq!(decode_slot(&older).unwrap(), (2, &b"two"[..]));
        // Flip a bit in the newest slot: the previous value comes back.
        let mut newest = s.inner.read("k.a").unwrap().unwrap();
        *newest.last_mut().unwrap() ^= 1;
        s.inner.write("k.a", &newest).unwrap();
        assert_eq!(s.read("k").unwrap().as_deref(), Some(&b"two"[..]));
        // Both damaged: an error, not a silent reset.
        s.inner.write("k.b", b"MBSLgarbage").unwrap();
        assert!(s.read("k").is_err());
        // The next save repairs it.
        s.write("k", b"four").unwrap();
        assert_eq!(s.read("k").unwrap().as_deref(), Some(&b"four"[..]));
        s.remove("k").unwrap();
        assert_eq!(s.read("k").unwrap(), None);
        assert_eq!(s.inner.read("k.a").unwrap(), None);
    }

    #[test]
    fn a_value_from_before_slots_is_read_and_then_replaced() {
        let mut inner = MemoryStorage::new();
        inner.write("profile", b"old save").unwrap();
        let mut s = SlotStorage::new(inner);
        assert_eq!(
            s.read("profile").unwrap().as_deref(),
            Some(&b"old save"[..])
        );
        s.write("profile", b"new save").unwrap();
        assert_eq!(
            s.read("profile").unwrap().as_deref(),
            Some(&b"new save"[..])
        );
        assert_eq!(s.inner.read("profile").unwrap(), None);
    }

    #[test]
    fn file_slots_survive_a_truncated_file() {
        let dir = std::env::temp_dir().join(format!("mb-slot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = SlotStorage::new(FileStorage::new(&dir));
        s.write("profile", b"first").unwrap();
        s.write("profile", b"second").unwrap();
        // The newest slot file cut short, as after a crash without the rename trick.
        std::fs::write(dir.join("profile.b"), b"MBSL\x02").unwrap();
        assert_eq!(s.read("profile").unwrap().as_deref(), Some(&b"first"[..]));
        // A stray temp file from an interrupted write is harmless.
        std::fs::write(dir.join("profile.a.tmp"), b"junk").unwrap();
        s.write("profile", b"third").unwrap();
        assert_eq!(s.read("profile").unwrap().as_deref(), Some(&b"third"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
