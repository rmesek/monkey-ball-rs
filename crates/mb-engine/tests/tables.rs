//! The game's S16 trig tables, read from the user's own `main.dol`. Skipped without a disc image.

mod common;

use mb_engine::math::{atan2_s16, cos_s16, sin_s16};
use mb_engine::tables::{ATAN_LEN, SIN_LEN, Tables};

#[test]
fn real_tables_load_and_look_like_trig() {
    let Some(dol) = common::read_main_dol() else {
        eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
        return;
    };
    let t = Tables::from_dol(&dol).expect("trig tables not found in main.dol");
    assert_eq!(t.sin.len(), SIN_LEN);
    assert_eq!(t.atan.len(), ATAN_LEN);
    assert!(t.exact);
    // Near the correctly-rounded values (a quarter of the entries differ by one f32 ulp).
    let approx = Tables::approximate();
    let mut ulp_diff = 0;
    for (a, b) in t.sin.iter().zip(&approx.sin) {
        let d = (a.to_bits() as i64 - b.to_bits() as i64).abs();
        assert!(d <= 2, "sin entry off by {d} ulp");
        if d != 0 {
            ulp_diff += 1;
        }
    }
    eprintln!("{ulp_diff} of {SIN_LEN} sine entries differ from fround(sin)");
    for (i, (a, b)) in t.atan.iter().zip(&approx.atan).enumerate() {
        let tol = 0.5;
        assert!(
            (a - b).abs() < tol,
            "atan entry {i}: {a} vs {b}, differs from a secant fit by more than {tol}"
        );
    }
}

#[test]
fn trig_with_real_tables() {
    if !common::install_real_tables() {
        eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
        return;
    }
    assert_eq!(sin_s16(0), 0.0);
    assert_eq!(sin_s16(0x4000), 1.0);
    assert_eq!(cos_s16(0), 1.0);
    assert_eq!(sin_s16(0x8000), -0.0);
    assert_eq!(sin_s16(-0x4000), -1.0);
    assert_eq!(atan2_s16(1.0, 1.0), 0x2000);
    assert_eq!(atan2_s16(1.0, 0.0), 0x4000);
    assert_eq!(atan2_s16(-1.0, -1.0), -0x6000);
    assert_eq!(atan2_s16(0.0, -1.0), -0x8000);
}
