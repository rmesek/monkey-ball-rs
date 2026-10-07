//! Shared windowed driver for the mobile and browser shells (`mb-android`, `mb-web`).
//!
//! The desktop driver in [`crate::app`] blocks on GPU setup and uses `std::time`; neither works in the
//! browser (no blocking, `Instant::now` panics), and Android additionally loses its surface on suspend.
//! This driver therefore (a) runs GPU setup through a [`Platform::run_local`] hook so the web shell can spawn it
//! on the JS event loop, (b) uses `web_time` clocks, and (c) drops/recreates only the surface across
//! suspend/resume. Platform specifics (safe-area insets, gamepad, tilt, error display) are the [`Platform`] trait.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use mb_render::Renderer;
use web_time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::content::Content;
use crate::game::{Game, GameCtx, GameStack};
use crate::gpu::{pick_surface_format, Gpu};
use crate::input::{action_for_key, Action, InputState};
use crate::ui::{Insets, Layout, TouchControls, TouchPhase};

/// Hooks a platform shell provides to the shared driver. All methods have neutral defaults.
pub trait Platform: 'static {
    /// Customise the window (the web shell attaches its canvas here).
    fn window_attributes(&mut self, attrs: WindowAttributes) -> WindowAttributes {
        attrs
    }
    /// Safe-area insets in physical pixels, queried every frame (rotation, IME and cutouts change them).
    fn insets(&mut self, _window: &Window) -> Insets {
        Insets::default()
    }
    /// Per-frame polling of non-event input: gamepads, accelerometer.
    fn poll_input(&mut self, _input: &mut InputState) {}
    /// Drive a future to completion. Native blocks; the web shell spawns it on the JS event loop.
    fn run_local(&mut self, fut: Pin<Box<dyn Future<Output = ()>>>) {
        pollster::block_on(fut);
    }
    /// Fatal error: show it to the user (the driver then stops).
    fn on_error(&mut self, msg: &str) {
        eprintln!("error: {msg}");
    }
}

type GpuInit = Result<(wgpu::Surface<'static>, Gpu), String>;

struct Live {
    window: Arc<Window>,
    /// `None` while suspended (Android destroys the native window surface).
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
}

struct Shell<P: Platform> {
    platform: P,
    instance: Option<wgpu::Instance>,
    content: Content,
    initial: Option<Box<dyn Game>>,
    stack: GameStack,
    input: InputState,
    touch: TouchControls,
    window: Option<Arc<Window>>,
    pending: Rc<RefCell<Option<GpuInit>>>,
    live: Option<Live>,
    start: Instant,
    last: Instant,
    failed: bool,
}

async fn init_gpu(instance: wgpu::Instance, surface: wgpu::Surface<'static>) -> GpuInit {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("no compatible GPU adapter: {e}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("mb device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("could not create a GPU device: {e}"))?;
    Ok((
        surface,
        Gpu {
            instance,
            adapter,
            device,
            queue,
        },
    ))
}

impl<P: Platform> Shell<P> {
    fn fail(&mut self, event_loop: &ActiveEventLoop, msg: String) {
        self.failed = true;
        self.platform.on_error(&msg);
        event_loop.exit();
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attrs = self
            .platform
            .window_attributes(Window::default_attributes().with_title("Monkey Ball"));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("window: {e}"))?,
        );
        let instance = self.instance.clone().ok_or("no wgpu instance")?;
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        self.window = Some(window);
        let slot = self.pending.clone();
        self.platform.run_local(Box::pin(async move {
            *slot.borrow_mut() = Some(init_gpu(instance, surface).await);
        }));
        Ok(())
    }

    /// Finish setup once the (possibly asynchronous) GPU init has landed.
    fn finish_init(&mut self) -> Result<(), String> {
        let Some(result) = self.pending.borrow_mut().take() else {
            return Ok(());
        };
        let (surface, gpu) = result?;
        let window = self.window.clone().ok_or("window missing")?;
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = pick_surface_format(&caps.formats);
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by adapter")?;
        config.format = format;
        surface.configure(&gpu.device, &config);
        let renderer = Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
        self.live = Some(Live {
            window,
            surface: Some(surface),
            config,
            gpu,
            renderer,
        });
        if let Some(game) = self.initial.take() {
            let st = self.live.as_mut().unwrap();
            let mut ctx = GameCtx {
                dt: 0.0,
                time: 0.0,
                input: &self.input,
                renderer: &mut st.renderer,
                content: &self.content,
                size: (st.config.width, st.config.height),
            };
            self.stack.push(game, &mut ctx)?;
        }
        self.last = Instant::now();
        Ok(())
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_none() {
            if let Err(e) = self.finish_init() {
                self.fail(event_loop, e);
            }
            return;
        }
        let st = self.live.as_mut().unwrap();
        if st.surface.is_none() {
            return;
        }
        self.platform.poll_input(&mut self.input);
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        let size = (st.config.width, st.config.height);
        let mut ctx = GameCtx {
            dt,
            time: (now - self.start).as_secs_f64(),
            input: &self.input,
            renderer: &mut st.renderer,
            content: &self.content,
            size,
        };
        match self.stack.update(&mut ctx) {
            Ok(true) => {}
            Ok(false) => {
                event_loop.exit();
                return;
            }
            Err(e) => {
                self.fail(event_loop, e);
                return;
            }
        }
        self.input.end_frame();
        let Some(top) = self.stack.top() else {
            return;
        };
        let camera = top.camera();
        let st = self.live.as_mut().unwrap();
        let surface = st.surface.as_ref().unwrap();
        use wgpu::CurrentSurfaceTexture as C;
        let frame = match surface.get_current_texture() {
            C::Success(f) | C::Suboptimal(f) => f,
            C::Outdated | C::Lost => {
                surface.configure(&st.gpu.device, &st.config);
                return;
            }
            C::Timeout | C::Occluded | C::Validation => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        st.renderer.render(&view, size, &camera);
        st.window.pre_present_notify();
        st.gpu.queue.present(frame);
    }
}

impl<P: Platform> ApplicationHandler for Shell<P> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(st) = self.live.as_mut() {
            // Android resume: the native window is new, so make a fresh surface for the existing device.
            if st.surface.is_none() {
                match st.gpu.instance.create_surface(st.window.clone()) {
                    Ok(s) => {
                        let size = st.window.inner_size();
                        st.config.width = size.width.max(1);
                        st.config.height = size.height.max(1);
                        s.configure(&st.gpu.device, &st.config);
                        st.surface = Some(s);
                    }
                    Err(e) => self.fail(event_loop, format!("surface: {e}")),
                }
            }
            return;
        }
        if self.window.is_some() {
            return;
        }
        if let Err(e) = self.create_window(event_loop) {
            self.fail(event_loop, e);
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(st) = self.live.as_mut() {
            st.surface = None;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(st) = self.live.as_mut() {
                    st.config.width = size.width.max(1);
                    st.config.height = size.height.max(1);
                    if let Some(s) = &st.surface {
                        s.configure(&st.gpu.device, &st.config);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    // The Android back button arrives as BrowserBack.
                    let action = match code {
                        KeyCode::BrowserBack => Some(Action::Cancel),
                        c => action_for_key(c),
                    };
                    if let Some(action) = action {
                        self.input
                            .set_action(action, event.state == ElementState::Pressed);
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => self.input.set_mouse_look(state == ElementState::Pressed),
            WindowEvent::Touch(t) => {
                if let Some(st) = self.live.as_ref() {
                    let insets = self.platform.insets(&st.window);
                    let layout = Layout::new(
                        (st.config.width, st.config.height),
                        insets,
                        st.window.scale_factor() as f32,
                    );
                    let phase = match t.phase {
                        winit::event::TouchPhase::Started => TouchPhase::Started,
                        winit::event::TouchPhase::Moved => TouchPhase::Moved,
                        _ => TouchPhase::Ended,
                    };
                    self.touch.touch(
                        t.id,
                        phase,
                        (t.location.x as f32, t.location.y as f32),
                        &layout,
                        &mut self.input,
                    );
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

/// Build the wgpu instance for an event loop (shares the display handle, required on Android/Wayland).
pub fn new_instance(event_loop: &EventLoop<()>) -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
        event_loop.owned_display_handle(),
    )))
}

/// Drive `initial` on `event_loop`. On the web this returns immediately (`spawn_app`); elsewhere it runs
/// until the app exits and returns the fatal error, if any.
pub fn run<P: Platform>(
    event_loop: EventLoop<()>,
    platform: P,
    content: Content,
    initial: Box<dyn Game>,
) -> Result<(), String> {
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let now = Instant::now();
    let shell = Shell {
        platform,
        instance: Some(new_instance(&event_loop)),
        content,
        initial: Some(initial),
        stack: GameStack::new(),
        input: InputState::new(),
        touch: TouchControls::new(),
        window: None,
        pending: Rc::new(RefCell::new(None)),
        live: None,
        start: now,
        last: now,
        failed: false,
    };
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn_app(shell);
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut shell = shell;
        event_loop
            .run_app(&mut shell)
            .map_err(|e| format!("event loop: {e}"))?;
        if shell.failed {
            Err("stopped after an error".into())
        } else {
            Ok(())
        }
    }
}
