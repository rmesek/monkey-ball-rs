//! The browser driver (wasm32 only; `lib.rs` starts it and supplies the [`Platform`] hooks).
//!
//! The desktop driver (`mb-desktop`'s `app.rs`) blocks on GPU setup and uses `std::time`; neither works in the browser (no
//! blocking, `Instant::now` panics). This driver therefore (a) runs GPU setup through
//! [`Platform::run_local`], which spawns it on the JS event loop, and (b) uses `web_time` clocks. What the
//! page knows and the game does not (safe-area insets, gamepads, tilt, the back gesture, error display) comes
//! in through the [`Platform`] trait.
//!
//! Why there are two drivers: the browser paces frames with `requestAnimationFrame`, hides the page as a
//! whole and has no blocking calls; the desktop paces with vsync, has to detect a minimised window itself
//! and has native gamepads, a real mouse pointer and a fullscreen key. Everything from the state stack up
//! is shared.
//!
//! Frame pacing: the event loop sleeps (`ControlFlow::Wait`) and every frame asks for the next one with
//! `request_redraw`, which is `requestAnimationFrame` in the browser and the compositor's frame callback
//! elsewhere. Nothing runs between frames, and nothing at all while the window is hidden. (Polling instead
//! spins the browser's event loop hundreds of times per frame, which costs battery and delays frames.)
//! A frame that would look exactly like the last one (a still menu, the pause screen) is not drawn again.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mb_render::Renderer;
use web_time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use mb_app::content::Content;
use mb_app::game::{Game, GameCtx, GameStack};
use mb_app::gpu::{pick_surface_format, required_limits, Gpu};
use mb_app::input::{action_for_key, Action, InputState};
use mb_app::perf::FrameSample;
use mb_app::ui::{Insets, Layout, TouchControls, TouchPhase};

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
    /// Per-frame polling of non-event input: gamepads, device tilt, a system "back" gesture. This is also
    /// where a shell handles what game states asked of it (`mb_app::host::drain`).
    fn poll_input(&mut self, _input: &mut InputState) {}
    /// Called every frame with the top state's [`Game::nav_depth`]: while it is not zero a system "back"
    /// should go back inside the game instead of leaving it.
    fn on_nav_depth(&mut self, _depth: u32) {}
    /// Whether the shell wants [`Platform::on_status`] this frame (building the text costs an allocation,
    /// so it is only done on request: automated checks).
    fn wants_status(&mut self) -> bool {
        false
    }
    /// The top state's status line (`Game::status`), when asked for.
    fn on_status(&mut self, _status: String) {}
    /// Run a future to completion on the page's event loop (`wasm_bindgen_futures::spawn_local`).
    fn run_local(&mut self, fut: Pin<Box<dyn Future<Output = ()>>>);
    /// Render resolution relative to the window's physical size, in 0..=1 (1.0 = native). The web shell caps
    /// very dense displays here; pointer positions and the layout scale follow the reduced size.
    fn render_scale(&mut self, _window: &Window) -> f32 {
        1.0
    }
    /// Surface pixels per logical (CSS / density-independent) pixel, which scales the HUD and touch layout.
    /// `render_scale` is the value [`Platform::render_scale`] last returned.
    fn ui_scale(&mut self, window: &Window, render_scale: f32) -> f32 {
        window.scale_factor() as f32 * render_scale
    }
    /// Map a pointer position from winit (window physical pixels) to surface pixels. `render_scale` is the
    /// value [`Platform::render_scale`] last returned, which is all it takes where winit's physical pixels
    /// are the window's real pixels.
    fn pointer_position(
        &mut self,
        _window: &Window,
        pos: (f32, f32),
        render_scale: f32,
    ) -> (f32, f32) {
        (pos.0 * render_scale, pos.1 * render_scale)
    }
    /// Browser only: the backends to choose from. WebGPU is used when the browser really provides an adapter,
    /// WebGL2 otherwise; a shell can narrow this (e.g. to force WebGL2 on a machine with a bad WebGPU driver).
    fn web_backends(&mut self) -> wgpu::Backends {
        wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL
    }
    /// Which GPU to ask for where there is a choice. The browser shell prefers the low-power one: the game is
    /// light, and phones and laptops pay for the fast one in battery and heat.
    fn power_preference(&mut self) -> wgpu::PowerPreference {
        wgpu::PowerPreference::HighPerformance
    }
    /// True once after the platform changed its mind about [`Platform::render_scale`] (automatic quality
    /// fallback); the driver then resizes the surface.
    fn render_scale_changed(&mut self) -> bool {
        false
    }
    /// Called once when the GPU is up, with the adapter description ("name (backend, device type)").
    fn on_ready(&mut self, _gpu: &str) {}
    /// Called after every presented frame with its timings (the web shell's `?fps=1` overlay).
    fn on_frame(&mut self, _sample: &FrameSample) {}
    /// Fatal error: show it to the user (the driver then stops).
    fn on_error(&mut self, msg: &str) {
        eprintln!("error: {msg}");
    }
}

type GpuInit = Result<(wgpu::Surface<'static>, Gpu), String>;

struct Live {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
}

/// Id used when the left mouse button drives the touch controls (real touch ids are small numbers).
const MOUSE_TOUCH_ID: u64 = u64::MAX;
/// Pixels a list scrolls per wheel notch (wheels that report lines).
const WHEEL_LINE: f32 = 60.0;

struct Shell<P: Platform> {
    platform: P,
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
    /// Current [`Platform::render_scale`]: surface pixels per window physical pixel.
    scale: f32,
    /// Last cursor position in window physical pixels.
    cursor: Option<(f32, f32)>,
    /// The left mouse button is held and acts as a touch.
    mouse_touch: bool,
    /// Actions currently held from the keyboard, released together when the window loses focus.
    keys_down: Vec<Action>,
    /// Event-loop iterations since the last frame (see [`FrameSample::wakeups`]).
    wakeups: u32,
    /// The window is not on screen (hidden tab, other app in front): no frames are produced.
    hidden: bool,
    /// The time since the last frame is not game time (the window was hidden or unfocused): the next frame
    /// advances nothing.
    resync: bool,
    /// Pause the stage being played at the next frame (the player looked away).
    auto_pause: bool,
    /// Set by wgpu when the device is lost (GPU reset, driver update, browser reclaiming it).
    device_lost: Arc<Mutex<Option<String>>>,
}

/// Instance for the browser: of `backends`, WebGPU when the browser really provides an adapter, otherwise
/// WebGL2. `Instance::new` cannot find that out (it is synchronous), so the web path creates the instance here.
async fn web_instance(backends: wgpu::Backends) -> wgpu::Instance {
    let desc = mb_app::gpu::instance_descriptor(Some(backends));
    wgpu::util::new_instance_with_webgpu_detection(desc).await
}

async fn init_gpu(
    web_backends: wgpu::Backends,
    power_preference: wgpu::PowerPreference,
    window: Arc<Window>,
) -> GpuInit {
    let instance = web_instance(web_backends).await;
    let surface = instance
        .create_surface(window)
        .map_err(|e| format!("surface: {e}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference,
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("no compatible GPU adapter: {e}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("mb device"),
            required_features: wgpu::Features::empty(),
            required_limits: required_limits(&adapter),
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
        self.window = Some(window.clone());
        let slot = self.pending.clone();
        let web_backends = self.platform.web_backends();
        let power = self.platform.power_preference();
        let kick = window.clone();
        self.platform.run_local(Box::pin(async move {
            *slot.borrow_mut() = Some(init_gpu(web_backends, power, window).await);
        }));
        // Start the frame chain; frames before the GPU is up only check whether it has arrived.
        kick.request_redraw();
        Ok(())
    }

    /// Surface size for the window: its physical size times the platform's render scale.
    fn surface_size(&mut self, window: &Window) -> (u32, u32) {
        let size = window.inner_size();
        self.scale = self.platform.render_scale(window).clamp(0.1, 1.0);
        let scaled = |v: u32| ((v as f32 * self.scale).round() as u32).max(1);
        (scaled(size.width), scaled(size.height))
    }

    /// Feed one pointer sample (window physical pixels) to the touch controls.
    fn pointer(&mut self, id: u64, phase: TouchPhase, pos: (f32, f32)) {
        let Some(st) = self.live.as_ref() else {
            return;
        };
        let insets = self.platform.insets(&st.window);
        let layout = Layout::new(
            (st.config.width, st.config.height),
            insets,
            self.platform.ui_scale(&st.window, self.scale),
        );
        let pos = self.platform.pointer_position(&st.window, pos, self.scale);
        self.touch.touch(id, phase, pos, &layout, &mut self.input);
    }

    /// The player cannot see or steer the game any more (hidden tab, lost focus): pause the stage at the
    /// next frame and do not count the time away as game time.
    fn look_away(&mut self) {
        self.auto_pause = true;
        self.resync = true;
    }

    /// Resize the surface to the window and the current render scale.
    fn resize_surface(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let (width, height) = self.surface_size(&window);
        if let Some(st) = self.live.as_mut() {
            if (st.config.width, st.config.height) != (width, height) {
                st.config.width = width;
                st.config.height = height;
                st.surface.configure(&st.gpu.device, &st.config);
            }
            st.renderer.invalidate();
        }
    }

    /// Let go of everything held through events that will not get their "up" (focus moved elsewhere).
    fn release_held(&mut self) {
        for action in std::mem::take(&mut self.keys_down) {
            self.input.set_action(action, false);
        }
        // Fingers and the mouse-as-touch: their ends will not arrive either. Cancelled, not ended, so
        // nothing under them is activated.
        self.mouse_touch = false;
        self.touch.cancel_all(&mut self.input);
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
        let (width, height) = self.surface_size(&window);
        let mut config = surface
            .get_default_config(&gpu.adapter, width, height)
            .ok_or("surface not supported by adapter")?;
        config.format = format;
        surface.configure(&gpu.device, &config);
        let lost = self.device_lost.clone();
        gpu.device.set_device_lost_callback(move |reason, message| {
            // `Destroyed` is our own teardown.
            if reason != wgpu::DeviceLostReason::Destroyed {
                if let Ok(mut slot) = lost.lock() {
                    *slot = Some(message);
                }
            }
        });
        self.platform.on_ready(&gpu.describe());
        let renderer = Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
        self.live = Some(Live {
            window,
            surface,
            config,
            gpu,
            renderer,
        });
        if let Some(game) = self.initial.take() {
            let st = self.live.as_mut().unwrap();
            let size = (st.config.width, st.config.height);
            let insets = self.platform.insets(&st.window);
            let mut ctx = GameCtx {
                dt: 0.0,
                time: 0.0,
                input: &self.input,
                renderer: &mut st.renderer,
                content: &self.content,
                size,
                layout: Layout::new(size, insets, self.platform.ui_scale(&st.window, self.scale)),
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
        let lost = self.device_lost.lock().ok().and_then(|mut l| l.take());
        if let Some(message) = lost {
            // Everything on the GPU is gone with the device; say so instead of showing a frozen picture.
            self.fail(
                event_loop,
                format!(
                    "The graphics device was lost ({}). Reload the page to continue; your progress is saved.",
                    if message.is_empty() {
                        "no reason given"
                    } else {
                        &message
                    }
                ),
            );
            return;
        }
        if self.platform.render_scale_changed() {
            self.resize_surface();
        }
        let st = self.live.as_mut().unwrap();
        self.platform.poll_input(&mut self.input);
        if std::mem::take(&mut self.auto_pause) {
            let playing = (self.stack.top())
                .and_then(|g| g.play_view())
                .is_some_and(|v| !v.paused);
            if playing {
                self.input.set_action(Action::Pause, true);
                self.input.set_action(Action::Pause, false);
            }
        }
        let now = Instant::now();
        let resync = std::mem::take(&mut self.resync);
        let interval = (now - self.last).as_secs_f32();
        // Time spent hidden or unfocused is not game time.
        let dt = if resync { 0.0 } else { interval.min(0.1) };
        self.last = now;
        let tick_before = self
            .stack
            .top()
            .and_then(|g| g.play_view())
            .map(|v| v.sim.tick);
        let size = (st.config.width, st.config.height);
        let insets = self.platform.insets(&st.window);
        let mut ctx = GameCtx {
            dt,
            time: (now - self.start).as_secs_f64(),
            input: &self.input,
            renderer: &mut st.renderer,
            content: &self.content,
            size,
            layout: Layout::new(size, insets, self.platform.ui_scale(&st.window, self.scale)),
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
        self.platform.on_nav_depth(top.nav_depth());
        if self.platform.wants_status() {
            self.platform.on_status(top.status());
        }
        let camera = top.camera();
        let animating = top.play_view().is_some_and(|v| !v.paused);
        let shadow = top.play_view().is_some_and(|v| v.shadow);
        let ticks = match (tick_before, top.play_view().map(|v| v.sim.tick)) {
            (Some(a), Some(b)) => b.saturating_sub(a),
            _ => 0,
        };
        let updated = Instant::now();
        let st = self.live.as_mut().unwrap();
        let surface = &st.surface;
        // Uploads what changed; a frame identical to the one on screen is not drawn or presented again
        // (still menus and the pause screen then cost no GPU time at all).
        if st.renderer.prepare(size, &camera) {
            use wgpu::CurrentSurfaceTexture as C;
            let frame = match surface.get_current_texture() {
                C::Success(f) | C::Suboptimal(f) => f,
                C::Outdated | C::Lost => {
                    surface.configure(&st.gpu.device, &st.config);
                    st.renderer.invalidate();
                    return;
                }
                C::Timeout | C::Occluded | C::Validation => {
                    st.renderer.invalidate();
                    return;
                }
            };
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            st.renderer.draw(&view);
            st.window.pre_present_notify();
            st.gpu.queue.present(frame);
        }
        if resync || !animating {
            // Not a frame of play: keep it out of the pacing statistics.
            self.wakeups = 0;
            return;
        }
        let stats = st.renderer.stats;
        let sample = FrameSample {
            interval_ms: interval * 1000.0,
            update_ms: (updated - now).as_secs_f32() * 1000.0,
            render_ms: (Instant::now() - updated).as_secs_f32() * 1000.0,
            ticks,
            draws: stats.draws as u32,
            state_changes: stats.state_changes as u32,
            upload_bytes: stats.upload_bytes as u32,
            wakeups: std::mem::take(&mut self.wakeups),
            mirror_draws: stats.mirror_draws as u32,
            instances: stats.instances as u32,
            quads: stats.quads as u32,
            shadow,
        };
        self.platform.on_frame(&sample);
    }
}

impl<P: Platform> ApplicationHandler for Shell<P> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() || self.window.is_some() {
            return;
        }
        if let Err(e) = self.create_window(event_loop) {
            self.fail(event_loop, e);
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.release_held();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.resize_surface()
            }
            WindowEvent::Occluded(hidden) => {
                self.hidden = hidden;
                if hidden {
                    self.release_held();
                    self.look_away();
                } else if let Some(w) = &self.window {
                    // Back on screen: redraw even if nothing changed, and restart the frame chain.
                    self.resync = true;
                    if let Some(st) = self.live.as_mut() {
                        st.renderer.invalidate();
                    }
                    w.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    // A hardware or keyboard "back" key arrives as BrowserBack.
                    let action = match code {
                        KeyCode::BrowserBack => Some(Action::Cancel),
                        c => action_for_key(c),
                    };
                    if let Some(action) = action {
                        let down = event.state == ElementState::Pressed;
                        self.keys_down.retain(|a| *a != action);
                        if down {
                            self.keys_down.push(action);
                        }
                        self.input.set_action(action, down);
                    }
                }
            }
            WindowEvent::Focused(false) => {
                self.release_held();
                self.look_away();
            }
            // The left button works the touch controls, so menus and the stick are usable with a mouse.
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let down = state == ElementState::Pressed;
                if let (Some(pos), true) = (self.cursor, down != self.mouse_touch) {
                    self.mouse_touch = down;
                    let phase = if down {
                        TouchPhase::Started
                    } else {
                        TouchPhase::Ended
                    };
                    self.pointer(MOUSE_TOUCH_ID, phase, pos);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pos = (position.x as f32, position.y as f32);
                if self.mouse_touch {
                    self.pointer(MOUSE_TOUCH_ID, TouchPhase::Moved, pos);
                }
                self.cursor = Some(pos);
            }
            // The wheel scrolls lists (physical pixels, like the pointer).
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.add_scroll(match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * WHEEL_LINE * self.scale,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 * self.scale,
                });
            }
            WindowEvent::CursorLeft { .. } => {
                if std::mem::take(&mut self.mouse_touch) {
                    let pos = self.cursor.unwrap_or_default();
                    self.pointer(MOUSE_TOUCH_ID, TouchPhase::Ended, pos);
                }
                self.cursor = None;
            }
            WindowEvent::Touch(t) => {
                let phase = match t.phase {
                    winit::event::TouchPhase::Started => Some(TouchPhase::Started),
                    winit::event::TouchPhase::Moved => Some(TouchPhase::Moved),
                    winit::event::TouchPhase::Ended => Some(TouchPhase::Ended),
                    // The system took the touch (a gesture, a palm): let go, activate nothing.
                    winit::event::TouchPhase::Cancelled => None,
                };
                match phase {
                    Some(phase) => {
                        self.pointer(t.id, phase, (t.location.x as f32, t.location.y as f32))
                    }
                    None => self.touch.cancel(t.id, &mut self.input),
                }
            }
            WindowEvent::RedrawRequested => {
                self.frame(event_loop);
                // The next frame, at the display's pace. A hidden window gets none until it is shown again.
                if !self.hidden && !self.failed {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        self.wakeups += 1;
    }
}

/// Start `initial` on `event_loop`. Returns at once: the browser owns the loop from here (`spawn_app`), and
/// later fatal errors reach the page through [`Platform::on_error`].
pub fn run<P: Platform>(
    event_loop: EventLoop<()>,
    platform: P,
    content: Content,
    initial: Box<dyn Game>,
) -> Result<(), String> {
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let now = Instant::now();
    let shell = Shell {
        platform,
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
        scale: 1.0,
        cursor: None,
        mouse_touch: false,
        keys_down: Vec::new(),
        wakeups: 0,
        hidden: false,
        resync: false,
        auto_pause: false,
        device_lost: Arc::new(Mutex::new(None)),
    };
    use winit::platform::web::EventLoopExtWebSys;
    event_loop.spawn_app(shell);
    Ok(())
}
