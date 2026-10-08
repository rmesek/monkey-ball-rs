//! A simple closed-loop player for automated runs (`--autoplay`, integration tests): it steers the ball
//! through the bananas of the main anim group and then through the goal by looking at the simulation state.
//! It grew out of the waypoint controller in `mb-engine`'s parity tests (same velocity-error steering in
//! the camera frame) and adds a line-up step in front of the goal. It only produces stick values, which
//! enter the game through `InputState` like any other device, and it is only meant for open, flat stages.

use mb_engine::math::Vec3;
use mb_engine::{QuantizedInput, Sim};
use mb_stage::Stage;

/// Distance in front of the goal where the ball lines up, and how far behind it the final target lies.
const GOAL_APPROACH: f64 = 2.5;
const GOAL_OVERSHOOT: f64 = 3.0;
/// Horizontal distance at which a banana counts as reached (ball and banana spheres overlap well before).
const BANANA_REACHED: f64 = 0.7;

#[derive(Clone, Copy, Debug)]
struct GoalLine {
    /// Goal centre in XZ.
    center: (f64, f64),
    /// Unit normal of the goal plane in XZ, pointing at the side the ball comes from.
    normal: (f64, f64),
}

pub struct Autopilot {
    /// Banana positions in visiting order.
    pub targets: Vec<Vec3>,
    pub next: usize,
    goal: Option<GoalLine>,
    lined_up: bool,
    /// Cruise speed in units per tick.
    pub speed: f64,
    pub gain: f64,
}

impl Autopilot {
    /// Bananas of anim group 0 (nearest first from the start), then the first goal. `None` for stages
    /// without a start position.
    pub fn for_stage(stage: &Stage) -> Option<Autopilot> {
        let g = stage.anim_groups.first()?;
        let start = stage.start_positions.first()?.pos;
        let mut cur = (start.x as f64, start.z as f64);
        let mut left: Vec<Vec3> = g
            .bananas
            .iter()
            .map(|b| Vec3::new(b.pos.x as f64, b.pos.y as f64, b.pos.z as f64))
            .collect();
        let mut targets = Vec::new();
        while !left.is_empty() {
            let d = |p: &Vec3| ((p.x - cur.0).powi(2) + (p.z - cur.1).powi(2)).sqrt();
            let (i, _) = left
                .iter()
                .enumerate()
                .min_by(|a, b| d(a.1).total_cmp(&d(b.1)))?;
            let p = left.remove(i);
            cur = (p.x, p.z);
            targets.push(p);
        }
        let goal = g.goals.first().map(|goal| {
            let center = (goal.pos.x as f64, goal.pos.z as f64);
            let yaw = goal.rot.y as f64 * std::f64::consts::PI / 32768.0;
            let n = (yaw.sin(), yaw.cos());
            // Approach from the side the start is on.
            let from = (start.x as f64 - center.0, start.z as f64 - center.1);
            let side = if from.0 * n.0 + from.1 * n.1 >= 0.0 {
                1.0
            } else {
                -1.0
            };
            GoalLine {
                center,
                normal: (n.0 * side, n.1 * side),
            }
        });
        Some(Autopilot {
            targets,
            next: 0,
            goal,
            lined_up: false,
            speed: 0.12,
            gain: 20.0,
        })
    }

    /// Skip the bananas: the fastest way to finish a stage.
    pub fn goal_only(stage: &Stage) -> Option<Autopilot> {
        let mut a = Self::for_stage(stage)?;
        a.targets.clear();
        Some(a)
    }

    /// Where to head now and how fast, from the ball's XZ position.
    fn aim(&mut self, pos: (f64, f64)) -> Option<((f64, f64), f64)> {
        while let Some(t) = self.targets.get(self.next) {
            let dist = ((t.x - pos.0).powi(2) + (t.z - pos.1).powi(2)).sqrt();
            if dist >= BANANA_REACHED {
                // Slow down on arrival so the next turn is not a wide swing.
                return Some(((t.x, t.z), self.speed.min(0.03 + dist * 0.06)));
            }
            self.next += 1;
        }
        let goal = self.goal?;
        let (c, n) = (goal.center, goal.normal);
        let rel = (pos.0 - c.0, pos.1 - c.1);
        // Distance in front of the goal plane and sideways offset from the goal axis.
        let ahead = rel.0 * n.0 + rel.1 * n.1;
        let lateral = (rel.0 - ahead * n.0, rel.1 - ahead * n.1);
        let off_axis = (lateral.0 * lateral.0 + lateral.1 * lateral.1).sqrt();
        if ahead < 0.3 && off_axis > 0.6 {
            // Went past the posts on the outside: come around again.
            self.lined_up = false;
        }
        if !self.lined_up {
            let approach = (c.0 + n.0 * GOAL_APPROACH, c.1 + n.1 * GOAL_APPROACH);
            let dist = ((approach.0 - pos.0).powi(2) + (approach.1 - pos.1).powi(2)).sqrt();
            if dist > 0.6 || off_axis > 0.35 {
                return Some((approach, self.speed.min(0.02 + dist * 0.06)));
            }
            self.lined_up = true;
        }
        // Chase a point on the goal axis two units ahead of the ball, so sideways drift is corrected
        // while it rolls through.
        let along = (ahead - 2.0).max(-GOAL_OVERSHOOT);
        Some(((c.0 + n.0 * along, c.1 + n.1 * along), self.speed))
    }

    /// Stick for this tick (engine convention: `y` negative = forward).
    pub fn input(&mut self, sim: &Sim) -> QuantizedInput {
        let b = &sim.ball;
        let Some((target, speed)) = self.aim((b.pos.x, b.pos.z)) else {
            return QuantizedInput::default();
        };
        let (dx, dz) = (target.0 - b.pos.x, target.1 - b.pos.z);
        let dist = (dx * dx + dz * dz).sqrt().max(1e-6);
        let want = (dx / dist * speed, dz / dist * speed);
        let err = (want.0 - b.vel.x, want.1 - b.vel.z);
        // The stick is relative to the camera: express the wanted acceleration in its frame.
        let theta = sim.camera.rot_y as f64 * std::f64::consts::PI / 32768.0;
        let forward = (-theta.sin(), -theta.cos());
        let right = (theta.cos(), -theta.sin());
        let af = err.0 * forward.0 + err.1 * forward.1;
        let ar = err.0 * right.0 + err.1 * right.1;
        let q = |v: f64| (v * self.gain * 127.0).round().clamp(-127.0, 127.0) as i32;
        QuantizedInput {
            x: q(ar),
            y: q(-af),
        }
    }

    /// The same stick as normalized `InputState` axes: `(MoveX, MoveY)` with +Y = forward.
    pub fn axes(&mut self, sim: &Sim) -> (f32, f32) {
        let q = self.input(sim);
        (q.x as f32 / 127.0, -q.y as f32 / 127.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pilot() -> Autopilot {
        Autopilot {
            targets: vec![Vec3::new(2.0, 0.0, 0.0)],
            next: 0,
            goal: Some(GoalLine {
                center: (10.0, 0.0),
                normal: (-1.0, 0.0),
            }),
            lined_up: false,
            speed: 0.12,
            gain: 20.0,
        }
    }

    #[test]
    fn visits_bananas_then_lines_up_then_runs_through() {
        let mut p = pilot();
        let (t, _) = p.aim((0.0, 0.0)).unwrap();
        assert_eq!(t, (2.0, 0.0));
        // At the banana: on to the approach point in front of the goal.
        let (t, _) = p.aim((1.8, 0.1)).unwrap();
        assert_eq!(p.next, 1);
        assert!((t.0 - 7.5).abs() < 1e-9 && t.1.abs() < 1e-9);
        // Off the axis near the approach point: not lined up yet.
        p.aim((7.4, 0.5)).unwrap();
        assert!(!p.lined_up);
        let (t, speed) = p.aim((7.5, 0.1)).unwrap();
        assert!(p.lined_up && speed == p.speed);
        // The carrot sits on the axis two units ahead and ends beyond the goal.
        assert!((t.0 - 9.5).abs() < 1e-9 && t.1.abs() < 1e-9);
        let (t, _) = p.aim((11.5, 0.0)).unwrap();
        assert!((t.0 - 13.0).abs() < 1e-9);
    }

    #[test]
    fn missing_the_goal_mouth_goes_around_again() {
        let mut p = pilot();
        p.next = 1;
        p.lined_up = true;
        let (t, _) = p.aim((10.5, 2.0)).unwrap();
        assert!(!p.lined_up);
        assert!((t.0 - 7.5).abs() < 1e-9);
    }
}
