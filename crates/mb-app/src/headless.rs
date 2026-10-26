//! Windowless driver: runs real game states on an offscreen target with scripted input. Used by the
//! integration tests. Compiled with the `dev` feature only.

use mb_game::{MemoryStorage, Profile, Storage};
use mb_render::{OffscreenTarget, Renderer};

use crate::autopilot::Autopilot;
use crate::content::Content;
use crate::game::{Game, GameCtx, GameStack};
use crate::gpu::request_gpu;
use crate::host::Host;
use crate::input::{Action, Axis, InputState};
use crate::ui::{Insets, Layout};

/// One simulation tick, the frame time scripted runs use.
pub const FRAME: f32 = 1.0 / 60.0;

/// A game stack with its own GPU device, renderer and offscreen target.
pub struct Headless {
    content: Content,
    renderer: Renderer,
    target: OffscreenTarget,
    pub input: InputState,
    /// What the platform offers and knows; tests set it and read the commands posted on it.
    pub host: Host,
    stack: GameStack,
    size: (u32, u32),
    time: f64,
    adapter: String,
    pending_release: Vec<Action>,
}

impl Headless {
    /// Create a device without a window (a software adapter is accepted as a fallback).
    pub fn new(content: &Content, size: (u32, u32), force_software: bool) -> Result<Self, String> {
        Self::on_backends(content, size, force_software, None)
    }

    /// [`Headless::new`] on the given graphics backends only (`None`: any). The OpenGL backend is the
    /// one the browser build uses for WebGL2, so what it costs can be measured without a browser.
    pub fn on_backends(
        content: &Content,
        size: (u32, u32),
        force_software: bool,
        backends: Option<wgpu::Backends>,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(crate::gpu::instance_descriptor(backends));
        let gpu = request_gpu(instance, None, force_software)?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let renderer = Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
        let target = OffscreenTarget::new(&gpu.device, size.0, size.1, format);
        Ok(Self {
            content: content.clone(),
            renderer,
            target,
            input: InputState::new(),
            host: Host::default(),
            stack: GameStack::new(),
            size,
            time: 0.0,
            adapter: gpu.describe(),
            pending_release: Vec::new(),
        })
    }

    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    pub fn layout(&self) -> Layout {
        Layout::new(self.size, Insets::default(), 1.0)
    }

    pub fn push(&mut self, game: Box<dyn Game>) -> Result<(), String> {
        let mut ctx = GameCtx {
            dt: 0.0,
            time: self.time,
            input: &self.input,
            renderer: &mut self.renderer,
            content: &self.content,
            host: &mut self.host,
            size: self.size,
            layout: Layout::new(self.size, Insets::default(), 1.0),
        };
        self.stack.push(game, &mut ctx)
    }

    pub fn top(&self) -> Option<&dyn Game> {
        self.stack.top()
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Status line of the top state (empty when the stack is empty).
    pub fn status(&self) -> String {
        self.top().map(|g| g.status()).unwrap_or_default()
    }

    /// Run one frame of `dt` seconds: update the stack, then clear the per-frame input edges. Sprites the
    /// previous frame queued but nobody rendered are dropped first. Returns false when the game quit.
    pub fn step(&mut self, dt: f32) -> Result<bool, String> {
        self.renderer.discard_sprites();
        self.time += dt as f64;
        let mut ctx = GameCtx {
            dt,
            time: self.time,
            input: &self.input,
            renderer: &mut self.renderer,
            content: &self.content,
            host: &mut self.host,
            size: self.size,
            layout: Layout::new(self.size, Insets::default(), 1.0),
        };
        let running = self.stack.update(&mut ctx)?;
        self.input.end_frame();
        for action in self.pending_release.drain(..) {
            self.input.set_action(action, false);
        }
        Ok(running)
    }

    /// Press `action` for exactly the next frame.
    pub fn tap(&mut self, action: Action) -> Result<bool, String> {
        self.input.set_action(action, true);
        self.pending_release.push(action);
        self.step(FRAME)
    }

    /// Render what the last [`Headless::step`] drew and read it back as RGBA8.
    pub fn render(&mut self) -> Result<Vec<u8>, String> {
        let camera = self.stack.top().ok_or("no game state")?.camera();
        Ok(self.renderer.render_to_rgba(&self.target, &camera))
    }

    /// [`Headless::render`] through another camera (tests that look at the scene from elsewhere).
    pub fn render_with(&mut self, camera: &mb_render::Camera) -> Vec<u8> {
        self.renderer.render_to_rgba(&self.target, camera)
    }

    pub fn draws(&self) -> usize {
        self.renderer.stats.draws
    }

    /// The renderer, for what tests measure: `stats` of the frame last rendered with
    /// [`Headless::render`], `load_stats` of the stage last built, texture and object counts.
    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    /// The renderer, for tests that change what is drawn between two renders of one frame (the shadow
    /// on and off).
    pub fn renderer_mut(&mut self) -> &mut Renderer {
        &mut self.renderer
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Draw into a target of another size from the next frame on, as after the window was resized: the
    /// states get the new size and layout, and the renderer sees it with the next frame it draws.
    pub fn resize(&mut self, size: (u32, u32)) {
        let format = self.renderer.target_format();
        self.target = OffscreenTarget::new(self.renderer.device(), size.0, size.1, format);
        self.size = size;
    }

    /// Steer with `pilot` for one frame if a stage is being played (the stick goes through `InputState`
    /// like a gamepad would). Returns false when no stage is running.
    pub fn pilot(&mut self, pilot: &mut Option<Autopilot>, goal_only: bool) -> bool {
        let Some(view) = self.stack.top().and_then(|g| g.play_view()) else {
            *pilot = None;
            return false;
        };
        // A fresh simulation (new floor or retry) needs a fresh route.
        if view.sim.tick == 0 || pilot.is_none() {
            *pilot = if goal_only {
                Autopilot::goal_only(view.stage)
            } else {
                Autopilot::for_stage(view.stage)
            };
        }
        let (x, y) = match pilot {
            Some(p) => p.axes(view.sim),
            None => (0.0, 0.0),
        };
        self.input.set_axis(Axis::MoveX, x);
        self.input.set_axis(Axis::MoveY, y);
        true
    }

    /// Leave all states and free their GPU resources.
    pub fn shutdown(&mut self) {
        let mut ctx = GameCtx {
            dt: 0.0,
            time: self.time,
            input: &self.input,
            renderer: &mut self.renderer,
            content: &self.content,
            host: &mut self.host,
            size: self.size,
            layout: Layout::new(self.size, Insets::default(), 1.0),
        };
        self.stack.clear(&mut ctx);
    }
}

/// In-memory save with every stage selectable, so scripted runs never touch the player's own save.
pub fn unlocked_storage() -> Box<dyn Storage> {
    let mut storage = MemoryStorage::new();
    let mut profile = Profile::default();
    profile.settings.unlock_all = true;
    // Writing to memory cannot fail.
    let _ = profile.save(&mut storage);
    Box::new(storage)
}
