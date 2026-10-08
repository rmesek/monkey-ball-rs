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
//! See `docs/RENDER.md` for the material model and known gaps.

pub mod camera;
pub mod gpu_types;
pub mod lighting;
pub mod material;
pub mod offscreen;
pub mod renderer;
pub mod sprite;

pub use camera::Camera;
pub use glam;
pub use lighting::Lighting;
pub use offscreen::{rgba_to_ppm, OffscreenTarget};
pub use renderer::{
    Fog, FrameStats, GpuTexture, Layer, LoadStats, ModelId, ObjectId, Renderer, TextureId,
    TextureSet, DEPTH_FORMAT,
};
pub use sprite::Sprite;
pub use wgpu;
