//! Headless capture: render a game state once into an offscreen texture and return the pixels.

use mb_render::{OffscreenTarget, Renderer};

use crate::content::Content;
use crate::game::{Game, GameCtx, GameStack};
use crate::gpu::request_gpu;
use crate::input::InputState;

pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub adapter: String,
    pub status: String,
    pub draws: usize,
}

/// Create a GPU device without a window (software adapter as a fallback), run `game` for one frame at
/// `dt = 0` and read the frame back as RGBA8.
pub fn capture(
    content: &Content,
    game: Box<dyn Game>,
    size: (u32, u32),
    force_software: bool,
) -> Result<Capture, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let gpu = request_gpu(instance, None, force_software)?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, size.0, size.1, format);

    let input = InputState::new();
    let mut stack = GameStack::new();
    let mut ctx = GameCtx {
        dt: 0.0,
        time: 0.0,
        input: &input,
        renderer: &mut renderer,
        content,
        size,
    };
    stack.push(game, &mut ctx)?;
    stack.update(&mut ctx)?;
    let top = stack.top().ok_or("no game state")?;
    let camera = top.camera();
    let status = top.status();
    let rgba = renderer.render_to_rgba(&target, &camera);
    Ok(Capture {
        status,
        draws: renderer.stats.draws,
        width: size.0,
        height: size.1,
        rgba,
        adapter: gpu.describe(),
    })
}
