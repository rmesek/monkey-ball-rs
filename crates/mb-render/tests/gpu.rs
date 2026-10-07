//! GPU-gated tests: they skip (pass) when no adapter is available.

use mb_render::glam::{Mat4, Vec3};
use mb_render::{Camera, OffscreenTarget, Renderer, Sprite};

fn setup() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    for fallback in [false, true] {
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                force_fallback_adapter: fallback,
                ..Default::default()
            }))
        else {
            continue;
        };
        if let Ok(dq) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        {
            return Some(dq);
        }
    }
    eprintln!("skipped: no GPU adapter");
    None
}

#[test]
fn clear_color_and_sprite_readback() {
    let Some((device, queue)) = setup() else {
        return;
    };
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let mut r = Renderer::new(device.clone(), queue, fmt);
    r.clear_color = [0.0, 0.0, 1.0, 1.0];
    let tex = r.upload_rgba(1, 1, &[255, 0, 0, 255]);
    r.draw_sprite(Sprite::new(tex, 0.0, 0.0, 8.0, 8.0));
    let target = OffscreenTarget::new(&device, 16, 16, fmt);
    let cam = Camera::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
    let px = r.render_to_rgba(&target, &cam);
    let at = |x: usize, y: usize| &px[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
    assert_eq!(at(12, 12), &[0, 0, 255, 255], "clear colour");
    assert_eq!(at(2, 2), &[255, 0, 0, 255], "sprite");
    let _ = Mat4::IDENTITY;
}
