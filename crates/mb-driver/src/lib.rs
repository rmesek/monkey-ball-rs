//! The driver every platform crate runs on (`mb-desktop`, `mb-web`, and an app outside this repository):
//! the winit event loop, the GPU device and its surface, the frame, and the routing of keys, mouse and
//! touch into `mb_app::input`. What only the platform knows (safe-area insets, gamepads, tilt, the back
//! gesture, where an error is shown) comes in through [`Platform`]; everything from the state stack up is
//! the game's.
//!
//! Life cycle: the GPU device and the renderer are made once, with the first surface, and live until the
//! end. The surface may not: a platform that takes it away (`suspended`, which an Android app gets whenever
//! it leaves the screen) gets a new one at `resumed`, so the states, their textures and the stack survive.
//! In the browser the surface is kept, because a WebGL2 device lives in the canvas' context. A lost device
//! ends the run through [`Platform::on_error`]; saves are written when they happen, so nothing goes with it.
//!
//! Pacing: the surface presents with vsync, and every presented frame asks for the next one with
//! `request_redraw` (`requestAnimationFrame` in the browser). A frame that would look exactly like the last
//! one (a still menu, the pause screen) is not drawn; a native loop then sleeps a frame's time before it
//! looks again, where the browser simply waits for the next animation frame. Nothing runs while the window
//! is hidden, minimised or without a surface, and that time is not game time: the stage being played
//! pauses, held keys and fingers are let go.
//!
//! The browser may not block and has no `std::time::Instant`: there the GPU setup is awaited on the page's
//! event loop and the clock is `web_time`. Elsewhere the setup blocks.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mb_render::Renderer;
use web_time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use mb_app::content::Content;
use mb_app::game::{Game, GameCtx, GameStack};
use mb_app::gpu::{Gpu, pick_surface_format, required_limits};
use mb_app::host::Host;
use mb_app::input::{Action, InputState, action_for_key};
use mb_app::perf::FrameSample;
use mb_app::ui::{Insets, Layout, TouchControls, TouchPhase};

/// The versions the driver is built on, for a platform crate that must name the same types.
pub use {wgpu, winit};

/// Hooks a platform provides to the driver. Every method has a neutral default. All of them are called on
/// the thread that runs the event loop, which is the game's thread.
pub trait Platform: 'static {
    /// Customise the window (the browser attaches its canvas here, the desktop sets a size).
    fn window_attributes(&mut self, attrs: WindowAttributes) -> WindowAttributes {
        attrs
    }
    /// Look at a window event before the driver does. Return true when the platform used it and the driver
    /// should not (the desktop's fullscreen key).
    fn window_event(&mut self, _window: &Window, _event: &WindowEvent) -> bool {
        false
    }
    /// Safe-area insets in surface pixels, asked for every frame (rotation, keyboard and cutouts change them).
    fn insets(&mut self, _window: &Window) -> Insets {
        Insets::default()
    }
    /// Once per frame before the game is updated: input that does not come as a window event (gamepads,
    /// device tilt, a system "back" gesture); what game states asked of the platform ([`Host::drain`]) and
    /// what changed on the platform's side (`host.fullscreen`, `host.info`).
    fn poll_input(&mut self, _input: &mut InputState, _host: &mut Host) {}
    /// Once per frame after the game was updated, with the state now on top (its `nav_depth` for a system
    /// back gesture, its `status` for a title or an automated check, its `name`).
    fn after_update(&mut self, _window: &Window, _top: &dyn Game) {}
    /// True when the platform wants the run to end now (the desktop's "Remove game data").
    fn wants_exit(&mut self) -> bool {
        false
    }
    /// The game went off screen (hidden, minimised, no surface) or came back. Sound is the platform's to
    /// pause.
    fn on_visible(&mut self, _visible: bool) {}
    /// Whether the left mouse button works the touch controls (so the stick and menus can be used with a
    /// mouse), or the mouse is a plain pointer that only menus see.
    fn mouse_as_touch(&mut self) -> bool {
        true
    }
    /// Render resolution relative to the window's physical size, in 0..=1 (1.0 = native). The browser caps
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
    /// True once after the platform changed its mind about [`Platform::render_scale`] (the browser's
    /// automatic quality fallback); the driver then resizes the surface.
    fn render_scale_changed(&mut self) -> bool {
        false
    }
    /// The graphics backends to choose from. In the browser WebGPU is used when it really provides an
    /// adapter and WebGL2 otherwise; a platform can narrow this (to force WebGL2, or GLES on a phone).
    fn backends(&mut self) -> wgpu::Backends {
        wgpu::Backends::all()
    }
    /// Which GPU to ask for where there is a choice. The game is light, and phones and laptops pay for the
    /// fast one in battery and heat; the desktop asks for it all the same.
    fn power_preference(&mut self) -> wgpu::PowerPreference {
        wgpu::PowerPreference::HighPerformance
    }
    /// Called once when the GPU is up, with the adapter description ("name (backend, device type)").
    fn on_ready(&mut self, _gpu: &str) {}
    /// Called after every presented frame of play with its timings (the browser's `?fps=1` overlay, the
    /// desktop's timed run).
    fn on_frame(&mut self, _sample: &FrameSample) {}
    /// What the player does after a fatal error, for the message: "Start the game again to continue".
    fn restart_hint(&self) -> &'static str {
        "Start the game again to continue"
    }
    /// Fatal error: show it to the player (the run then ends).
    fn on_error(&mut self, msg: &str) {
        eprintln!("error: {msg}");
    }
}

type GpuInit = Result<(wgpu::Surface<'static>, Gpu), String>;

struct Live {
    gpu: Gpu,
    renderer: Renderer,
    config: wgpu::SurfaceConfiguration,
    /// `None` while the platform has taken the surface away (an app in the background).
    surface: Option<wgpu::Surface<'static>>,
}

/// Id used when the left mouse button drives the touch controls (real touch ids are small numbers).
const MOUSE_TOUCH_ID: u64 = u64::MAX;
/// Pixels a list scrolls per wheel notch (wheels that report lines).
const WHEEL_LINE: f32 = 60.0;
/// How long a native loop sleeps after a frame that was not drawn, before it looks again.
const IDLE_FRAME: Duration = Duration::from_millis(16);
/// How often a loop looks at a window that is minimised or has no size (not every platform says when that
/// ends).
const HIDDEN_POLL: Duration = Duration::from_millis(100);

struct Shell<P: Platform> {
    platform: P,
    content: Content,
    host: Host,
    initial: Option<Box<dyn Game>>,
    stack: GameStack,
    input: InputState,
    touch: TouchControls,
    window: Option<Arc<Window>>,
    /// Where the (in the browser asynchronous) GPU setup lands.
    pending: Rc<RefCell<Option<GpuInit>>>,
    live: Option<Live>,
    start: Instant,
    last: Instant,
    /// The fatal error that ended the run.
    error: Option<String>,
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
    /// The window is covered (a hidden tab, another app in front).
    occluded: bool,
    /// What [`Platform::on_visible`] was last told.
    visible: bool,
    /// The time since the last frame is not game time (the window was hidden or unfocused): the next frame
    /// advances nothing.
    resync: bool,
    /// Pause the stage being played at the next frame (the player looked away).
    auto_pause: bool,
    /// When a native loop looks again after a frame that was not drawn.
    wake_at: Option<Instant>,
    /// Set by wgpu when the device is lost (GPU reset, driver update, browser reclaiming it).
    device_lost: Arc<Mutex<Option<String>>>,
}

async fn init_gpu(
    backends: wgpu::Backends,
    power_preference: wgpu::PowerPreference,
    window: Arc<Window>,
) -> GpuInit {
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut desc = mb_app::gpu::instance_descriptor(Some(backends));
    // `Instance::new` cannot find out whether the browser really provides a WebGPU adapter (it is
    // synchronous); natively the GL backend wants the window's display.
    #[cfg(target_arch = "wasm32")]
    let instance = wgpu::util::new_instance_with_webgpu_detection(desc).await;
    #[cfg(not(target_arch = "wasm32"))]
    let instance = {
        desc.display = Some(Box::new(window.clone()));
        wgpu::Instance::new(desc)
    };
    let surface = instance
        .create_surface(window)
        .map_err(|e| format!("surface: {e}"))?;
    // Natively a software adapter is better than none (a machine without a GPU driver); a browser has none
    // to offer.
    let fallbacks: &[bool] = if cfg!(target_arch = "wasm32") {
        &[false]
    } else {
        &[false, true]
    };
    let mut last_err = String::from("no adapter");
    for &force_fallback_adapter in fallbacks {
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                force_fallback_adapter,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
        {
            Ok(a) => a,
            Err(e) => {
                last_err = format!("no compatible GPU adapter: {e}");
                continue;
            }
        };
        match adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("mb device"),
                required_features: wgpu::Features::empty(),
                required_limits: required_limits(&adapter),
                ..Default::default()
            })
            .await
        {
            Ok((device, queue)) => {
                let gpu = Gpu {
                    instance,
                    adapter,
                    device,
                    queue,
                };
                return Ok((surface, gpu));
            }
            Err(e) => last_err = format!("could not create a GPU device: {e}"),
        }
    }
    Err(last_err)
}

impl<P: Platform> Shell<P> {
    fn new(platform: P, host: Host, content: Content, initial: Box<dyn Game>) -> Self {
        let now = Instant::now();
        Self {
            platform,
            content,
            host,
            initial: Some(initial),
            stack: GameStack::new(),
            input: InputState::new(),
            touch: TouchControls::new(),
            window: None,
            pending: Rc::new(RefCell::new(None)),
            live: None,
            start: now,
            last: now,
            error: None,
            scale: 1.0,
            cursor: None,
            mouse_touch: false,
            keys_down: Vec::new(),
            wakeups: 0,
            occluded: false,
            visible: true,
            resync: false,
            auto_pause: false,
            wake_at: None,
            device_lost: Arc::new(Mutex::new(None)),
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, msg: String) {
        self.platform.on_error(&msg);
        self.error = Some(msg);
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
        let backends = self.platform.backends();
        let power = self.platform.power_preference();
        let kick = window.clone();
        let setup = async move {
            *slot.borrow_mut() = Some(init_gpu(backends, power, window).await);
        };
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(setup);
        #[cfg(not(target_arch = "wasm32"))]
        pollster::block_on(setup);
        // Natively the GPU is up now; in the browser frames only check whether it has arrived.
        self.finish_init()?;
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

    fn layout(&mut self, window: &Window, size: (u32, u32)) -> Layout {
        let insets = self.platform.insets(window);
        Layout::new(size, insets, self.platform.ui_scale(window, self.scale))
    }

    /// Feed one pointer sample (window physical pixels) to the touch controls.
    fn pointer(&mut self, id: u64, phase: TouchPhase, pos: (f32, f32)) {
        let (Some(window), Some(size)) = (
            self.window.clone(),
            (self.live.as_ref()).map(|l| (l.config.width, l.config.height)),
        ) else {
            return;
        };
        let layout = self.layout(&window, size);
        let pos = self.platform.pointer_position(&window, pos, self.scale);
        self.touch.touch(id, phase, pos, &layout, &mut self.input);
    }

    /// The mouse as a plain pointer, in surface pixels.
    fn mouse_pointer(&mut self, pos: Option<(f32, f32)>) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let pos = pos.map(|p| self.platform.pointer_position(&window, p, self.scale));
        self.input.set_pointer(pos);
    }

    /// The player cannot see or steer the game any more (hidden, lost focus): pause the stage at the next
    /// frame and do not count the time away as game time.
    fn look_away(&mut self) {
        self.auto_pause = true;
        self.resync = true;
    }

    /// Nothing can be seen: covered, minimised, no size, or no surface.
    fn hidden(&self) -> bool {
        self.occluded
            || self.live.as_ref().is_some_and(|l| l.surface.is_none())
            || self.window.as_ref().is_some_and(|w| {
                let size = w.inner_size();
                size.width == 0 || size.height == 0 || w.is_minimized() == Some(true)
            })
    }

    /// Tell the platform when the game goes off screen or comes back, and act on it.
    fn update_visibility(&mut self) {
        let visible = !self.hidden();
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        self.platform.on_visible(visible);
        if visible {
            // Back on screen: redraw even if nothing changed, and restart the frame chain.
            self.resync = true;
            if let Some(live) = self.live.as_mut() {
                live.renderer.invalidate();
            }
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        } else {
            self.release_held();
            self.look_away();
        }
    }

    /// Resize the surface to the window and the current render scale.
    fn resize_surface(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let (width, height) = self.surface_size(&window);
        if let Some(live) = self.live.as_mut() {
            if (live.config.width, live.config.height) != (width, height) {
                live.config.width = width;
                live.config.height = height;
                if let Some(surface) = &live.surface {
                    surface.configure(&live.gpu.device, &live.config);
                }
            }
            live.renderer.invalidate();
        }
    }

    /// A new surface for the window, on the device that is already there (back from the background).
    fn make_surface(&mut self) -> Result<(), String> {
        let Some(window) = self.window.clone() else {
            return Ok(());
        };
        if self.live.as_ref().is_none_or(|l| l.surface.is_some()) {
            return Ok(());
        }
        let (width, height) = self.surface_size(&window);
        let live = self.live.as_mut().ok_or("no GPU")?;
        let surface = (live.gpu.instance)
            .create_surface(window)
            .map_err(|e| format!("surface: {e}"))?;
        let caps = surface.get_capabilities(&live.gpu.adapter);
        if !caps.formats.contains(&live.config.format) {
            return Err(format!(
                "the new surface cannot show {:?}",
                live.config.format
            ));
        }
        live.config.width = width;
        live.config.height = height;
        surface.configure(&live.gpu.device, &live.config);
        live.surface = Some(surface);
        live.renderer.invalidate();
        Ok(())
    }

    /// Let go of everything held through events that will not get their "up" (focus moved elsewhere).
    fn release_held(&mut self) {
        for action in std::mem::take(&mut self.keys_down) {
            self.input.set_action(action, false);
        }
        // Fingers, the mouse-as-touch and the mouse button: their ends will not arrive either. Cancelled,
        // not ended, so nothing under them is activated.
        self.mouse_touch = false;
        self.touch.cancel_all(&mut self.input);
        self.input.cancel_pointer();
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
        // Vsync: one frame per display refresh, no tearing, no busy loop. Every surface supports it.
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&gpu.device, &config);
        let lost = self.device_lost.clone();
        gpu.device.set_device_lost_callback(move |reason, message| {
            // `Destroyed` is our own teardown.
            if reason != wgpu::DeviceLostReason::Destroyed
                && let Ok(mut slot) = lost.lock()
            {
                *slot = Some(message);
            }
        });
        self.platform.on_ready(&gpu.describe());
        let renderer = Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
        self.live = Some(Live {
            gpu,
            renderer,
            config,
            surface: Some(surface),
        });
        if let Some(game) = self.initial.take() {
            let size = (width, height);
            let layout = self.layout(&window, size);
            let live = self.live.as_mut().unwrap();
            let mut ctx = GameCtx {
                dt: 0.0,
                time: 0.0,
                input: &self.input,
                renderer: &mut live.renderer,
                content: &self.content,
                host: &mut self.host,
                size,
                layout,
            };
            self.stack.push(game, &mut ctx)?;
        }
        self.last = Instant::now();
        Ok(())
    }

    /// One frame: update the game and draw it. Returns whether a picture was presented.
    fn frame(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.live.is_none() {
            if let Err(e) = self.finish_init() {
                self.fail(event_loop, e);
            }
            return false;
        }
        let lost = self.device_lost.lock().ok().and_then(|mut l| l.take());
        if let Some(message) = lost {
            // Everything on the GPU is gone with the device; say so instead of showing a frozen picture.
            let reason = if message.is_empty() {
                "no reason given"
            } else {
                &message
            };
            let hint = self.platform.restart_hint();
            self.fail(
                event_loop,
                format!("The graphics device was lost ({reason}). {hint}; your progress is saved."),
            );
            return false;
        }
        self.update_visibility();
        let Some(window) = self.window.clone() else {
            return false;
        };
        if !self.visible {
            return false;
        }
        if self.platform.render_scale_changed() {
            self.resize_surface();
        }
        self.platform.poll_input(&mut self.input, &mut self.host);
        if self.platform.wants_exit() {
            event_loop.exit();
            return false;
        }
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
        let live = self.live.as_ref().unwrap();
        let size = (live.config.width, live.config.height);
        let layout = self.layout(&window, size);
        let live = self.live.as_mut().unwrap();
        let mut ctx = GameCtx {
            dt,
            time: (now - self.start).as_secs_f64(),
            input: &self.input,
            renderer: &mut live.renderer,
            content: &self.content,
            host: &mut self.host,
            size,
            layout,
        };
        match self.stack.update(&mut ctx) {
            Ok(true) => {}
            Ok(false) => {
                event_loop.exit();
                return false;
            }
            Err(e) => {
                self.fail(event_loop, e);
                return false;
            }
        }
        self.input.end_frame();
        let Some(top) = self.stack.top() else {
            return false;
        };
        self.platform.after_update(&window, top);
        let camera = top.camera();
        let animating = top.play_view().is_some_and(|v| !v.paused);
        let shadow = top.play_view().is_some_and(|v| v.shadow);
        let ticks = match (tick_before, top.play_view().map(|v| v.sim.tick)) {
            (Some(a), Some(b)) => b.saturating_sub(a),
            _ => 0,
        };
        let updated = Instant::now();
        let live = self.live.as_mut().unwrap();
        let Some(surface) = &live.surface else {
            return false;
        };
        // Uploads what changed; a frame identical to the one on screen is not drawn or presented again
        // (still menus and the pause screen then cost no GPU time at all).
        if !live.renderer.prepare(size, &camera) {
            return false;
        }
        use wgpu::CurrentSurfaceTexture as C;
        let frame = match surface.get_current_texture() {
            C::Success(f) | C::Suboptimal(f) => f,
            C::Outdated | C::Lost => {
                surface.configure(&live.gpu.device, &live.config);
                live.renderer.invalidate();
                return false;
            }
            C::Timeout | C::Occluded | C::Validation => {
                live.renderer.invalidate();
                return false;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        live.renderer.draw(&view);
        window.pre_present_notify();
        live.gpu.queue.present(frame);
        if resync || !animating {
            // Not a frame of play: keep it out of the pacing statistics.
            self.wakeups = 0;
            return true;
        }
        let stats = live.renderer.stats;
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
        true
    }

    /// Ask for the next frame: at once after a presented one (the present waits for the display) and
    /// always in the browser (the animation frame does); after a frame that was not drawn, natively, a frame's
    /// time later. A covered window or one without a surface gets none until an event brings it back.
    fn schedule(&mut self, presented: bool) {
        self.wake_at = None;
        let Some(window) = &self.window else {
            return;
        };
        if self.error.is_some()
            || self.occluded
            || (self.live.as_ref()).is_some_and(|l| l.surface.is_none())
        {
            return;
        }
        if !self.visible {
            self.wake_at = Some(Instant::now() + HIDDEN_POLL);
        } else if presented || cfg!(target_arch = "wasm32") {
            window.request_redraw();
        } else {
            self.wake_at = Some(Instant::now() + IDLE_FRAME);
        }
    }
}

impl<P: Platform> ApplicationHandler for Shell<P> {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && self.wake_at.take().is_some()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let made = if self.window.is_none() {
            self.create_window(event_loop)
        } else {
            self.make_surface()
        };
        if let Err(e) = made {
            self.fail(event_loop, e);
            return;
        }
        self.update_visibility();
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.release_held();
        // The surface goes with the native window (Android destroys it); the device and all that is on it
        // stay. The browser keeps its canvas, and its WebGL2 device needs the canvas' context.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(live) = self.live.as_mut() {
            live.surface = None;
        }
        self.update_visibility();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(window) = self.window.clone()
            && self.platform.window_event(&window, &event)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.resize_surface();
                self.update_visibility();
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.update_visibility();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    // A hardware or keyboard "back" key (a phone's, the browser's) arrives as BrowserBack.
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
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let down = state == ElementState::Pressed;
                if !self.platform.mouse_as_touch() {
                    self.input.set_pointer_down(down);
                } else if let (Some(pos), true) = (self.cursor, down != self.mouse_touch) {
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
                if !self.platform.mouse_as_touch() {
                    self.mouse_pointer(Some(pos));
                } else if self.mouse_touch {
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
                if !self.platform.mouse_as_touch() {
                    self.mouse_pointer(None);
                } else if std::mem::take(&mut self.mouse_touch) {
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
                let presented = self.frame(event_loop);
                self.schedule(presented);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.wakeups += 1;
        event_loop.set_control_flow(match self.wake_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Leave every state, so each releases what it holds on the GPU before the device goes.
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(size) = (self.live.as_ref()).map(|l| (l.config.width, l.config.height)) else {
            return;
        };
        let layout = self.layout(&window, size);
        let live = self.live.as_mut().unwrap();
        let mut ctx = GameCtx {
            dt: 0.0,
            time: 0.0,
            input: &self.input,
            renderer: &mut live.renderer,
            content: &self.content,
            host: &mut self.host,
            size,
            layout,
        };
        self.stack.clear(&mut ctx);
    }
}

/// Run `initial` on `content` until the player or the platform ends it, then hand the platform back.
/// `event_loop` is the platform's (an Android app builds it with its `AndroidApp`); `host` says what the
/// platform offers before the first state starts. A fatal error has been shown through
/// [`Platform::on_error`] and is returned as well.
#[cfg(not(target_arch = "wasm32"))]
pub fn run<P: Platform>(
    event_loop: EventLoop<()>,
    platform: P,
    host: Host,
    content: Content,
    initial: Box<dyn Game>,
) -> Result<P, String> {
    let mut shell = Shell::new(platform, host, content, initial);
    event_loop
        .run_app(&mut shell)
        .map_err(|e| format!("event loop: {e}"))?;
    match shell.error.take() {
        Some(e) => Err(e),
        None => Ok(shell.platform),
    }
}

/// Start `initial` on `content` in the browser. Returns at once: the page owns the loop from here, and later
/// fatal errors reach it through [`Platform::on_error`].
#[cfg(target_arch = "wasm32")]
pub fn spawn<P: Platform>(
    event_loop: EventLoop<()>,
    platform: P,
    host: Host,
    content: Content,
    initial: Box<dyn Game>,
) {
    use winit::platform::web::EventLoopExtWebSys;
    event_loop.spawn_app(Shell::new(platform, host, content, initial));
}
