//! Test-only allocator that counts what the current thread allocates, so the hostile-input tests can assert
//! "no giant allocation" instead of hoping.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

pub struct Tracking;

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: defers to the system allocator; the counters are plain thread-local integers.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = LIVE.try_with(|live| {
            live.set(live.get() + layout.size());
            let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
        });
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = LIVE.try_with(|live| live.set(live.get().saturating_sub(layout.size())));
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Run `f` and return its result with the most bytes this thread held above its starting level meanwhile.
pub fn peak_during<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let base = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(base));
    let result = f();
    (result, PEAK.with(Cell::get).saturating_sub(base))
}
