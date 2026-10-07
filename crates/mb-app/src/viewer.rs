//! Stage viewer: loads one SMB1 stage and lets you fly around it. This is the first [`Game`] state; menus,
//! the ball and the HUD will be separate states on the same stack.

use glam::{Mat3, Vec3};
use mb_render::Camera;

use crate::game::{Game, GameCtx, Transition};
use crate::input::{Action, Axis, InputState};
use crate::stage_info::BgInfo;
use crate::stage_scene::{StageAssets, StageScene, S16_TO_RADIANS};

/// Free-fly camera pose. Yaw turns about +Y (positive = left), pitch about the camera X (positive = up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlyCamera {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl FlyCamera {
    pub fn forward(&self) -> Vec3 {
        Mat3::from_rotation_y(self.yaw) * (Mat3::from_rotation_x(self.pitch) * Vec3::NEG_Z)
    }

    pub fn right(&self) -> Vec3 {
        Mat3::from_rotation_y(self.yaw) * Vec3::X
    }

    pub fn camera(&self) -> Camera {
        Camera::from_pose(self.pos, self.yaw, self.pitch)
    }

    /// Apply input for `dt` seconds. `speed` is units per second.
    pub fn update(&mut self, input: &InputState, dt: f32, speed: f32) {
        const MOUSE_SENS: f32 = 0.003;
        const KEY_TURN: f32 = 1.6;
        let (dx, dy) = input.look_delta();
        self.yaw -= dx * MOUSE_SENS;
        self.pitch -= dy * MOUSE_SENS;
        self.yaw -=
            input.axis_with_keys(Axis::LookX, Action::LookLeft, Action::LookRight) * KEY_TURN * dt;
        self.pitch +=
            input.axis_with_keys(Axis::LookY, Action::LookDown, Action::LookUp) * KEY_TURN * dt;
        self.pitch = self.pitch.clamp(-1.55, 1.55);

        let fwd = input.axis_with_keys(Axis::MoveY, Action::MoveBack, Action::MoveForward);
        let side = input.axis_with_keys(Axis::MoveX, Action::MoveLeft, Action::MoveRight);
        let up =
            input.held(Action::MoveUp) as i32 as f32 - input.held(Action::MoveDown) as i32 as f32;
        let boost = if input.held(Action::Fast) { 4.0 } else { 1.0 };
        let delta = self.forward() * fwd + self.right() * side + Vec3::Y * up;
        self.pos += delta * speed * boost * dt;
    }
}

/// Default camera behind the first start position, looking along its facing direction.
pub fn default_pose(start: Vec3, start_rot_y: i16) -> FlyCamera {
    let yaw = start_rot_y as f32 * S16_TO_RADIANS;
    let fwd = Mat3::from_rotation_y(yaw) * Vec3::NEG_Z;
    FlyCamera {
        pos: start - fwd * 10.0 + Vec3::Y * 5.0,
        yaw,
        pitch: -0.3,
    }
}

pub struct StageViewer {
    stage_id: u32,
    bg: Option<BgInfo>,
    pose_override: Option<FlyCamera>,
    fly: FlyCamera,
    assets: Option<StageAssets>,
    scene: Option<StageScene>,
    stats: String,
    anim_time: f32,
    anim_paused: bool,
}

impl StageViewer {
    pub fn new(stage_id: u32, bg: Option<BgInfo>, pose_override: Option<FlyCamera>) -> Self {
        Self {
            stage_id,
            bg,
            pose_override,
            fly: pose_override.unwrap_or(FlyCamera {
                pos: Vec3::new(0.0, 5.0, 10.0),
                yaw: 0.0,
                pitch: -0.3,
            }),
            assets: None,
            scene: None,
            stats: String::new(),
            anim_time: 0.0,
            anim_paused: false,
        }
    }

    /// Start the stage animation at `seconds` (used by headless captures).
    pub fn with_anim_time(mut self, seconds: f32) -> Self {
        self.anim_time = seconds;
        self
    }

    pub fn stage_id(&self) -> u32 {
        self.stage_id
    }

    pub fn scene(&self) -> Option<&StageScene> {
        self.scene.as_ref()
    }

    pub fn assets(&self) -> Option<&StageAssets> {
        self.assets.as_ref()
    }

    fn stage_exists(ctx: &GameCtx, id: u32) -> bool {
        ctx.content
            .read(&format!("smb1_content/test/st{id:03}/STAGE{id:03}.lz"))
            .is_ok()
    }

    /// Next/previous stage that exists on the disc (ids have gaps).
    fn neighbour(ctx: &GameCtx, from: u32, step: i32) -> Option<u32> {
        let mut id = from as i32;
        for _ in 0..200 {
            id += step;
            if !(1..=999).contains(&id) {
                return None;
            }
            if Self::stage_exists(ctx, id as u32) {
                return Some(id as u32);
            }
        }
        None
    }
}

impl Game for StageViewer {
    fn name(&self) -> &'static str {
        "stage viewer"
    }

    fn on_enter(&mut self, ctx: &mut GameCtx) -> Result<(), String> {
        let assets = StageAssets::load(ctx.content, self.stage_id, self.bg)
            .map_err(|e| format!("stage {}: {e}", self.stage_id))?;
        let scene = StageScene::build(ctx.renderer, &assets);
        if self.pose_override.is_none() {
            let rot_y = assets
                .stagedef
                .start_positions
                .first()
                .map(|s| s.rot.y)
                .unwrap_or(0);
            self.fly = default_pose(scene.start, rot_y);
        }
        if !scene.missing_models.is_empty() {
            eprintln!("missing models: {:?}", scene.missing_models);
        }
        let objs = scene.items.len();
        self.stats = format!(
            "stage {} bg {} | {} group objs, {} bg, {} fg{}",
            self.stage_id,
            assets.bg.file_name,
            objs,
            scene.bg_objects.len(),
            scene.fg_objects.len(),
            if scene.missing_models.is_empty() {
                String::new()
            } else {
                format!(", {} missing models", scene.missing_models.len())
            }
        );
        self.assets = Some(assets);
        self.scene = Some(scene);
        Ok(())
    }

    fn on_exit(&mut self, ctx: &mut GameCtx) {
        ctx.renderer.clear_scene();
        if let Some(s) = &self.scene {
            ctx.renderer.truncate_textures(s.texture_base);
        }
        self.scene = None;
        self.assets = None;
    }

    fn update(&mut self, ctx: &mut GameCtx) -> Transition {
        if ctx.input.pressed(Action::Cancel) {
            return Transition::Quit;
        }
        if ctx.input.pressed(Action::ResetCamera) {
            if let (Some(a), Some(s)) = (&self.assets, &self.scene) {
                let rot_y = a
                    .stagedef
                    .start_positions
                    .first()
                    .map(|p| p.rot.y)
                    .unwrap_or(0);
                self.fly = default_pose(s.start, rot_y);
            }
        }
        for (action, step) in [(Action::NextStage, 1), (Action::PrevStage, -1)] {
            if ctx.input.pressed(action) {
                if let Some(id) = Self::neighbour(ctx, self.stage_id, step) {
                    return Transition::Replace(Box::new(StageViewer::new(id, None, None)));
                }
            }
        }
        if ctx.input.pressed(Action::Pause) {
            self.anim_paused = !self.anim_paused;
        }
        if !self.anim_paused {
            self.anim_time += ctx.dt;
        }
        if let (Some(a), Some(s)) = (&self.assets, &self.scene) {
            s.set_time(ctx.renderer, &a.stagedef, self.anim_time);
        }
        self.fly.update(ctx.input, ctx.dt, 12.0);
        Transition::None
    }

    fn camera(&self) -> Camera {
        self.fly.camera()
    }

    fn status(&self) -> String {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_follows_yaw_and_pitch() {
        let c = FlyCamera {
            pos: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        };
        assert!((c.forward() - Vec3::NEG_Z).length() < 1e-6);
        let c = FlyCamera {
            yaw: std::f32::consts::FRAC_PI_2,
            ..c
        };
        // Turning left by 90 degrees faces -X.
        assert!((c.forward() - Vec3::NEG_X).length() < 1e-5);
        let up = FlyCamera { pitch: 0.5, ..c };
        assert!(up.forward().y > 0.0);
    }

    #[test]
    fn flying_forward_moves_along_view() {
        let mut c = FlyCamera {
            pos: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        };
        let mut input = InputState::new();
        input.set_action(Action::MoveForward, true);
        c.update(&input, 1.0, 2.0);
        assert!((c.pos - Vec3::new(0.0, 0.0, -2.0)).length() < 1e-5);
    }

    #[test]
    fn default_pose_is_behind_start() {
        let p = default_pose(Vec3::new(0.0, 1.0, 0.0), 0);
        assert!(p.pos.z > 5.0 && p.pos.y > 1.0);
        // Camera at +Z looking towards -Z sees the start point.
        assert!(p.forward().z < 0.0);
    }

    #[test]
    fn pitch_is_clamped() {
        let mut c = FlyCamera {
            pos: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        };
        let mut input = InputState::new();
        input.set_mouse_look(true);
        input.add_mouse_delta(0.0, -1.0e6);
        c.update(&input, 0.0, 1.0);
        assert!(c.pitch <= 1.55);
    }
}
