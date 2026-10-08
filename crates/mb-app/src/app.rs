//! Desktop driver: winit event loop, wgpu surface and the per-frame update / render loop, plus the
//! desktop-only devices (mouse pointer, native gamepads) and window niceties (fullscreen, fps in the title).
//! Everything game-specific lives in [`crate::game::Game`] states; the browser has its own driver (`shell.rs`,
//! which explains why there are two) and feeds the same `InputState`.
//!
//! Pacing: the surface presents with vsync (`Fifo`), so the loop runs once per display refresh without
//! spinning; game states advance their simulation in fixed 60 Hz ticks from `ctx.dt`. While the window is
//! minimized or fully covered nothing is rendered and the loop sleeps.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mb_render::Renderer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::content::Content;
use crate::game::{Game, GameCtx, GameStack};
use crate::gpu::{pick_surface_format, request_gpu, Gpu};
use crate::input::{action_for_key, Action, Axis, InputState};
use crate::ui::{Insets, Layout, TouchControls, TouchPhase};

/// Pixels scrolled per mouse-wheel notch.
const WHEEL_LINE: f32 = 60.0;
/// Stick travel ignored around the centre.
const PAD_DEADZONE: f32 = 0.15;

#[derive(Clone, Debug, Default)]
pub struct WindowOptions {
    /// Close the window after this many seconds and measure frame pacing meanwhile (the developer flag
    /// `--exit-after` of a `dev` build; the kill test uses it).
    pub exit_after: Option<f32>,
    pub fullscreen: bool,
    /// The game's audio system, to silence it while the window is hidden. The game states hold clones.
    pub audio: Option<crate::audio::Audio>,
}

/// Frame pacing measured over a session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStats {
    pub frames: u64,
    pub seconds: f64,
    pub worst_frame_ms: f64,
    /// 99th percentile frame time.
    pub p99_frame_ms: f64,
    /// Simulation ticks run by the stage that was open at exit, and how long it had been open.
    pub stage_ticks: Option<(u32, f64)>,
    pub adapter: String,
}

impl RunStats {
    pub fn average_fps(&self) -> f64 {
        if self.seconds > 0.0 {
            self.frames as f64 / self.seconds
        } else {
            0.0
        }
    }

    pub fn average_frame_ms(&self) -> f64 {
        if self.frames > 0 {
            self.seconds * 1000.0 / self.frames as f64
        } else {
            0.0
        }
    }

    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} frames in {:.2} s: {:.1} fps, frame avg {:.2} ms, p99 {:.2} ms, worst {:.2} ms ({})",
            self.frames,
            self.seconds,
            self.average_fps(),
            self.average_frame_ms(),
            self.p99_frame_ms,
            self.worst_frame_ms,
            self.adapter
        );
        if let Some((ticks, seconds)) = self.stage_ticks {
            s.push_str(&format!(
                "\nstage: {ticks} sim ticks in {seconds:.2} s, expected {:.0} at 60 Hz",
                seconds * 60.0
            ));
        }
        s
    }
}

struct WindowState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
}

/// Native gamepads through gilrs: left stick steers, A / B / Start confirm, cancel and pause, and the
/// d-pad walks menus.
struct Gamepads {
    gilrs: gilrs::Gilrs,
    stick: (f32, f32),
}

impl Gamepads {
    fn new() -> Option<Self> {
        match gilrs::Gilrs::new() {
            Ok(gilrs) => Some(Self {
                gilrs,
                stick: (0.0, 0.0),
            }),
            Err(e) => {
                eprintln!("gamepads unavailable: {e}");
                None
            }
        }
    }

    fn poll(&mut self, input: &mut InputState) {
        use gilrs::{Axis as A, Button as B, EventType as E};
        let button = |b: B| match b {
            B::South => Some(Action::Confirm),
            B::East => Some(Action::Cancel),
            B::Start => Some(Action::Pause),
            B::DPadUp => Some(Action::MoveForward),
            B::DPadDown => Some(Action::MoveBack),
            B::DPadLeft => Some(Action::MoveLeft),
            B::DPadRight => Some(Action::MoveRight),
            _ => None,
        };
        let mut used = false;
        while let Some(event) = self.gilrs.next_event() {
            match event.event {
                E::ButtonPressed(b, _) | E::ButtonReleased(b, _) => {
                    if let Some(action) = button(b) {
                        input.set_action(action, matches!(event.event, E::ButtonPressed(..)));
                        used = true;
                    }
                }
                E::AxisChanged(A::LeftStickX, v, _) => self.stick.0 = v,
                E::AxisChanged(A::LeftStickY, v, _) => self.stick.1 = v,
                E::Disconnected => self.stick = (0.0, 0.0),
                _ => continue,
            }
            let (x, y) = self.stick;
            // Radial deadzone, rescaled so the stick still reaches full deflection.
            let len = (x * x + y * y).sqrt();
            let (x, y) = if len <= PAD_DEADZONE {
                (0.0, 0.0)
            } else {
                let k = ((len - PAD_DEADZONE) / (1.0 - PAD_DEADZONE)).min(1.0) / len;
                used = true;
                (x * k, y * k)
            };
            input.set_axis(Axis::MoveX, x);
            input.set_axis(Axis::MoveY, y);
        }
        if used {
            input.set_touch_active(false);
        }
    }
}

struct App {
    instance: Option<wgpu::Instance>,
    content: Content,
    initial: Option<Box<dyn Game>>,
    options: WindowOptions,
    stack: GameStack,
    input: InputState,
    state: Option<WindowState>,
    start: Instant,
    last: Instant,
    last_title: Instant,
    title_frames: u32,
    error: Option<String>,
    touch: TouchControls,
    /// Safe-area insets in pixels; desktop has none.
    insets: Insets,
    modifiers: ModifiersState,
    gamepads: Option<Gamepads>,
    occluded: bool,
    frame_times: Vec<f32>,
    stage_since: Option<Instant>,
    stats: RunStats,
}

impl App {
    fn layout(&self) -> Option<Layout> {
        let st = self.state.as_ref()?;
        Some(Layout::new(
            (st.config.width, st.config.height),
            self.insets,
            st.window.scale_factor() as f32,
        ))
    }

    fn init_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let mut attrs = Window::default_attributes()
            .with_title("Monkey Ball")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        if self.options.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
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
        self.stats.adapter = gpu.describe();
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = pick_surface_format(&caps.formats);
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by adapter")?;
        config.format = format;
        // Vsync: one frame per display refresh, no tearing, no busy loop.
        config.present_mode = wgpu::PresentMode::Fifo;
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
            let layout = self.layout().ok_or("window missing")?;
            let st = self.state.as_mut().ok_or("window missing")?;
            let mut ctx = GameCtx {
                dt: 0.0,
                time: 0.0,
                input: &self.input,
                renderer: &mut st.renderer,
                content: &self.content,
                size: (st.config.width, st.config.height),
                layout,
            };
            self.stack.push(game, &mut ctx)?;
        }
        let now = Instant::now();
        self.start = now;
        self.last = now;
        self.last_title = now;
        Ok(())
    }

    fn toggle_fullscreen(&mut self) {
        if let Some(st) = &self.state {
            let next = match st.window.fullscreen() {
                Some(_) => None,
                None => Some(Fullscreen::Borderless(None)),
            };
            st.window.set_fullscreen(next);
        }
    }

    fn hidden(&self) -> bool {
        self.occluded
            || self.state.as_ref().is_none_or(|st| {
                let size = st.window.inner_size();
                size.width == 0 || size.height == 0 || st.window.is_minimized() == Some(true)
            })
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let hidden = self.hidden();
        if let Some(audio) = &self.options.audio {
            // Minimized or fully covered: no sound and no mixing until the window is back.
            audio.set_suspended(hidden);
        }
        if hidden {
            // Do not count the hidden time as one long frame when the window comes back.
            self.last = Instant::now();
            return;
        }
        if let Some(pads) = &mut self.gamepads {
            pads.poll(&mut self.input);
        }
        let Some(layout) = self.layout() else {
            return;
        };
        let Some(st) = self.state.as_mut() else {
            return;
        };
        let now = Instant::now();
        let frame_time = (now - self.last).as_secs_f32();
        let dt = frame_time.min(0.1);
        self.last = now;
        let size = (st.config.width, st.config.height);
        let mut ctx = GameCtx {
            dt,
            time: (now - self.start).as_secs_f64(),
            input: &self.input,
            renderer: &mut st.renderer,
            content: &self.content,
            size,
            layout,
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
                st.renderer.discard_sprites();
                return;
            }
            C::Timeout | C::Occluded | C::Validation => {
                st.renderer.discard_sprites();
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        st.renderer.render(&view, size, &camera);
        st.window.pre_present_notify();
        st.gpu.queue.present(frame);

        // Pacing statistics are only kept for a timed run (they grow with every frame). The first frames
        // include asset loading; leave them out.
        if self.options.exit_after.is_some() && (self.stats.frames > 0 || frame_time < 0.1) {
            self.frame_times.push(frame_time);
        }
        self.stats.frames += 1;
        match (top.play_view().is_some(), self.stage_since) {
            (true, None) => self.stage_since = Some(now),
            (false, Some(_)) => self.stage_since = None,
            _ => {}
        }
        self.title_frames += 1;
        if now - self.last_title >= Duration::from_secs(1) {
            let fps = self.title_frames as f32 / (now - self.last_title).as_secs_f32();
            st.window
                .set_title(&format!("Monkey Ball | {fps:.0} fps | {}", top.status()));
            self.title_frames = 0;
            self.last_title = now;
        }
        if self
            .options
            .exit_after
            .is_some_and(|limit| (now - self.start).as_secs_f32() >= limit)
        {
            event_loop.exit();
        }
    }

    fn finish_stats(&mut self) {
        let mut times: Vec<f64> = self
            .frame_times
            .iter()
            .map(|t| *t as f64 * 1000.0)
            .collect();
        times.sort_by(|a, b| a.total_cmp(b));
        self.stats.frames = times.len() as u64;
        self.stats.seconds = times.iter().sum::<f64>() / 1000.0;
        self.stats.worst_frame_ms = times.last().copied().unwrap_or(0.0);
        self.stats.p99_frame_ms = times
            .get(times.len().saturating_sub(1) * 99 / 100)
            .copied()
            .unwrap_or(0.0);
        if let (Some(since), Some(view)) = (
            self.stage_since,
            self.stack.top().and_then(|g| g.play_view()),
        ) {
            self.stats.stage_ticks = Some((view.sim.tick, (self.last - since).as_secs_f64()));
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
                    if size.width > 0 && size.height > 0 {
                        st.config.width = size.width;
                        st.config.height = size.height;
                        st.surface.configure(&st.gpu.device, &st.config);
                    }
                }
            }
            WindowEvent::Occluded(occluded) => self.occluded = occluded,
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                let alt_enter = code == KeyCode::Enter && self.modifiers.alt_key();
                if code == KeyCode::F11 || alt_enter {
                    if pressed && !event.repeat {
                        self.toggle_fullscreen();
                    }
                    return;
                }
                if let Some(action) = action_for_key(code) {
                    self.input.set_action(action, pressed);
                    self.input.set_touch_active(false);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input
                    .set_pointer(Some((position.x as f32, position.y as f32)));
            }
            WindowEvent::CursorLeft { .. } => self.input.set_pointer(None),
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.input.set_pointer_down(state == ElementState::Pressed),
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.add_scroll(match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * WHEEL_LINE,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                });
            }
            WindowEvent::Focused(false) => {
                // Keys released while another window has focus never send their "up"; neither do fingers.
                self.input.release_all();
                self.touch.cancel_all(&mut self.input);
            }
            WindowEvent::Touch(t) => {
                let phase = match t.phase {
                    winit::event::TouchPhase::Started => Some(TouchPhase::Started),
                    winit::event::TouchPhase::Moved => Some(TouchPhase::Moved),
                    winit::event::TouchPhase::Ended => Some(TouchPhase::Ended),
                    winit::event::TouchPhase::Cancelled => None,
                };
                match (phase, self.layout()) {
                    (Some(phase), Some(layout)) => self.touch.touch(
                        t.id,
                        phase,
                        (t.location.x as f32, t.location.y as f32),
                        &layout,
                        &mut self.input,
                    ),
                    // The system took the touch away: let go without activating anything.
                    (None, _) => self.touch.cancel(t.id, &mut self.input),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.hidden() {
            // Nothing to draw: wake up a few times a second instead of spinning.
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(100),
            ));
            self.frame(event_loop);
            return;
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Some(st) = &self.state {
            st.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.finish_stats();
        if let Some(st) = self.state.as_mut() {
            let layout = Layout::new(
                (st.config.width, st.config.height),
                self.insets,
                st.window.scale_factor() as f32,
            );
            let mut ctx = GameCtx {
                dt: 0.0,
                time: 0.0,
                input: &self.input,
                renderer: &mut st.renderer,
                content: &self.content,
                size: (st.config.width, st.config.height),
                layout,
            };
            self.stack.clear(&mut ctx);
        }
    }
}

/// Open a window and run `initial` until the user quits. Returns the session's frame pacing.
pub fn run_windowed(
    content: Content,
    initial: Box<dyn Game>,
    options: WindowOptions,
) -> Result<RunStats, String> {
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
        options,
        stack: GameStack::new(),
        input: InputState::new(),
        state: None,
        start: now,
        last: now,
        last_title: now,
        title_frames: 0,
        error: None,
        touch: TouchControls::new(),
        insets: Insets::default(),
        modifiers: ModifiersState::empty(),
        gamepads: Gamepads::new(),
        occluded: false,
        frame_times: Vec::new(),
        stage_since: None,
        stats: RunStats::default(),
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop: {e}"))?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(app.stats),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_summary_reports_rate_and_ticks() {
        let stats = RunStats {
            frames: 600,
            seconds: 10.0,
            worst_frame_ms: 17.5,
            p99_frame_ms: 16.9,
            stage_ticks: Some((300, 5.0)),
            adapter: "test".into(),
        };
        assert_eq!(stats.average_fps(), 60.0);
        assert!((stats.average_frame_ms() - 16.666).abs() < 0.01);
        let s = stats.summary();
        assert!(
            s.contains("60.0 fps") && s.contains("300 sim ticks") && s.contains("expected 300")
        );
        assert_eq!(RunStats::default().average_fps(), 0.0);
    }
}
