//! The waypoint controller used to script goal runs for the parity test must itself reach the goal on a few
//! early stages (skipped without a disc image).

mod common;

use mb_engine::{Outcome, Phase, QuantizedInput, Sim, SimConfig};

#[test]
fn autopilot_reaches_the_goal_and_collects_bananas() {
    let Some(disc) = common::open_disc() else {
        eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
        return;
    };
    if !common::install_real_tables() {
        eprintln!("skipped: could not read the trig tables from main.dol");
        return;
    }
    for (id, min_bananas) in [(1u32, 4), (11, 4), (17, 10)] {
        let stage = common::load_stage(&disc, id);
        let mut ap = common::Autopilot::for_stage(&stage);
        let mut sim = Sim::new(&stage, SimConfig::first_attempt(id));
        let total = sim.bananas_left;
        for _ in 0..4000 {
            let input = if sim.intro_timer <= 0 {
                ap.input(&sim)
            } else {
                QuantizedInput::default()
            };
            sim.step(input);
            if sim.is_finished() {
                break;
            }
        }
        assert!(
            matches!(sim.phase, Phase::Finished(Outcome::Goal(_))),
            "stage {id}: {:?}",
            sim.phase
        );
        assert!(total - sim.bananas_left >= min_bananas, "stage {id}");
        assert!(sim.score > 0);
    }
}
