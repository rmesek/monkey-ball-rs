//! The desktop's side of the shared driver (`mb-driver`): the window's size and fullscreen, native
//! gamepads, the mouse as a pointer, the frame rate in the title, sound paused while the window is hidden,
//! and the measurements of a timed run. The event loop, the surface and the frame are the driver's.

use std::time::{Duration, Instant};

use mb_app::game::Game;
use mb_app::host::{Command, Host};
use mb_app::import_screen::ImageSlot;
use mb_app::input::{Action, Axis, InputState};
use mb_app::perf::FrameSample;
use mb_driver::Platform;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::EventLoop;
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowAttributes};

/// Stick travel ignored around the centre.
const PAD_DEADZONE: f32 = 0.15;

#[derive(Clone, Default)]
pub struct WindowOptions {
    /// Close the window this many seconds after the first frame past the import screen, and measure frame
    /// pacing meanwhile (the developer flag `--exit-after` of a `dev` build; the kill test uses it).
    pub exit_after: Option<f32>,
    pub fullscreen: bool,
    /// The game's audio system, to silence it while the window is hidden. The game states hold clones.
    pub audio: Option<mb_app::audio::Audio>,
    /// Where the import screen, if it runs, takes the image the player picks.
    pub images: Option<ImageSlot>,
}

/// Frame pacing measured over the frames of play of a session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStats {
    pub frames: u64,
    pub seconds: f64,
    pub worst_frame_ms: f64,
    /// 99th percentile frame time.
    pub p99_frame_ms: f64,
    /// Simulation ticks run in those frames.
    pub ticks: u64,
    pub adapter: String,
    /// The player chose Options -> Remove game data: the window was closed for it, and the caller
    /// deletes the data now that nothing has it open.
    pub remove_game_data: bool,
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
        format!(
            "{} frames of play in {:.2} s: {:.1} fps, frame avg {:.2} ms, p99 {:.2} ms, worst {:.2} ms ({})\n\
             stage: {} sim ticks, expected {:.0} at 60 Hz",
            self.frames,
            self.seconds,
            self.average_fps(),
            self.average_frame_ms(),
            self.p99_frame_ms,
            self.worst_frame_ms,
            self.adapter,
            self.ticks,
            self.seconds * 60.0
        )
    }
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

struct Desktop {
    options: WindowOptions,
    gamepads: Option<Gamepads>,
    modifiers: ModifiersState,
    /// Frame times of play, kept for a timed run only (they grow with every frame).
    frame_times: Vec<f32>,
    stats: RunStats,
    /// When the timed run started counting (the first frame past the import screen).
    timed_since: Option<Instant>,
    exit: bool,
    last_title: Instant,
    title_frames: u32,
}

impl Platform for Desktop {
    fn window_attributes(&mut self, attrs: WindowAttributes) -> WindowAttributes {
        let attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        if self.options.fullscreen {
            attrs.with_fullscreen(Some(Fullscreen::Borderless(None)))
        } else {
            attrs
        }
    }

    fn window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            // F11 or Alt+Enter toggles fullscreen.
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return false;
                };
                let alt_enter = code == KeyCode::Enter && self.modifiers.alt_key();
                if code == KeyCode::F11 || alt_enter {
                    if event.state == ElementState::Pressed && !event.repeat {
                        let next = match window.fullscreen() {
                            Some(_) => None,
                            None => Some(Fullscreen::Borderless(None)),
                        };
                        window.set_fullscreen(next);
                    }
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn poll_input(&mut self, input: &mut InputState, host: &mut Host) {
        if let Some(pads) = &mut self.gamepads {
            pads.poll(input);
        }
        // What the states asked of the platform. Fullscreen is a key here and tilt does not exist.
        for command in host.drain() {
            match command {
                Command::RemoveGameData => {
                    self.stats.remove_game_data = true;
                    self.exit = true;
                }
                // The import screen wants another image; without one there is nothing to play.
                Command::PickImage => match (&self.options.images, crate::pick_image()) {
                    (Some(slot), Some(image)) => slot.offer(image),
                    _ => self.exit = true,
                },
                Command::GameDataReady { bytes } => {
                    host.info
                        .insert(0, format!("Game data: {} MB", bytes >> 20));
                }
                _ => {}
            }
        }
    }

    fn after_update(&mut self, window: &Window, top: &dyn Game) {
        let now = Instant::now();
        if let Some(limit) = self.options.exit_after
            && top.name() != "import"
        {
            let since = *self.timed_since.get_or_insert(now);
            if (now - since).as_secs_f32() >= limit {
                self.exit = true;
            }
        }
        self.title_frames += 1;
        if now - self.last_title >= Duration::from_secs(1) {
            let fps = self.title_frames as f32 / (now - self.last_title).as_secs_f32();
            window.set_title(&format!("Monkey Ball | {fps:.0} fps | {}", top.status()));
            self.title_frames = 0;
            self.last_title = now;
        }
    }

    fn wants_exit(&mut self) -> bool {
        self.exit
    }

    fn on_visible(&mut self, visible: bool) {
        // Minimised or fully covered: no sound and no mixing until the window is back.
        if let Some(audio) = &self.options.audio {
            audio.set_suspended(!visible);
        }
    }

    fn mouse_as_touch(&mut self) -> bool {
        // A real pointer: menus see where it is, play is steered by keys and pads.
        false
    }

    fn on_ready(&mut self, gpu: &str) {
        eprintln!("adapter: {gpu}");
        self.stats.adapter = gpu.to_string();
    }

    fn on_frame(&mut self, sample: &FrameSample) {
        if self.timed_since.is_some() {
            self.frame_times.push(sample.interval_ms);
            self.stats.ticks += u64::from(sample.ticks);
        }
    }

    fn on_error(&mut self, msg: &str) {
        eprintln!("error: {msg}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Monkey Ball")
            .set_description(msg)
            .show();
    }
}

impl Desktop {
    fn finish_stats(mut self) -> RunStats {
        let mut times: Vec<f64> = self.frame_times.iter().map(|t| *t as f64).collect();
        times.sort_by(|a, b| a.total_cmp(b));
        self.stats.frames = times.len() as u64;
        self.stats.seconds = times.iter().sum::<f64>() / 1000.0;
        self.stats.worst_frame_ms = times.last().copied().unwrap_or(0.0);
        self.stats.p99_frame_ms = times
            .get(times.len().saturating_sub(1) * 99 / 100)
            .copied()
            .unwrap_or(0.0);
        self.stats
    }
}

/// Open a window and run `initial` until the user quits. Returns the pacing of the frames of play.
pub fn run_windowed(
    host: Host,
    content: mb_app::content::Content,
    initial: Box<dyn Game>,
    options: WindowOptions,
) -> Result<RunStats, String> {
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    let now = Instant::now();
    let desktop = Desktop {
        options,
        gamepads: Gamepads::new(),
        modifiers: ModifiersState::empty(),
        frame_times: Vec::new(),
        stats: RunStats::default(),
        timed_since: None,
        exit: false,
        last_title: now,
        title_frames: 0,
    };
    Ok(mb_driver::run(event_loop, desktop, host, content, initial)?.finish_stats())
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
            ticks: 600,
            adapter: "test".into(),
            remove_game_data: false,
        };
        assert_eq!(stats.average_fps(), 60.0);
        assert!((stats.average_frame_ms() - 16.666).abs() < 0.01);
        let s = stats.summary();
        assert!(
            s.contains("60.0 fps") && s.contains("600 sim ticks") && s.contains("expected 600")
        );
        assert_eq!(RunStats::default().average_fps(), 0.0);
    }
}
