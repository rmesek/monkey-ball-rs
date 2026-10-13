//! GX-material renderer on wgpu. No windowing code: the caller supplies the `Device`, `Queue`, colour
//! format and a target `TextureView` (a swapchain frame or an [`OffscreenTarget`]).
//!
//! ```ignore
//! let mut r = Renderer::new(device, queue, format);
//! let tex: Vec<_> = tpl.textures().map(|t| r.upload_tpl_texture(t)).collect();
//! let model = r.upload_model(gma.model("STG001").unwrap(), &tex)?;
//! let obj = r.add_object(model, Layer::World, Mat4::IDENTITY);
//! r.set_transform(obj, Mat4::from_translation(Vec3::Y));
//! r.render(&view, (w, h), &Camera::look_at(eye, target, Vec3::Y));
//! ```
//!
//! Particles and the ball's shadow (`effects.rs`) are set up while loading and fed every frame; they cost
//! one draw per mesh of a batch that has instances, one per run of quads, and none for the shadow:
//!
//! ```ignore
//! // Loading.
//! let glow = r.create_instance_batch(circle_model, InstanceStyle::GLOW);
//! r.reserve_quads(256);
//! r.warm_quad_texture(sparkle);
//! r.set_shadow_mask(Some((circle_texture, sampler_key(layer, w, h))));
//! r.set_shadow_receiver(stage_object, true);
//! // Every frame, before `render`.
//! r.push_instance(glow, &transform, [red, green, blue, 1.0]);
//! r.push_quad(sparkle, BlendMode::Additive, DrawOrder::AfterSorted, &Quad::new(corners, rgba));
//! r.set_shadow(Shadow::projected(ball_position, ball_radius, &r.lighting));
//! ```
//!
//! Per-object material parameters of the reference's `RenderParams`, the bonus wave and blended HUD
//! sprites:
//!
//! ```ignore
//! r.set_tex_offset(cloud, [u, v]);                       // texMtx; add set_tex_matrix_every_layer on SMB1
//! r.set_specular_disabled(inner_shell, true);            // disableSpecular
//! r.set_wave(floor, Some(WaveParams::BONUS_WAVE));       // loading
//! r.set_wave_time(stage_timer_frames);                   // every frame
//! r.draw_sprite_blended(banner, SpriteBlend::Additive);
//! let before = r.counters();                             // GPU objects created so far
//! r.reserve_objects(object_count);                       // loading: the first frame creates nothing
//! ```
//!
//! Mirror floors (`mirror.rs`; the game sets one on 37 course stages: those whose stagedef names mirror
//! models, and the bonus wave): one more scene pass into a reduced target, cut to the mirror models in
//! view, and one draw per mesh of those models, only while a mirror is set and one of them is in view:
//!
//! ```ignore
//! // Loading, after `clear_scene` and the objects.
//! r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));      // or Mirror::wavy(plane, gb_grad_texture)
//! r.set_mirror_surface(mirror_model_object, true);      // or set_mirror_surface_only: reflection alone
//! r.set_in_mirror(bg_object_with_flag_bit_7, false);
//! // Every frame, for a mirror that moves.
//! r.set_mirror_plane(anim_group_transform);
//! let extra = r.stats.mirror_draws;                      // and r.mirror_bytes() for the target
//! ```
//!
//! The material model is described in the `material` module and in `docs/GMA.md` ("How `mb-render`
//! draws the materials"), with the differences from the reference that remain.

pub mod camera;
pub mod effects;
pub mod gpu_types;
mod instances;
pub mod lighting;
pub mod material;
pub mod mirror;
pub mod offscreen;
mod quads;
pub mod renderer;
pub mod sprite;
pub mod wave;

pub use camera::Camera;
pub use effects::{BlendMode, DrawOrder, InstanceBatchId, InstanceStyle, Quad, Shadow};
pub use glam;
pub use lighting::Lighting;
pub use material::SamplerKey;
pub use mirror::{Distortion, Mirror};
pub use offscreen::{OffscreenTarget, rgba_to_ppm};
pub use renderer::{
    DEPTH_FORMAT, FrameStats, GpuCounters, GpuTexture, Layer, LoadStats, ModelId, ObjectId,
    Renderer, TextureId, TextureSet,
};
pub use sprite::{Sprite, SpriteBlend};
pub use wave::WaveParams;
pub use wgpu;
