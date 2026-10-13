//! What every platform driver does the same way when it sets the GPU up: the instance, the limits to ask
//! for, the surface format, and the blocking adapter / device request of the native drivers and of the
//! windowless test driver (the browser driver awaits the same requests in `mb-web`'s `shell.rs`).

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// One-line description for logs ("NVIDIA ... (Vulkan, DiscreteGpu)").
    pub fn describe(&self) -> String {
        let i = self.adapter.get_info();
        format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type)
    }
}

/// The instance both drivers and the windowless one create, on `backends` (`None`: all there are).
///
/// On the OpenGL backend (WebGL2 in the browser) submitted work counts as done when it is submitted
/// (`GlFenceBehavior::AutoFinish`). `wgpu` keeps what a submission used until its fence is signalled:
/// the command list of the frame and a staging copy of every texture and buffer written since the
/// submit before. WebGL only updates a fence between two tasks of the page, so with real fences a whole
/// stage load (up to 16.6 MB of textures) and two frames' command lists were held at once, in memory
/// that wasm never gives back. OpenGL runs the commands in order and keeps deleted objects alive while
/// they are in use, so nothing needs the wait; the game maps no buffer and asks for no completion
/// callback.
pub fn instance_descriptor(backends: Option<wgpu::Backends>) -> wgpu::InstanceDescriptor {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    if let Some(backends) = backends {
        descriptor.backends = backends;
    }
    descriptor.backend_options.gl.fence_behavior = wgpu::GlFenceBehavior::AutoFinish;
    descriptor
}

/// Limits to request. GL adapters (WebGL2 in the browser, OpenGL ES on a phone, OpenGL on a desktop without
/// anything newer) get the WebGL2 baseline, raised to the adapter's texture size, which is what the renderer
/// is written against; requesting everything a GL adapter reports is not portable. The other backends
/// (Vulkan, Metal, DX12, WebGPU) get what the adapter has.
pub fn required_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    if adapter.get_info().backend == wgpu::Backend::Gl {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
    } else {
        adapter.limits()
    }
}

/// Request an adapter and device. With `compatible_surface` the adapter can present to it; without one
/// (headless) a software adapter is accepted as a fallback so screenshots work on machines without a GPU.
#[cfg(not(target_arch = "wasm32"))]
pub fn request_gpu(
    instance: wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
    force_software: bool,
) -> Result<Gpu, String> {
    let mut attempts = vec![false];
    if force_software {
        attempts = vec![true];
    } else {
        attempts.push(true);
    }
    let mut last_err = String::from("no adapter");
    for fallback in attempts {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            compatible_surface,
            ..Default::default()
        }));
        let adapter = match adapter {
            Ok(a) => a,
            Err(e) => {
                last_err = format!("{e}");
                continue;
            }
        };
        let device = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mb device"),
            required_features: wgpu::Features::empty(),
            required_limits: required_limits(&adapter),
            ..Default::default()
        }));
        match device {
            Ok((device, queue)) => {
                return Ok(Gpu {
                    instance,
                    adapter,
                    device,
                    queue,
                })
            }
            Err(e) => last_err = format!("{e}"),
        }
    }
    Err(format!("could not create a GPU device: {last_err}"))
}

/// Prefer a non-sRGB 8-bit format: GX colours are display-referred, so we want no implicit encoding.
pub fn pick_surface_format(formats: &[wgpu::TextureFormat]) -> wgpu::TextureFormat {
    formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .or_else(|| formats.first().copied())
        .unwrap_or(wgpu::TextureFormat::Bgra8Unorm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::TextureFormat as F;

    #[test]
    fn prefers_linear_surface_format() {
        assert_eq!(
            pick_surface_format(&[F::Bgra8UnormSrgb, F::Bgra8Unorm]),
            F::Bgra8Unorm
        );
        assert_eq!(pick_surface_format(&[F::Rgba8UnormSrgb]), F::Rgba8UnormSrgb);
        assert_eq!(pick_surface_format(&[]), F::Bgra8Unorm);
    }
}
