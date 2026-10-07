//! Port of `src/animation.ts`: keyframe interpolation.

use mb_stage::Keyframe;

pub fn smoothstep(t: f64) -> f64 {
    (3.0 - 2.0 * t) * t * t
}

/// `interpolateKeyframes(count, frames, timeSeconds)`. Keyframe fields are f32 in the file, promoted to f64.
pub fn interpolate_keyframes(frames: &[Keyframe], time_seconds: f64) -> f64 {
    let count = frames.len();
    if count == 0 {
        return 0.0;
    }
    if count < 2 || time_seconds <= frames[0].time_seconds as f64 {
        return frames[0].value as f64;
    }
    if time_seconds >= frames[count - 1].time_seconds as f64 {
        return frames[count - 1].value as f64;
    }
    let mut next_index = 1;
    while next_index < count - 1 && frames[next_index].time_seconds as f64 <= time_seconds {
        next_index += 1;
    }
    let curr = &frames[next_index - 1];
    let next = &frames[next_index];
    if curr.ease_type == 0 {
        return curr.value as f64;
    }
    let curr_time = curr.time_seconds as f64;
    let dt = next.time_seconds as f64 - curr_time;
    let t = (time_seconds - curr_time) / dt;
    if curr.ease_type == 1 {
        return curr.value as f64 * (1.0 - t) + next.value as f64 * t;
    }
    let t2 = t * t;
    let t3 = t2 * t;
    curr.value as f64 * (1.0 + (2.0 * t3 - 3.0 * t2))
        + next.value as f64 * (-2.0 * t3 + 3.0 * t2)
        + dt * ((curr.tangent_out as f64 * (t + (t3 - 2.0 * t2)))
            + next.tangent_in as f64 * (t3 - t2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kf(ease: i32, t: f32, v: f32) -> Keyframe {
        Keyframe {
            ease_type: ease,
            time_seconds: t,
            value: v,
            tangent_in: 0.0,
            tangent_out: 0.0,
        }
    }

    #[test]
    fn clamps_and_interpolates() {
        let f = [kf(1, 0.0, 0.0), kf(1, 2.0, 10.0), kf(1, 4.0, 10.0)];
        assert_eq!(interpolate_keyframes(&f, -1.0), 0.0);
        assert_eq!(interpolate_keyframes(&f, 1.0), 5.0);
        assert_eq!(interpolate_keyframes(&f, 9.0), 10.0);
        let c = [kf(0, 0.0, 3.0), kf(0, 1.0, 7.0)];
        assert_eq!(interpolate_keyframes(&c, 0.5), 3.0);
        assert_eq!(interpolate_keyframes(&[], 0.5), 0.0);
    }

    #[test]
    fn smoothstep_endpoints() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(0.5), 0.5);
    }
}
