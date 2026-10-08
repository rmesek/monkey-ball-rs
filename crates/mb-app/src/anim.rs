//! Keyframe interpolation and anim-group transforms for the viewer, ported from
//! `noclip/SuperMonkeyBall/Anim.ts` and `AnimGroup.update`. The simulation core will own the real animation
//! state later; this keeps stages visibly animated and exercises `Renderer::set_transform`.

use glam::{Mat4, Vec3};
use mb_stage::{AnimGroup, Keyframe};

use crate::stage_scene::S16_TO_RADIANS;

const DEG_TO_RAD: f32 = std::f32::consts::PI / 180.0;

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value of a keyframe track at `time` (seconds). Empty tracks return `None`.
pub fn interpolate(time: f32, keys: &[Keyframe]) -> Option<f32> {
    let first = keys.first()?;
    let last = keys.last()?;
    if time <= first.time_seconds {
        return Some(first.value);
    }
    if time >= last.time_seconds {
        return Some(last.value);
    }
    // First key strictly after `time`.
    let next_idx = keys.partition_point(|k| k.time_seconds <= time);
    let prev = &keys[next_idx - 1];
    let next = &keys[next_idx];
    if prev.ease_type == 0 {
        return Some(prev.value);
    }
    let t = (time - prev.time_seconds) / (next.time_seconds - prev.time_seconds);
    if prev.ease_type == 1 {
        return Some(prev.value + (next.value - prev.value) * t);
    }
    // Any other ease type is a hermite-style smoothstep with tangents.
    let dt = next.time_seconds - prev.time_seconds;
    let base = prev.value + (next.value - prev.value) * smoothstep(t);
    let (t2, t3) = (t * t, t * t * t);
    let in_adjust = next.tangent_in * (t3 - t2);
    let out_adjust = prev.tangent_out * (t + (t3 - 2.0 * t2));
    Some(base + dt * (in_adjust + out_adjust))
}

/// The reference's loop wrap (it adds the loop start before the modulo).
pub fn loop_wrap(time: f32, start: f32, end: f32) -> f32 {
    let duration = end - start;
    if duration <= 0.0 {
        return time;
    }
    ((time + start) % duration) + start
}

/// World-from-anim-group matrix at `time`. Group 0 is the static world (identity); other groups rotate and
/// translate around their origin, defaulting to the origin pose for tracks without keyframes.
pub fn world_from_group(
    group: &AnimGroup,
    index: usize,
    time: f32,
    loop_range: (i32, i32),
) -> Mat4 {
    if index == 0 {
        return Mat4::IDENTITY;
    }
    let origin = Vec3::new(group.init_pos.x, group.init_pos.y, group.init_pos.z);
    let origin_rot = group.init_rot;
    let t = loop_wrap(time, loop_range.0 as f32, loop_range.1 as f32);

    let mut pos = origin;
    let mut rot = Vec3::new(
        origin_rot.x as f32 * S16_TO_RADIANS,
        origin_rot.y as f32 * S16_TO_RADIANS,
        origin_rot.z as f32 * S16_TO_RADIANS,
    );
    if let Some(a) = &group.anim {
        if let Some(v) = interpolate(t, &a.pos_x) {
            pos.x = v;
        }
        if let Some(v) = interpolate(t, &a.pos_y) {
            pos.y = v;
        }
        if let Some(v) = interpolate(t, &a.pos_z) {
            pos.z = v;
        }
        if let Some(v) = interpolate(t, &a.rot_x) {
            rot.x = v * DEG_TO_RAD;
        }
        if let Some(v) = interpolate(t, &a.rot_y) {
            rot.y = v * DEG_TO_RAD;
        }
        if let Some(v) = interpolate(t, &a.rot_z) {
            rot.z = v * DEG_TO_RAD;
        }
    }
    let origin_from_group = Mat4::from_rotation_x(-(origin_rot.x as f32) * S16_TO_RADIANS)
        * Mat4::from_rotation_y(-(origin_rot.y as f32) * S16_TO_RADIANS)
        * Mat4::from_rotation_z(-(origin_rot.z as f32) * S16_TO_RADIANS)
        * Mat4::from_translation(-origin);
    Mat4::from_translation(pos)
        * Mat4::from_rotation_z(rot.z)
        * Mat4::from_rotation_y(rot.y)
        * Mat4::from_rotation_x(rot.x)
        * origin_from_group
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(ease: i32, t: f32, v: f32) -> Keyframe {
        Keyframe {
            ease_type: ease,
            time_seconds: t,
            value: v,
            tangent_in: 0.0,
            tangent_out: 0.0,
        }
    }

    #[test]
    fn empty_track_is_none() {
        assert_eq!(interpolate(1.0, &[]), None);
    }

    #[test]
    fn clamps_outside_the_range() {
        let k = [key(1, 1.0, 10.0), key(1, 3.0, 20.0)];
        assert_eq!(interpolate(0.0, &k), Some(10.0));
        assert_eq!(interpolate(9.0, &k), Some(20.0));
    }

    #[test]
    fn linear_constant_and_smooth() {
        let lin = [key(1, 0.0, 0.0), key(1, 2.0, 10.0)];
        assert_eq!(interpolate(1.0, &lin), Some(5.0));
        let constant = [key(0, 0.0, 3.0), key(0, 2.0, 10.0)];
        assert_eq!(interpolate(1.0, &constant), Some(3.0));
        let smooth = [key(2, 0.0, 0.0), key(2, 2.0, 10.0)];
        // Zero tangents: plain smoothstep, 0.5 at the midpoint and slower near the ends.
        assert!((interpolate(1.0, &smooth).unwrap() - 5.0).abs() < 1e-5);
        assert!(interpolate(0.5, &smooth).unwrap() < 2.5);
    }

    #[test]
    fn loop_wrap_matches_reference_formula() {
        assert_eq!(loop_wrap(7.0, 0.0, 5.0), 2.0);
        assert_eq!(loop_wrap(1.0, 0.0, 0.0), 1.0);
    }
}
