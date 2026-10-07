//! S16 trig lookup tables (sine and arctangent slope/intercept pairs).
//!
//! The game's tables are data baked into its executable and are *not* the correctly-rounded values of
//! `sin`/`atan`: roughly a quarter of the sine entries differ from `fround(sin(x))` by one f32 ulp. Bit-exact
//! parity with the reference therefore needs the real tables, which are loaded at runtime from the user's own
//! `main.dol` ([`Tables::from_dol`]). Nothing from the game is stored in this repository. When no real table is
//! installed an [`approximate`](Tables::approximate) pair is computed instead (within an ulp or so for sine and
//! within one S16 unit for atan; fine for tests and previews, not for replay parity).

use std::sync::OnceLock;

/// Number of sine entries: one quadrant (0x4000 steps) plus the endpoint.
pub const SIN_LEN: usize = 0x4001;
/// Number of atan floats: 4096 `(slope, intercept)` pairs indexed by `floor(x * 4096)`.
pub const ATAN_LEN: usize = 0x2000;

/// The two lookup tables used by `sin_s16` / `atan2_s16`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tables {
    pub sin: Vec<f32>,
    pub atan: Vec<f32>,
    /// True when loaded from the game's own data, false for the computed approximation.
    pub exact: bool,
}

fn be_f32(b: &[u8], i: usize) -> f32 {
    f32::from_bits(u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]))
}

fn find_table(dol: &[u8], sig: &[f32], len: usize) -> Option<Vec<f32>> {
    let sig_bytes: Vec<u8> = sig.iter().flat_map(|f| f.to_bits().to_be_bytes()).collect();
    let mut start = 0;
    while start + len * 4 <= dol.len() {
        let rel = dol[start..]
            .windows(sig_bytes.len())
            .position(|w| w == sig_bytes)?;
        let at = start + rel;
        if at + len * 4 <= dol.len() {
            return Some((0..len).map(|i| be_f32(dol, at + i * 4)).collect());
        }
        start = at + 1;
    }
    None
}

impl Tables {
    /// Locate both tables inside a GameCube `main.dol` (or any blob containing the game's `.rodata`) by
    /// scanning for the first entries of each table. Returns `None` when either is missing or implausible.
    pub fn from_dol(dol: &[u8]) -> Option<Tables> {
        let sin = find_table(
            dol,
            &[
                0.0,
                9.58738019107841e-05_f64 as f32,
                0.000191747603821568_f64 as f32,
            ],
            SIN_LEN,
        )?;
        let atan = find_table(
            dol,
            &[
                10430.3779296875_f64 as f32,
                0.0,
                10430.376953125_f64 as f32,
                3.0356392244357266e-7_f64 as f32,
            ],
            ATAN_LEN,
        )?;
        if sin[SIN_LEN - 1] != 1.0 || !(sin[0x2000] > 0.707 && sin[0x2000] < 0.7072) {
            return None;
        }
        Some(Tables {
            sin,
            atan,
            exact: true,
        })
    }

    /// Computed stand-in: `fround(sin)` and secant fits of `atan` per 1/4096 interval.
    pub fn approximate() -> Tables {
        let k = std::f64::consts::PI / 32768.0;
        let sin = (0..SIN_LEN).map(|i| (i as f64 * k).sin() as f32).collect();
        let s16 = 32768.0 / std::f64::consts::PI;
        let n = 4096.0;
        let mut atan = Vec::with_capacity(ATAN_LEN);
        for i in 0..4096 {
            let x0 = i as f64 / n;
            let x1 = (i + 1) as f64 / n;
            let (a0, a1) = (x0.atan() * s16, x1.atan() * s16);
            let slope = (a1 - a0) * n;
            let intercept = a0 - slope * x0;
            atan.push(slope as f32);
            atan.push(intercept as f32);
        }
        Tables {
            sin,
            atan,
            exact: false,
        }
    }
}

static TABLES: OnceLock<Tables> = OnceLock::new();

/// Install the process-wide tables. Must be called before the first simulation call; returns the tables back
/// if something (including an earlier lazy fallback) is already installed.
pub fn install_tables(t: Tables) -> Result<(), Tables> {
    TABLES.set(t)
}

/// The installed tables, installing the [`approximate`](Tables::approximate) fallback on first use.
pub fn tables() -> &'static Tables {
    TABLES.get_or_init(Tables::approximate)
}
