//! Frame pacing statistics for the `?fps=1` overlay of the web shell (and anything else that wants them).
//!
//! The driver records one [`FrameSample`] per presented frame into a fixed ring; nothing allocates after
//! construction, so recording is a handful of stores. [`PerfStats::summary`] sorts a copy of the window and is
//! only called when somebody looks (twice a second by the overlay).

use std::cell::Cell;

use crate::stage_scene::LoadTimings;

/// Frames kept (about 17 s at 60 Hz, 8.5 s at 120 Hz).
pub const WINDOW: usize = 1024;

/// Upper edges (ms) of the frame-interval histogram; the last bucket is everything above.
pub const BUCKETS_MS: [f32; 6] = [6.0, 10.0, 14.0, 20.0, 34.0, 50.0];

thread_local! {
    static LAST_LOAD: Cell<Option<LoadTimings>> = const { Cell::new(None) };
}

/// Remember the timings of the stage that was just loaded (one slot; the shells read it on demand).
pub fn set_last_load(t: LoadTimings) {
    LAST_LOAD.with(|l| l.set(Some(t)));
}

pub fn last_load() -> Option<LoadTimings> {
    LAST_LOAD.with(Cell::get)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameSample {
    /// Wall time since the previous frame started.
    pub interval_ms: f32,
    /// Game update: simulation ticks, posing, HUD.
    pub update_ms: f32,
    /// Acquire, encode, submit, present.
    pub render_ms: f32,
    /// Simulation ticks run this frame.
    pub ticks: u32,
    pub draws: u32,
    /// Pipeline switches plus bind-group switches issued this frame.
    pub state_changes: u32,
    /// Bytes of uniform / vertex data written to GPU buffers this frame.
    pub upload_bytes: u32,
    /// Event-loop iterations since the previous frame (1 when the loop sleeps until the next frame).
    pub wakeups: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub avg_ms: f32,
    pub p50_ms: f32,
    pub p99_ms: f32,
    pub worst_ms: f32,
    /// Frames longer than 1.5 times the median interval.
    pub long_frames: usize,
    pub update_avg_ms: f32,
    pub update_worst_ms: f32,
    pub render_avg_ms: f32,
    pub render_worst_ms: f32,
    /// Frames that ran 0, 1, 2 and 3-or-more simulation ticks.
    pub tick_hist: [usize; 4],
    /// Frame intervals per [`BUCKETS_MS`] bucket.
    pub hist: [usize; BUCKETS_MS.len() + 1],
    pub draws: u32,
    pub state_changes: u32,
    pub upload_bytes: u32,
    pub wakeups_avg: f32,
}

pub struct PerfStats {
    ring: Box<[FrameSample; WINDOW]>,
    next: usize,
    len: usize,
    /// All frames since the last reset (the ring only holds the newest).
    pub total_frames: u64,
}

impl Default for PerfStats {
    fn default() -> Self {
        Self::new()
    }
}

impl PerfStats {
    pub fn new() -> Self {
        Self {
            ring: Box::new([FrameSample::default(); WINDOW]),
            next: 0,
            len: 0,
            total_frames: 0,
        }
    }

    pub fn reset(&mut self) {
        self.next = 0;
        self.len = 0;
        self.total_frames = 0;
    }

    pub fn record(&mut self, s: FrameSample) {
        self.ring[self.next] = s;
        self.next = (self.next + 1) % WINDOW;
        self.len = (self.len + 1).min(WINDOW);
        self.total_frames += 1;
    }

    /// Samples in the window, oldest first.
    pub fn samples(&self) -> impl Iterator<Item = &FrameSample> {
        let start = (self.next + WINDOW - self.len) % WINDOW;
        (0..self.len).map(move |i| &self.ring[(start + i) % WINDOW])
    }

    pub fn summary(&self) -> Summary {
        let n = self.len;
        if n == 0 {
            return Summary::default();
        }
        let mut intervals: Vec<f32> = self.samples().map(|s| s.interval_ms).collect();
        intervals.sort_by(f32::total_cmp);
        let p50 = intervals[n / 2];
        let mut out = Summary {
            frames: n,
            p50_ms: p50,
            p99_ms: intervals[((n * 99) / 100).min(n - 1)],
            worst_ms: intervals[n - 1],
            ..Summary::default()
        };
        let (mut sum, mut update, mut render, mut wakeups) = (0.0f64, 0.0f64, 0.0f64, 0u64);
        for s in self.samples() {
            sum += s.interval_ms as f64;
            update += s.update_ms as f64;
            render += s.render_ms as f64;
            wakeups += s.wakeups as u64;
            out.update_worst_ms = out.update_worst_ms.max(s.update_ms);
            out.render_worst_ms = out.render_worst_ms.max(s.render_ms);
            out.long_frames += (s.interval_ms > p50 * 1.5) as usize;
            out.tick_hist[(s.ticks as usize).min(3)] += 1;
            let bucket = BUCKETS_MS
                .iter()
                .position(|edge| s.interval_ms <= *edge)
                .unwrap_or(BUCKETS_MS.len());
            out.hist[bucket] += 1;
            out.draws = s.draws;
            out.state_changes = s.state_changes;
            out.upload_bytes = s.upload_bytes;
        }
        out.avg_ms = (sum / n as f64) as f32;
        out.update_avg_ms = (update / n as f64) as f32;
        out.render_avg_ms = (render / n as f64) as f32;
        out.wakeups_avg = wakeups as f32 / n as f32;
        out
    }
}

impl Summary {
    /// The summary as a JSON object (numbers only, so no escaping is needed).
    pub fn to_json(&self) -> String {
        let list = |v: &[usize]| v.iter().map(usize::to_string).collect::<Vec<_>>().join(",");
        format!(
            "{{\"frames\":{},\"avg\":{:.3},\"p50\":{:.3},\"p99\":{:.3},\"worst\":{:.3},\"long\":{},\
             \"updateAvg\":{:.3},\"updateWorst\":{:.3},\"renderAvg\":{:.3},\"renderWorst\":{:.3},\
             \"ticks\":[{}],\"hist\":[{}],\"draws\":{},\"stateChanges\":{},\"uploadBytes\":{},\"wakeups\":{:.2}}}",
            self.frames,
            self.avg_ms,
            self.p50_ms,
            self.p99_ms,
            self.worst_ms,
            self.long_frames,
            self.update_avg_ms,
            self.update_worst_ms,
            self.render_avg_ms,
            self.render_worst_ms,
            list(&self.tick_hist),
            list(&self.hist),
            self.draws,
            self.state_changes,
            self.upload_bytes,
            self.wakeups_avg,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(interval_ms: f32, ticks: u32) -> FrameSample {
        FrameSample {
            interval_ms,
            update_ms: 1.0,
            render_ms: 2.0,
            ticks,
            ..FrameSample::default()
        }
    }

    #[test]
    fn summary_reports_percentiles_and_long_frames() {
        let mut p = PerfStats::new();
        assert_eq!(p.summary(), Summary::default());
        for i in 0..200 {
            p.record(sample(if i == 50 { 40.0 } else { 16.7 }, 1));
        }
        let s = p.summary();
        assert_eq!(s.frames, 200);
        assert_eq!(s.worst_ms, 40.0);
        assert_eq!(s.p50_ms, 16.7);
        assert_eq!(s.long_frames, 1);
        assert_eq!(s.tick_hist, [0, 200, 0, 0]);
        assert_eq!(s.hist[3], 199);
        assert_eq!(s.hist[5], 1);
        assert!((s.update_avg_ms - 1.0).abs() < 1e-6 && s.render_worst_ms == 2.0);
        assert!(s.to_json().contains("\"worst\":40.000"));
    }

    #[test]
    fn ring_keeps_the_newest_window() {
        let mut p = PerfStats::new();
        for i in 0..WINDOW + 10 {
            p.record(sample(i as f32, 0));
        }
        assert_eq!(p.total_frames, WINDOW as u64 + 10);
        assert_eq!(p.samples().count(), WINDOW);
        assert_eq!(p.samples().next().unwrap().interval_ms, 10.0);
        p.reset();
        assert_eq!(p.samples().count(), 0);
    }
}
