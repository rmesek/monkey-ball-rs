//! wgpu adapter / device setup of the desktop window and the windowless test driver (blocking; the browser
//! driver sets its device up asynchronously in `shell`), and the surface format both drivers pick.

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
            required_limits: adapter.limits(),
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
