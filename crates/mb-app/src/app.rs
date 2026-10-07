//! Desktop shell: winit event loop, wgpu surface, and the per-frame update/render driver.
//! Everything game-specific lives in [`crate::game::Game`] states; this file is only platform glue
//! (Android and web will get their own shells that feed the same `InputState` and call the same loop).

use std::sync::Arc;
use std::time::Instant;

use mb_render::Renderer;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

use crate::content::Content;
use crate::game::{Game, GameCtx, GameStack};
use crate::gpu::{pick_surface_format, request_gpu, Gpu};
use crate::input::{action_for_key, InputState};
use crate::ui::{Insets, Layout, TouchControls, TouchPhase};

struct WindowState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
}

struct App {
    instance: Option<wgpu::Instance>,
    content: Content,
    initial: Option<Box<dyn Game>>,
    stack: GameStack,
    input: InputState,
    state: Option<WindowState>,
    start: Instant,
    last: Instant,
    last_title: Instant,
    frames: u32,
    error: Option<String>,
    touch: TouchControls,
    /// Safe-area insets in pixels; desktop has none, mobile shells set them from the system.
    insets: Insets,
}

impl App {
    fn init_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attrs = Window::default_attributes()
            .with_title("Monkey Ball")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("window: {e}"))?,
        );
        let instance = self.instance.take().ok_or("instance already consumed")?;
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        let gpu = request_gpu(instance, Some(&surface), false)?;
        eprintln!("adapter: {}", gpu.describe());
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = pick_surface_format(&caps.formats);
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by adapter")?;
        config.format = format;
        surface.configure(&gpu.device, &config);
        let renderer = Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
        self.state = Some(WindowState {
            window,
            surface,
            config,
            gpu,
            renderer,
        });
        if let Some(game) = self.initial.take() {
            let st = self.state.as_mut().unwrap();
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
        Ok(())
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(st) = self.state.as_mut() else {
            return;
        };
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
                self.error = Some(e);
                event_loop.exit();
                return;
            }
        }
        self.input.end_frame();
        let Some(top) = self.stack.top() else {
            return;
        };
        let camera = top.camera();

        use wgpu::CurrentSurfaceTexture as C;
        let frame = match st.surface.get_current_texture() {
            C::Success(f) | C::Suboptimal(f) => f,
            C::Outdated | C::Lost => {
                st.surface.configure(&st.gpu.device, &st.config);
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

        self.frames += 1;
        if now - self.last_title >= std::time::Duration::from_secs(1) {
            let fps = self.frames as f32 / (now - self.last_title).as_secs_f32();
            st.window
                .set_title(&format!("Monkey Ball | {fps:.0} fps | {}", top.status()));
            self.frames = 0;
            self.last_title = now;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        if let Err(e) = self.init_window(event_loop) {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(st) = self.state.as_mut() {
                    st.config.width = size.width.max(1);
                    st.config.height = size.height.max(1);
                    st.surface.configure(&st.gpu.device, &st.config);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(action) = action_for_key(code) {
                        self.input
                            .set_action(action, event.state == ElementState::Pressed);
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => {
                self.input.set_mouse_look(state == ElementState::Pressed);
            }
            WindowEvent::Touch(t) => {
                if let Some(st) = self.state.as_ref() {
                    let layout = Layout::new(
                        (st.config.width, st.config.height),
                        self.insets,
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

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.add_mouse_delta(delta.0 as f32, delta.1 as f32);
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(st) = &self.state {
            st.window.request_redraw();
        }
    }
}

/// Open a window and run `initial` until the user quits.
pub fn run_windowed(content: Content, initial: Box<dyn Game>) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
        Box::new(event_loop.owned_display_handle()),
    ));
    let now = Instant::now();
    let mut app = App {
        instance: Some(instance),
        content,
        initial: Some(initial),
        stack: GameStack::new(),
        input: InputState::new(),
        state: None,
        start: now,
        last: now,
        last_title: now,
        frames: 0,
        error: None,
        touch: TouchControls::new(),
        insets: Insets::default(),
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop: {e}"))?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
