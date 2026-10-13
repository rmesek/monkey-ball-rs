//! The memory budget, counted natively: the peak of live heap bytes while a
//! stage is loaded and played may be at most 2 MB above what version 0.1.0 needed for the same stage,
//! with reflections on.
//!
//! In the browser, wasm memory never shrinks, so what a page shows after a stage is the peak of the
//! heap up to then. The heap of this native test is a proxy for that: the same loaders, parsers,
//! decoders, scene code and `wgpu` run, under an allocator that counts, on `wgpu`'s OpenGL backend,
//! which is the one the browser build uses for WebGL2 and whose command lists live in the heap. It is
//! not the same number:
//!
//! * The music is off here. In the browser a track is decoded straight into the browser's own buffers,
//!   outside wasm memory; natively its 15 to 54 MB of samples would be the peak of every load.
//! * The disc image is read in place here; the browser holds the stage's raw files (up to 10.6 MB)
//!   while it parses them. That is the same in 0.1.0.
//! * WebGL2 keeps a staging copy of every texture and buffer it is given until the next submit, in
//!   wasm memory; desktop OpenGL does not. The renderer counts those bytes itself
//!   (`LoadStats::staged_peak_bytes`) and the test holds them to their limit.
//! * A wasm allocator cannot return or move memory, so a buffer that grows by doubling costs the
//!   browser up to twice its final size; here it counts once, plus the copy while it grows.
//!
//! What the proxy does show is every buffer the game and `wgpu` allocate for a stage and its frames,
//! which is where a regression against 0.1.0 comes from.
//!
//! The 0.1.0 figures were measured with this file's `measure` in a checkout of the `v0.1.0` tag (its
//! `Headless` given the same choice of backend), on the same machine and disc:
//! `MB_MEMORY_PRINT=1 cargo test -p mb-app --release --test memory -- --nocapture` prints the rows as
//! measured instead of comparing them. Like the other tests this needs the developer's own disc image
//! and a GPU adapter with OpenGL, and skips without either.

mod common;

use common::find_disc;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use mb_app::content::open_disc_image;
use mb_app::headless::{FRAME, Headless};
use mb_app::menu::{Autostart, Menu};

/// Live bytes and their high-water mark.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let now = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            grew(new_size);
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// A save with every stage open, the music off (see the top of the file) and the Reflections option
/// as given.
fn quiet_storage(reflections: bool) -> Box<mb_game::MemoryStorage> {
    let mut storage = mb_game::MemoryStorage::new();
    let mut profile = mb_game::Profile::default();
    profile.settings.unlock_all = true;
    profile.settings.music_volume = 0;
    profile.settings.reflections = reflections;
    profile.save(&mut storage).unwrap();
    Box::new(storage)
}

/// What a stage needed, in bytes above the heap of the idle game (device, renderer, no state).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Usage {
    /// Most live bytes at any moment of the load and of 480 drawn frames (the fly-in and two seconds
    /// of play).
    peak: usize,
    /// Live bytes when the load is over and the first frame is drawn: the stage at rest, and the
    /// command lists of that one frame, which `wgpu` frees with the next.
    loaded: usize,
    /// Most live bytes during the 480 frames: `loaded` plus what a frame needs for a moment (the
    /// command lists of its passes, the picture this test reads back).
    playing: usize,
    /// `LoadStats::staged_peak_bytes` of the load.
    staged: usize,
}

/// Load `stage` from the title menu's autostart, as practice, and play it.
fn measure(h: &mut Headless, stage: u32, reflections: bool) -> Usage {
    h.shutdown();
    let idle = LIVE.load(Ordering::Relaxed);
    PEAK.store(idle, Ordering::Relaxed);
    let menu = Menu::new(quiet_storage(reflections)).with_autostart(Some(Autostart::Stage(stage)));
    h.push(Box::new(menu))
        .unwrap_or_else(|e| panic!("stage {stage}: {e}"));
    assert!(h.step(FRAME).unwrap());
    assert!(h.top().unwrap().play_view().is_some(), "stage {stage}");
    let staged = h.renderer().load_stats.staged_peak_bytes;
    h.render().unwrap();
    let loaded = LIVE.load(Ordering::Relaxed).saturating_sub(idle);
    let load_peak = PEAK.load(Ordering::Relaxed).saturating_sub(idle);
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    for _ in 0..480 {
        assert!(h.step(FRAME).unwrap());
        h.render().unwrap();
    }
    let playing = PEAK.load(Ordering::Relaxed).saturating_sub(idle);
    h.shutdown();
    Usage {
        peak: load_peak.max(playing),
        loaded,
        playing,
        staged,
    }
}

/// `(stage, peak, live after the load, peak in play)` of version 0.1.0, in bytes: a plain stage, a night
/// stage (window flipbooks and a mirror now), an ice stage (a mirror now), two storm stages (fire
/// flipbooks now, and a mirror on 89), the bonus wave (a mirror now) and the stage with the most
/// textures (a mirror now).
const USAGE_0_1_0: &[(u32, usize, usize, usize)] = &[
    (1, 7_479_545, 5_982_562, 6_906_626),
    (21, 8_615_681, 6_666_651, 7_872_040),
    (76, 7_904_198, 6_300_097, 7_226_542),
    (89, 8_713_365, 6_470_144, 8_288_389),
    (90, 7_960_486, 5_977_520, 6_900_816),
    (92, 5_555_553, 3_950_840, 4_902_774),
    (109, 12_520_312, 7_015_209, 8_284_190),
];

/// What the peak of a stage may be above its recorded figure.
const GROWTH_BUDGET: usize = 2_000_000;

#[test]
fn loading_and_playing_a_stage_peaks_within_two_megabytes_of_0_1_0() {
    let Some(disc) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return;
    };
    let content = match open_disc_image(&disc) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {}: {e}", disc.display());
            return;
        }
    };
    // The OpenGL backend: the one the browser build uses (as WebGL2), whose command lists live in
    // the heap, where a frame's draw calls are counted.
    let gl = Some(mb_render::wgpu::Backends::GL);
    let mut h = match Headless::on_backends(&content, (640, 360), false, gl) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped: no OpenGL adapter ({e})");
            return;
        }
    };
    // A stage first, unmeasured: what the first load of a process sets up for good (the trig tables,
    // the shared files' cache, pipelines, the image reader's buffers) is not any one stage's memory.
    measure(&mut h, 2, true);
    measure(&mut h, 2, true);
    let printing = std::env::var_os("MB_MEMORY_PRINT").is_some();
    let mb = |bytes: usize| bytes as f64 / 1e6;
    let mut over = Vec::new();
    for (stage, peak, loaded, playing) in USAGE_0_1_0.iter().copied() {
        let on = measure(&mut h, stage, true);
        let off = measure(&mut h, stage, false);
        if printing {
            println!(
                "    ({stage}, {}, {}, {}), // reflections off: {}, {}, {}; staged {}",
                on.peak, on.loaded, on.playing, off.peak, off.loaded, off.playing, on.staged
            );
            continue;
        }
        eprintln!(
            "stage {stage:3}: peak {:.2} MB (0.1.0 {:.2}), at rest {:.2} (0.1.0 {:.2}), in play {:.2} \
             (0.1.0 {:.2}); reflections off: peak {:.2}, at rest {:.2}, in play {:.2}; \
             {:.2} MB of uploads staged at most",
            mb(on.peak),
            mb(peak),
            mb(on.loaded),
            mb(loaded),
            mb(on.playing),
            mb(playing),
            mb(off.peak),
            mb(off.loaded),
            mb(off.playing),
            mb(on.staged),
        );
        // With reflections on, which is how the game is played: the budget of the plan, at the peak
        // and for the stage at rest.
        if on.peak > peak + GROWTH_BUDGET {
            over.push(format!(
                "stage {stage}: peak {} bytes, {} more than 0.1.0",
                on.peak,
                on.peak - peak
            ));
        }
        if on.loaded > loaded + GROWTH_BUDGET {
            over.push(format!(
                "stage {stage}: {} bytes at rest, {} more than 0.1.0",
                on.loaded,
                on.loaded - loaded
            ));
        }
        // No load hands the queue more at once than its limit, or than the one upload that is larger
        // than the limit by itself (the top level of a 1024 x 1024 texture, on stage 109).
        let limit = mb_render::Renderer::STAGED_LIMIT.max(1024 * 1024 * 4);
        if on.staged > limit || on.staged == 0 {
            over.push(format!("stage {stage}: {} bytes staged at once", on.staged));
        }
    }
    assert!(over.is_empty(), "over the budget:\n{}", over.join("\n"));
}
