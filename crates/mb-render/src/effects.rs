//! What the caller says about particle-like drawing: instanced models, world-space quads and the projected
//! shadow. Everything here is plain data; the GPU side is in `renderer.rs`, `instances.rs` and `quads.rs`.
//!
//! The states mirror the ones the reference sets for its effects (`World.ts`, `glowMegaState`,
//! `flashMegaState`, `streakMegaState`, `shadowMegaState`).

use glam::{Mat4, Vec3};

use crate::lighting::Lighting;

/// Blending of an effect over the frame. Both leave the target's alpha alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendMode {
    /// `SrcAlpha, OneMinusSrcAlpha`.
    Alpha,
    /// `SrcAlpha, One`: the reference's glow, flash, streak and sparkle state.
    Additive,
}

impl BlendMode {
    pub fn factors(self) -> (wgpu::BlendFactor, wgpu::BlendFactor) {
        match self {
            BlendMode::Alpha => (
                wgpu::BlendFactor::SrcAlpha,
                wgpu::BlendFactor::OneMinusSrcAlpha,
            ),
            BlendMode::Additive => (wgpu::BlendFactor::SrcAlpha, wgpu::BlendFactor::One),
        }
    }
}

/// Where in the world pass an instance batch or a run of quads is drawn. One draw call holds many
/// particles, so they cannot be sorted one by one against the scene's blended objects; the caller picks
/// the side instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DrawOrder {
    /// With the opaque geometry (the reference's `RenderSort.None`: confetti).
    Opaque,
    /// After the opaque geometry, before the objects that are sorted back to front.
    BeforeSorted,
    /// After the sorted objects (the default for effects: they then show in front of the ball's shells
    /// where they are nearer, and are hidden by them where they are farther).
    AfterSorted,
}

pub(crate) const DRAW_ORDERS: [DrawOrder; 3] = [
    DrawOrder::Opaque,
    DrawOrder::BeforeSorted,
    DrawOrder::AfterSorted,
];

/// How the instances of one batch are drawn. `None` / `false` keeps what each mesh's material says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InstanceStyle {
    pub order: DrawOrder,
    /// Replace the material's blend factors.
    pub blend: Option<BlendMode>,
    /// Replace the material's depth write (the depth test stays).
    pub depth_write: Option<bool>,
    /// Draw both faces whatever the mesh culls.
    pub two_sided: bool,
    /// Pull the batch one depth step towards the camera (the reference's `polygonOffset`).
    pub depth_bias: bool,
    /// The stage tilt ([`crate::Renderer::set_tilt`]) applies on top of each instance transform.
    pub tilted: bool,
}

impl InstanceStyle {
    /// The model as its materials define it, drawn with the opaque geometry: confetti. Meshes whose
    /// alpha is always one do not blend here, so an instance alpha below one needs [`Self::SORTED`].
    pub const OPAQUE: Self = Self {
        order: DrawOrder::Opaque,
        blend: None,
        depth_write: None,
        two_sided: false,
        depth_bias: false,
        tilted: true,
    };
    /// The model as its materials define it, blended, after the sorted objects: the star.
    pub const SORTED: Self = Self {
        order: DrawOrder::AfterSorted,
        ..Self::OPAQUE
    };
    /// `glowMegaState`: additive, no depth write, both faces.
    pub const GLOW: Self = Self {
        order: DrawOrder::AfterSorted,
        blend: Some(BlendMode::Additive),
        depth_write: Some(false),
        two_sided: true,
        depth_bias: false,
        tilted: true,
    };
    /// `flashMegaState`: the glow state plus the depth bias.
    pub const FLASH: Self = Self {
        depth_bias: true,
        ..Self::GLOW
    };
}

/// Handle to an instance batch (valid until [`crate::Renderer::clear_scene`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InstanceBatchId(pub(crate) usize);

/// A quad in world space (after any stage tilt). Corners 0, 1, 2 and 2, 1, 3 form its two triangles, which
/// is the order of the reference's streak (`head0, head1, tail0, tail1`) and sparkle (`top-left, top-right,
/// bottom-left, bottom-right`). Both faces are drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub positions: [Vec3; 4],
    pub uvs: [[f32; 2]; 4],
    /// Straight-alpha RGBA per corner, multiplied with the texel.
    pub colors: [[u8; 4]; 4],
}

impl Quad {
    /// A quad showing the whole texture in one colour.
    pub fn new(positions: [Vec3; 4], color: [u8; 4]) -> Self {
        Self {
            positions,
            uvs: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
            colors: [color; 4],
        }
    }
}

/// The projected shadow of one round caster (the ball) on the objects flagged with
/// [`crate::Renderer::set_shadow_receiver`]: the fragment maths of the reference's shadow program
/// (`createShadowProgram`), evaluated inside the stage shader instead of in a second geometry pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// World space before the stage tilt -> shadow space, in which the light looks down -Z at the caster
    /// and the caster's centre is at x = y = 0 (`scratchShadowView`).
    pub from_world: Mat4,
    /// Half the side of the projected mask (`u_ShadowInfo.x`).
    pub radius: f32,
    /// z of the caster's centre in shadow space (`u_ShadowInfo.y`).
    pub depth: f32,
    /// Opacity lost per unit behind the caster (`u_ShadowInfo.z`).
    pub fade: f32,
    /// Colour and peak opacity (`u_ShadowColor`).
    pub color: [f32; 4],
}

impl Shadow {
    /// `SHADOW_LIGHT_OFFSET`: how far towards the light the projection starts.
    pub const LIGHT_OFFSET: f32 = 15.0;
    /// `SHADOW_RADIUS_SCALE`.
    pub const RADIUS_SCALE: f32 = 1.4;
    /// `SHADOW_FADE_SCALE`.
    pub const FADE_SCALE: f32 = 0.2;
    /// `shadowColor`.
    pub const COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.4];

    /// The reference's `drawProjectedShadow` for a caster at `center` (stage space, before the tilt) with
    /// `caster_radius`, lit by the scene's directional light. `None` when there is nothing to cast.
    pub fn projected(center: Vec3, caster_radius: f32, lighting: &Lighting) -> Option<Self> {
        let radius = caster_radius * Self::RADIUS_SCALE;
        if radius.is_nan() || radius <= 0.0 {
            return None;
        }
        let light = center + lighting.direction_to_light() * Self::LIGHT_OFFSET;
        let from_world = Mat4::look_at_rh(light, center, lighting.light_up());
        Some(Self {
            from_world,
            radius,
            depth: from_world.transform_point3(center).z,
            fade: Self::FADE_SCALE,
            color: Self::COLOR,
        })
    }

    /// How strongly the shadow covers a point of a receiver, 0..=1 of `color` (the fragment maths of the
    /// shader without the mask texture, whose value is passed in). `normal` is the surface normal in the
    /// same space as `point`, zero when the vertex has none.
    pub fn coverage(&self, point: Vec3, normal: Vec3, mask: f32) -> f32 {
        let p = self.from_world.transform_point3(point);
        let uv = p.truncate() / (self.radius * 2.0) + 0.5;
        if !(0.0..=1.0).contains(&uv.x) || !(0.0..=1.0).contains(&uv.y) {
            return 0.0;
        }
        let n = self.from_world.transform_vector3(normal);
        if n.length() > 1e-5 && n.z <= 0.0 {
            return 0.0;
        }
        let fade = (1.0 - (self.depth - p.z).max(0.0) * self.fade).clamp(0.0, 1.0);
        (mask * self.color[3] * fade).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overhead() -> Lighting {
        // rotX = a quarter turn takes (0, 0, -1) to straight up.
        Lighting {
            light_rot_x: 16384,
            light_rot_y: 0,
            ..Lighting::default()
        }
    }

    #[test]
    fn projected_shadow_matches_the_reference_constants() {
        let center = Vec3::new(3.0, 1.0, -2.0);
        let s = Shadow::projected(center, 0.5, &overhead()).unwrap();
        assert!((s.radius - 0.7).abs() < 1e-6);
        // The caster is `LIGHT_OFFSET` in front of the light, which looks down -Z.
        assert!((s.depth + 15.0).abs() < 1e-4, "{}", s.depth);
        let c = s.from_world.transform_point3(center);
        assert!(c.truncate().length() < 1e-4);
        assert_eq!(s.fade, 0.2);
        assert_eq!(s.color, [0.0, 0.0, 0.0, 0.4]);
        assert!(Shadow::projected(center, 0.0, &overhead()).is_none());
        assert!(Shadow::projected(center, f32::NAN, &overhead()).is_none());
    }

    #[test]
    fn coverage_follows_the_reference_fragment_maths() {
        let center = Vec3::new(0.0, 0.5, 0.0);
        let s = Shadow::projected(center, 0.5, &overhead()).unwrap();
        let up = Vec3::Y;
        // Straight below the caster, half a unit behind its centre: 1 - 0.5 * 0.2 of the peak.
        let under = s.coverage(Vec3::ZERO, up, 1.0);
        assert!((under - 0.4 * 0.9).abs() < 1e-5, "{under}");
        // The mask scales it; outside the projected square there is none.
        assert!((s.coverage(Vec3::ZERO, up, 0.5) - 0.2 * 0.9).abs() < 1e-5);
        assert_eq!(s.coverage(Vec3::new(0.71, 0.0, 0.0), up, 1.0), 0.0);
        assert!(s.coverage(Vec3::new(0.69, 0.0, 0.0), up, 1.0) > 0.0);
        // Faces turned away from the light get none; a vertex without a normal is not tested.
        assert_eq!(s.coverage(Vec3::ZERO, -up, 1.0), 0.0);
        assert!(s.coverage(Vec3::ZERO, Vec3::ZERO, 1.0) > 0.0);
        // Gone five units behind the caster, full in front of it.
        assert_eq!(s.coverage(Vec3::new(0.0, -4.5, 0.0), up, 1.0), 0.0);
        assert!((s.coverage(Vec3::new(0.0, 2.0, 0.0), up, 1.0) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn presets_carry_the_reference_states() {
        assert_eq!(InstanceStyle::GLOW.blend, Some(BlendMode::Additive));
        assert_eq!(InstanceStyle::GLOW.depth_write, Some(false));
        assert_eq!(
            (
                InstanceStyle::GLOW.two_sided,
                InstanceStyle::GLOW.depth_bias
            ),
            (true, false)
        );
        // The flash is the glow plus the bias.
        assert_eq!(
            InstanceStyle::FLASH,
            InstanceStyle {
                depth_bias: true,
                ..InstanceStyle::GLOW
            }
        );
        assert_eq!(InstanceStyle::OPAQUE.order, DrawOrder::Opaque);
        assert_eq!(InstanceStyle::SORTED.blend, None);
        assert_eq!(
            BlendMode::Additive.factors(),
            (wgpu::BlendFactor::SrcAlpha, wgpu::BlendFactor::One)
        );
    }
}
