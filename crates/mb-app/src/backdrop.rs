//! What a background does besides standing there: ports of `BgNight`, `BgStorm` and `BgSunset`
//! (`noclip/SuperMonkeyBall/Background.ts`), and the name of the bonus wave (`BonusWave.ts`).
//!
//! * **Night**: the dancers in the diner windows are flipbooks. A window names one of seven lists of frame
//!   models (`NIG_MADO_A_PT00` ...); the frame shown is `floor(time / 2) % frames`.
//! * **Storm**: the fires are flipbooks of 32 frame models (`STM_FIRE00` ...), each fire a few frames ahead
//!   of the next, turned about Y to face the camera.
//! * **Sunset**: the textures of `SUN_GROUND` and `SUN_CLOUD_*` drift, each at its own speed and heading,
//!   and change both when the clock has eleven seconds left.
//!
//! Every frame of a flipbook is an object created at load ([`crate::stage_scene::StageScene::build`]);
//! showing another frame switches two visibility flags and creates nothing. Nothing here allocates after
//! the stage is built. What differs from the reference is listed in `docs/GAMEPLAY.md`.

use glam::{Mat4, Vec2, Vec3};
use mb_engine::rng::DeterministicRng;
use mb_render::{Camera, ObjectId, Renderer};

/// `BONUS_WAVE_MODEL_NAME`: the rippling floor of stage 92, a model of the stage's NaomiLib archive that
/// no anim group names.
pub const BONUS_WAVE_MODEL_NAME: &str = "SHAPE_STAGE134";
/// `StageId.St092_Bonus_Wave`, also `BONUS_WAVE_STAGE_ID` of the collision code.
pub const BONUS_WAVE_STAGE_ID: u32 = 92;

/// `NIGHT_WINDOW_MODEL_LISTS`: letter and frame count of each list. The archive holds 22 frames for `E`;
/// the reference's list stops at 18, and so does this.
pub const NIGHT_WINDOW_LISTS: [(char, usize); 7] = [
    ('A', 14),
    ('B', 11),
    ('C', 18),
    ('D', 18),
    ('E', 18),
    ('F', 16),
    ('G', 15),
];
/// `STORM_FIRE_MODELS.length`.
pub const STORM_FIRE_FRAMES: usize = 32;

/// The list a window animates: `NIGHT_WINDOW_MODEL_LISTS[id - 65] ?? NIGHT_WINDOW_A_MODELS` (the id is the
/// letter's character code).
pub fn night_window_list(id: u8) -> (char, usize) {
    usize::from(id)
        .checked_sub(65)
        .and_then(|i| NIGHT_WINDOW_LISTS.get(i))
        .copied()
        .unwrap_or(NIGHT_WINDOW_LISTS[0])
}

/// Name of frame `frame` of a window list (`BgNightModelID`).
pub fn night_window_model(letter: char, frame: usize) -> String {
    format!("NIG_MADO_{letter}_PT{frame:02}")
}

/// Name of frame `frame` of the fire (`BgStormModelID`).
pub fn storm_fire_model(frame: usize) -> String {
    format!("STM_FIRE{frame:02}")
}

/// Whole frames of `anim_frames`, never negative (the stage timer starts at zero).
fn whole_frames(anim_frames: f32) -> usize {
    if anim_frames.is_finite() && anim_frames > 0.0 {
        anim_frames.floor() as usize
    } else {
        0
    }
}

/// `Math.floor(getAnimTimeFrames() / 2) % modelList.length`.
pub fn night_window_frame(anim_frames: f32, frames: usize) -> usize {
    (whole_frames(anim_frames) / 2) % frames.max(1)
}

/// `(Math.floor(getAnimTimeFrames()) + 4 * frameOffset) % STORM_FIRE_MODELS.length`.
pub fn storm_fire_frame(anim_frames: f32, frame_offset: u8) -> usize {
    (whole_frames(anim_frames) + 4 * usize::from(frame_offset)) % STORM_FIRE_FRAMES
}

/// `getMat4RotY(camera.worldMatrix)`: the camera's turn about Y, from where its right-hand side points.
pub fn camera_rot_y(camera: &Camera) -> f32 {
    let right = camera.view.inverse().x_axis.truncate();
    let flat = Vec2::new(right.x, right.z);
    if flat.length_squared() < 1e-12 || !flat.is_finite() {
        return 0.0;
    }
    (-flat.y).atan2(flat.x)
}

/// What a flipbook is, beyond its frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlipbookKind {
    /// Placed and turned once, at load.
    NightWindow,
    /// Stands at `pos` and turns to the camera every frame.
    StormFire { pos: Vec3, frame_offset: u8 },
}

/// One window or fire: an object per frame, of which at most one is visible.
#[derive(Clone, Debug)]
pub struct Flipbook {
    pub kind: FlipbookKind,
    /// `None`: the archive lacks that frame, and nothing is drawn while it is due.
    pub frames: Vec<Option<ObjectId>>,
    shown: Option<usize>,
}

impl Flipbook {
    /// `frames` are hidden objects in frame order.
    pub fn new(kind: FlipbookKind, frames: Vec<Option<ObjectId>>) -> Self {
        Self {
            kind,
            frames,
            shown: None,
        }
    }

    /// The frame due at `anim_frames` (the stage timer, blended between ticks).
    pub fn frame_at(&self, anim_frames: f32) -> usize {
        match self.kind {
            FlipbookKind::NightWindow => night_window_frame(anim_frames, self.frames.len()),
            FlipbookKind::StormFire { frame_offset, .. } => {
                storm_fire_frame(anim_frames, frame_offset) % self.frames.len().max(1)
            }
        }
    }

    /// The frame on screen, once one has been shown.
    pub fn shown(&self) -> Option<usize> {
        self.shown
    }

    /// Show the frame due at `anim_frames` and hide the one that was showing.
    pub fn show(&mut self, r: &mut Renderer, anim_frames: f32) {
        let frame = self.frame_at(anim_frames);
        if self.shown == Some(frame) {
            return;
        }
        if let Some(old) = self
            .shown
            .and_then(|i| self.frames.get(i).copied().flatten())
        {
            r.set_visible(old, false);
        }
        if let Some(new) = self.frames.get(frame).copied().flatten() {
            r.set_visible(new, true);
        }
        self.shown = Some(frame);
    }

    /// Turn the frame on screen to the camera (fires only): `T(pos) * Ry(camera)`.
    pub fn face(&self, r: &mut Renderer, rot_y: f32) {
        let FlipbookKind::StormFire { pos, .. } = self.kind else {
            return;
        };
        if let Some(object) = self
            .shown
            .and_then(|i| self.frames.get(i).copied().flatten())
        {
            r.set_transform(
                object,
                Mat4::from_translation(pos) * Mat4::from_rotation_y(rot_y),
            );
        }
    }
}

/// Texture units a cloud layer drifts per frame before the hurry-up (`0.0015151514671742916`, one
/// repeat in eleven seconds) and after it (`0.0030303029343485832`).
const CLOUD_SPEED: f32 = 0.001_515_151_5;
const CLOUD_SPEED_HURRY: f32 = 0.003_030_303;
/// The share of the difference to the wanted velocity that is left after one frame.
const CLOUD_VELOCITY_KEEP: f32 = 0.95;
/// Frames left on the clock below which the clouds speed up (the HUD's `HURRY UP!`).
pub const CLOUD_HURRY_FRAMES: i32 = 660;
/// `SPIN_IN_FRAMES`: the stage timer is this far ahead of the clock (`MkbTime`).
const SPIN_IN_FRAMES: i32 = 120;
/// What the stage id is mixed with to seed the clouds ("SUNS").
const CLOUD_SEED: u32 = 0x5355_4e53;

/// `MkbTime.getStageTimeFrames`: frames left on the clock as the reference's scene sees them, from the
/// stage timer. `None` for a stage without a time limit. (The reference assumes a limit of 60 seconds for
/// every stage, with a note to fix that; this takes the stage's own.)
pub fn stage_time_left(time_limit_frames: i32, timer_frames: i32) -> Option<i32> {
    (time_limit_frames > 0)
        .then(|| (time_limit_frames - timer_frames + SPIN_IN_FRAMES).clamp(0, time_limit_frames))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Cloud {
    /// `currTexTranslate` at the last two ticks (`[previous, current]`).
    offset: [Vec2; 2],
    /// `currTexVel`, `desiredTexVel`.
    vel: Vec2,
    desired: Vec2,
}

/// The drifting cloud layers of the sunset background (`BgSunset`).
///
/// The reference draws its offsets and headings from `Math.random`. Here they come from a generator of
/// their own, seeded from the stage id, so a stage looks the same every time it is loaded and the
/// simulation's generators are never touched.
#[derive(Clone, Debug)]
pub struct Clouds {
    /// The object of each cloud, in stagedef order.
    objects: Vec<ObjectId>,
    clouds: Vec<Cloud>,
    seed: u32,
    rng: DeterministicRng,
    hurry: bool,
}

/// `(0, speed, 0)` turned about Z by `angle` (`vec3.rotateZ`).
fn heading(speed: f32, angle: f32) -> Vec2 {
    Vec2::new(-speed * angle.sin(), speed * angle.cos())
}

impl Clouds {
    /// `objects` are the cloud objects in stagedef order.
    pub fn new(objects: Vec<ObjectId>, stage_id: u32) -> Self {
        let mut clouds = Self::drifting(objects.len(), stage_id);
        clouds.objects = objects;
        clouds
    }

    /// The state of `count` clouds without their objects.
    fn drifting(count: usize, stage_id: u32) -> Self {
        let still = Cloud {
            offset: [Vec2::ZERO; 2],
            vel: Vec2::ZERO,
            desired: Vec2::ZERO,
        };
        let mut clouds = Self {
            objects: Vec::new(),
            clouds: vec![still; count],
            // Not the seed of the simulation's visual generator (`stage id ^ 0x9e3779b9`).
            seed: stage_id ^ CLOUD_SEED,
            rng: DeterministicRng::new(1),
            hurry: false,
        };
        clouds.reset();
        clouds
    }

    pub fn is_empty(&self) -> bool {
        self.clouds.is_empty()
    }

    /// The clouds sped up for the end of the clock.
    pub fn hurried(&self) -> bool {
        self.hurry
    }

    /// Texture offset of cloud `index` at the newest tick.
    pub fn offset(&self, index: usize) -> Option<[f32; 2]> {
        self.clouds.get(index).map(|c| c.offset[1].to_array())
    }

    fn random(&mut self) -> f32 {
        self.rng.next_float() as f32
    }

    /// A new attempt: the constructor of `BgSunset` again, with the same numbers.
    pub fn reset(&mut self) {
        self.rng = DeterministicRng::new(self.seed);
        self.hurry = false;
        for i in 0..self.clouds.len() {
            // `currTexTranslate` is a vector of three; only two reach the texture.
            let offset = Vec2::new(self.random(), self.random());
            self.random();
            let speed = (self.random() * 0.2 + 0.9) * CLOUD_SPEED;
            let angle = self.random() * std::f32::consts::PI;
            let cloud = &mut self.clouds[i];
            cloud.offset = [offset; 2];
            cloud.desired = heading(speed, angle);
            cloud.vel = cloud.desired;
        }
    }

    /// `BgSunset.update` for one simulation tick in which the stage timer advanced by `dt` frames;
    /// `time_left` is [`stage_time_left`].
    pub fn tick(&mut self, dt: f32, time_left: Option<i32>) {
        let speed_up = !self.hurry && time_left.is_some_and(|t| t < CLOUD_HURRY_FRAMES);
        self.hurry |= speed_up;
        let keep = CLOUD_VELOCITY_KEEP.powf(dt);
        for i in 0..self.clouds.len() {
            if speed_up {
                let speed = (self.random() * 0.2 + 0.9) * CLOUD_SPEED_HURRY;
                let angle = self.random() * std::f32::consts::PI;
                self.clouds[i].desired = heading(speed, angle);
            }
            let cloud = &mut self.clouds[i];
            cloud.vel = cloud.desired + (cloud.vel - cloud.desired) * keep;
            cloud.offset[0] = cloud.offset[1];
            cloud.offset[1] += cloud.vel * dt;
        }
    }

    /// Set the texture offsets for a frame `t` of the way from the previous tick to the current one.
    pub fn apply(&self, r: &mut Renderer, t: f32) {
        for (object, cloud) in self.objects.iter().zip(&self.clouds) {
            let offset = cloud.offset[0].lerp(cloud.offset[1], t);
            r.set_tex_offset(*object, offset.to_array());
        }
    }
}

/// Whether a background object of the sunset is one of its cloud layers.
pub fn is_sunset_cloud(model_name: &str) -> bool {
    model_name == "SUN_GROUND" || model_name.starts_with("SUN_CLOUD_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_lists_are_the_reference_ones() {
        assert_eq!(night_window_list(b'A'), ('A', 14));
        assert_eq!(night_window_list(b'B'), ('B', 11));
        assert_eq!(night_window_list(b'E'), ('E', 18));
        assert_eq!(night_window_list(b'G'), ('G', 15));
        // An id outside the table animates list A.
        assert_eq!(night_window_list(b'H'), ('A', 14));
        assert_eq!(night_window_list(0), ('A', 14));
        assert_eq!(night_window_model('C', 7), "NIG_MADO_C_PT07");
        assert_eq!(night_window_model('D', 17), "NIG_MADO_D_PT17");
        assert_eq!(storm_fire_model(0), "STM_FIRE00");
        assert_eq!(storm_fire_model(31), "STM_FIRE31");
        let frames: usize = NIGHT_WINDOW_LISTS.iter().map(|l| l.1).sum();
        assert_eq!(frames, 110);
    }

    #[test]
    fn a_window_shows_each_frame_for_two_ticks_and_wraps() {
        assert_eq!(night_window_frame(0.0, 14), 0);
        assert_eq!(night_window_frame(1.9, 14), 0);
        assert_eq!(night_window_frame(2.0, 14), 1);
        assert_eq!(night_window_frame(27.5, 14), 13);
        assert_eq!(night_window_frame(28.0, 14), 0);
        assert_eq!(night_window_frame(3600.0, 11), 1800 % 11);
        assert_eq!(night_window_frame(-3.0, 14), 0);
        assert_eq!(night_window_frame(f32::NAN, 14), 0);
        assert_eq!(night_window_frame(5.0, 0), 0);
    }

    #[test]
    fn a_fire_shows_a_frame_per_tick_four_frames_per_offset_apart() {
        // The offsets on the disc are the letters 'A' to 'H'.
        assert_eq!(storm_fire_frame(0.0, 65), (4 * 65) % 32);
        assert_eq!(storm_fire_frame(0.0, 66), (4 * 66) % 32);
        assert_eq!(
            (storm_fire_frame(10.0, 66) + 32 - storm_fire_frame(10.0, 65)) % 32,
            4
        );
        assert_eq!(storm_fire_frame(1.0, 65), storm_fire_frame(0.0, 65) + 1);
        assert_eq!(storm_fire_frame(0.99, 65), storm_fire_frame(0.0, 65));
        assert_eq!(storm_fire_frame(27.0, 65), 31);
        assert_eq!(storm_fire_frame(28.0, 65), 0);
    }

    #[test]
    fn the_camera_turn_is_its_yaw() {
        for yaw in [0.0f32, 0.4, 1.5, -2.0, 3.0] {
            let world = Mat4::from_translation(Vec3::new(3.0, 2.0, 1.0))
                * Mat4::from_rotation_y(yaw)
                * Mat4::from_rotation_x(-0.5);
            let camera = Camera {
                view: world.inverse(),
                ..Camera::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y)
            };
            assert!((camera_rot_y(&camera) - yaw).abs() < 1e-5, "{yaw}");
        }
    }

    #[test]
    fn the_scene_clock_follows_the_stage_timer() {
        // The timer reaches 120 when the clock starts.
        assert_eq!(stage_time_left(3600, 0), Some(3600));
        assert_eq!(stage_time_left(3600, 120), Some(3600));
        assert_eq!(stage_time_left(3600, 121), Some(3599));
        assert_eq!(stage_time_left(3600, 3061), Some(659));
        assert_eq!(stage_time_left(1800, 1261), Some(659));
        assert_eq!(stage_time_left(3600, 9000), Some(0));
        assert_eq!(stage_time_left(0, 50), None);
    }

    #[test]
    fn clouds_start_as_the_reference_draws_them_and_the_same_every_time() {
        let c = Clouds::drifting(3, 5);
        assert!(!c.is_empty() && !c.hurried());
        assert!(Clouds::drifting(0, 5).is_empty());
        // The numbers of the clouds, from the generator alone, in the reference's order of draws.
        let mut rng = DeterministicRng::new(5 ^ CLOUD_SEED);
        let mut next = || rng.next_float() as f32;
        for cloud in &c.clouds {
            let offset = Vec2::new(next(), next());
            next();
            let speed = (next() * 0.2 + 0.9) * CLOUD_SPEED;
            let v = heading(speed, next() * std::f32::consts::PI);
            assert_eq!(cloud.offset, [offset; 2]);
            assert_eq!((cloud.vel, cloud.desired), (v, v));
            assert!((0.0..1.0).contains(&offset.x) && (0.0..1.0).contains(&offset.y));
            // Within a tenth of the nominal speed, heading into the half turn the reference picks from.
            assert!((v.length() / CLOUD_SPEED - 1.0).abs() <= 0.1001);
            assert!(v.x <= 1e-9);
        }
        assert!((CLOUD_SPEED as f64 - 0.0015151514671742916).abs() < 1e-9);
        assert!((CLOUD_SPEED_HURRY as f64 - 0.0030303029343485832).abs() < 1e-9);
        // Another load of the stage, or a retry, starts from the same numbers; another stage does not.
        let mut again = Clouds::drifting(3, 5);
        assert_eq!(again.clouds, c.clouds);
        again.tick(1.0, Some(3600));
        assert_ne!(again.clouds, c.clouds);
        again.reset();
        assert_eq!(again.clouds, c.clouds);
        assert_ne!(Clouds::drifting(3, 6).clouds, c.clouds);
    }

    #[test]
    fn clouds_drift_stand_still_with_the_timer_and_hurry_at_the_end() {
        let mut c = Clouds::drifting(2, 15);
        let start = c.clouds.clone();
        // The timer stands still (the fly-in, the time-over sequence): so do the clouds.
        c.tick(0.0, Some(3600));
        assert_eq!(c.clouds, start);
        // One frame: the offset moves by the velocity, which is already the wanted one.
        c.tick(1.0, Some(3600));
        for (now, was) in c.clouds.iter().zip(&start) {
            assert_eq!(now.offset[0], was.offset[1]);
            assert!((now.offset[1] - (was.offset[1] + was.vel)).length() < 1e-7);
            assert_eq!(now.vel, was.vel);
        }
        assert_eq!(c.offset(0), Some(c.clouds[0].offset[1].to_array()));
        assert_eq!(c.offset(2), None);
        // 660 frames left is not yet "fewer than 660".
        c.tick(1.0, Some(CLOUD_HURRY_FRAMES));
        assert!(!c.hurried());
        let before = c.clouds.clone();
        c.tick(1.0, Some(CLOUD_HURRY_FRAMES - 1));
        assert!(c.hurried());
        for (now, was) in c.clouds.iter().zip(&before) {
            // A new heading at twice the speed, approached by a twentieth of the difference per frame.
            assert_ne!(now.desired, was.desired);
            assert!((now.desired.length() / CLOUD_SPEED_HURRY - 1.0).abs() <= 0.1001);
            let expect = now.desired + (was.vel - now.desired) * 0.95;
            assert!((now.vel - expect).length() < 1e-8);
        }
        // The change happens once; a second of frames later the velocity is nearly there.
        let wanted: Vec<Vec2> = c.clouds.iter().map(|c| c.desired).collect();
        for _ in 0..120 {
            c.tick(1.0, Some(100));
        }
        for (cloud, wanted) in c.clouds.iter().zip(&wanted) {
            assert_eq!(cloud.desired, *wanted);
            assert!((cloud.vel - *wanted).length() < CLOUD_SPEED_HURRY * 0.01);
        }
        // Two frames in one tick are the square of one frame's share.
        let mut one = Clouds::drifting(1, 9);
        one.clouds[0].vel = Vec2::ZERO;
        let wanted = one.clouds[0].desired;
        one.tick(2.0, None);
        assert!((one.clouds[0].vel - wanted * (1.0 - 0.95 * 0.95)).length() < 1e-8);
        assert!(!one.hurried(), "a stage without a clock never hurries");
    }

    #[test]
    fn cloud_layers_are_told_by_name() {
        assert!(is_sunset_cloud("SUN_GROUND"));
        assert!(is_sunset_cloud("SUN_CLOUD_A"));
        assert!(!is_sunset_cloud("SUN_SKY") && !is_sunset_cloud("SUN_CLOUD"));
    }
}
