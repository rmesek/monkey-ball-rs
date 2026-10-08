//! Shared helpers of the disc-backed and mutation tests (the same file in `mb-stage`, `mb-gma`, `mb-tpl`).
#![allow(dead_code)]

use mb_assets::disc::DiscPack;
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Heap measurements are process-wide, so tests that take them hold this lock.
pub fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn find_disc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MB_DISC") {
        return Some(PathBuf::from(p));
    }
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for entry in std::fs::read_dir(parent).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("super monkey ball")
            && (name.ends_with(".rvz") || name.ends_with(".iso") || name.ends_with(".gcm"))
        {
            let disc = DiscPack::open(entry.path()).ok()?;
            if disc.smb1_region().is_some() {
                return Some(entry.path());
            }
        }
    }
    None
}

/// The disc, or `None` (with a note on stderr) when no image is available.
pub fn open_disc() -> Option<DiscPack> {
    match find_disc() {
        Some(p) => Some(DiscPack::open(p).expect("open disc")),
        None => {
            eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
            None
        }
    }
}

/// Sorted disc paths accepted by `keep`.
pub fn disc_paths(disc: &DiscPack, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let mut v: Vec<String> = disc
        .list()
        .into_iter()
        .filter(|f| keep(f))
        .map(String::from)
        .collect();
    v.sort();
    v
}

/// File bytes, LZSS-decompressed when the name ends in `.lz`. Held in memory only.
pub fn fetch(disc: &DiscPack, path: &str) -> Vec<u8> {
    let raw = disc.fetch(path).unwrap();
    if path.ends_with(".lz") {
        lzss::decompress(&raw).unwrap_or_else(|e| panic!("{path}: {e}"))
    } else {
        raw
    }
}

/// FNV-1a (64-bit) that can be fed `Debug` output without building the string.
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    pub fn debug(&mut self, v: &dyn std::fmt::Debug) {
        write!(self, "{v:?}").unwrap();
    }
}

impl Write for Fnv {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

/// xorshift64*: fixed-seed, dependency-free PRNG for the mutation campaigns.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Values that break naive count / offset handling. `len` is the input length.
pub fn hostile_u32(len: usize) -> Vec<u32> {
    let l = len as u32;
    vec![
        0,
        1,
        2,
        0x7f,
        0x80,
        0xff,
        0x100,
        0x7fff,
        0x8000,
        0xffff,
        0x1_0000,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_fff0,
        0xffff_fffe,
        u32::MAX,
        l.wrapping_sub(4),
        l.wrapping_sub(1),
        l,
        l.wrapping_add(1),
        l / 2,
    ]
}

/// One corrupted copy of `src`: random byte edits, hostile big-endian words at 4-aligned offsets, or a word
/// that points at or just before itself.
pub fn mutate(src: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut b = src.to_vec();
    if b.is_empty() {
        return b;
    }
    match rng.below(4) {
        0 => {
            let i = rng.below(b.len());
            b[i] = rng.next() as u8;
        }
        1 => {
            for _ in 0..1 + rng.below(8) {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
        }
        2 => {
            let h = hostile_u32(b.len());
            for _ in 0..1 + rng.below(3) {
                if b.len() >= 4 {
                    let o = rng.below(b.len() / 4) * 4;
                    let v = h[rng.below(h.len())];
                    b[o..o + 4].copy_from_slice(&v.to_be_bytes());
                }
            }
        }
        _ => {
            if b.len() >= 4 {
                let o = rng.below(b.len() / 4) * 4;
                let delta = [0u32, 4, 8, 0x10][rng.below(4)];
                let v = (o as u32).wrapping_sub(delta);
                b[o..o + 4].copy_from_slice(&v.to_be_bytes());
            }
        }
    }
    b
}

/// Counts live heap bytes and their high-water mark. Install with `#[global_allocator]` in a test binary and
/// measure under [`serial`] with [`peak_heap_growth`].
pub struct CountingAlloc;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let live = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = System.realloc(p, l, new);
        if !q.is_null() {
            if new >= l.size() {
                let live = LIVE.fetch_add(new - l.size(), Ordering::Relaxed) + new - l.size();
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(l.size() - new, Ordering::Relaxed);
            }
        }
        q
    }
}

/// Heap bytes allocated above the level at the call, at the worst moment of `f`.
pub fn peak_heap_growth<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let out = f();
    (out, PEAK.load(Ordering::Relaxed).saturating_sub(before))
}

/// Runs `f` on `input`, failing the test with `what` if it panics. Returns the result and the heap growth.
pub fn run_guarded<T>(what: &str, input: &[u8], f: impl Fn(&[u8]) -> T) -> (T, usize) {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        peak_heap_growth(|| f(input))
    }));
    match r {
        Ok(v) => v,
        Err(_) => panic!("parser panicked on {what} ({} bytes)", input.len()),
    }
}
