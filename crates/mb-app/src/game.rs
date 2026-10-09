//! Game-flow skeleton: a stack of [`Game`] states (menu, stage select, play, pause, ...) driven by the
//! platform shell. States get the renderer, content and input through [`GameCtx`] and answer with a
//! [`Transition`]; nothing here knows about windows, so it runs identically headless.

use mb_render::{Camera, Renderer};

use crate::content::Content;
use crate::input::InputState;
use crate::ui::Layout;

pub struct GameCtx<'a> {
    /// Seconds since the previous update (0 for the first frame and for headless captures).
    pub dt: f32,
    /// Seconds since start.
    pub time: f64,
    pub input: &'a InputState,
    pub renderer: &'a mut Renderer,
    pub content: &'a Content,
    /// Render target size in pixels.
    pub size: (u32, u32),
    /// Safe area and scale of the target (`Layout::new(size, insets, window scale factor)`).
    pub layout: Layout,
}

/// What automated drivers (autoplay, tests) may look at while a stage is being played.
pub struct PlayView<'a> {
    /// What the menus asked for (stage, floor, lives and score at stage start, retry flag).
    pub request: &'a mb_game::PlayRequest,
    pub sim: &'a mb_engine::Sim,
    pub stage: &'a mb_stage::Stage,
    /// The ball's projected shadow is drawn in the current frame.
    pub shadow: bool,
    pub paused: bool,
    /// HUD banner on screen (`"READY"`, `"GO"`, `"GOAL"`, ...).
    pub banner: Option<&'static str>,
}

pub enum Transition {
    None,
    /// Keep the current state underneath and run a new one on top (pause menu, dialog).
    Push(Box<dyn Game>),
    /// Leave the current state and resume the one below.
    Pop,
    /// Swap the current state for another.
    Replace(Box<dyn Game>),
    Quit,
}

pub trait Game {
    fn name(&self) -> &'static str;

    /// Called once when the state becomes active (load assets, build the scene).
    fn on_enter(&mut self, _ctx: &mut GameCtx) -> Result<(), String> {
        Ok(())
    }

    /// Called when the state is removed; release renderer resources here.
    fn on_exit(&mut self, _ctx: &mut GameCtx) {}

    /// Advance the state and update renderer objects (transforms, sprites).
    fn update(&mut self, ctx: &mut GameCtx) -> Transition;

    fn camera(&self) -> Camera;

    /// Window title suffix (debug info).
    fn status(&self) -> String {
        String::new()
    }

    /// The running simulation, if this state is playing a stage.
    fn play_view(&self) -> Option<PlayView<'_>> {
        None
    }

    /// A message this state is showing to the player about something that went wrong (drivers, tests).
    fn notice(&self) -> Option<String> {
        None
    }

    /// How many "back" steps lead from here to the title screen's top level (0 there). Shells with a
    /// system back gesture (browser history) keep it inside the game while this is not zero.
    fn nav_depth(&self) -> u32 {
        0
    }
}

/// Most state changes handled in one frame (menu -> play -> ... loops would otherwise never draw).
const MAX_TRANSITIONS_PER_FRAME: usize = 6;

#[derive(Default)]
pub struct GameStack {
    stack: Vec<Box<dyn Game>>,
}

impl GameStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn top(&self) -> Option<&dyn Game> {
        self.stack.last().map(|g| g.as_ref())
    }

    pub fn push(&mut self, mut game: Box<dyn Game>, ctx: &mut GameCtx) -> Result<(), String> {
        game.on_enter(ctx)?;
        self.stack.push(game);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Leave every state (top first), releasing their renderer resources.
    pub fn clear(&mut self, ctx: &mut GameCtx) {
        while let Some(mut g) = self.stack.pop() {
            g.on_exit(ctx);
        }
    }

    /// Update the top state once and apply its transition. `Ok(None)` means it asked for no change.
    fn step(&mut self, ctx: &mut GameCtx) -> Result<Option<bool>, String> {
        let Some(top) = self.stack.last_mut() else {
            return Ok(Some(false));
        };
        match top.update(ctx) {
            Transition::None => return Ok(Some(true)),
            Transition::Push(g) => self.push(g, ctx)?,
            Transition::Pop => {
                if let Some(mut g) = self.stack.pop() {
                    g.on_exit(ctx);
                }
            }
            Transition::Replace(g) => {
                if let Some(mut old) = self.stack.pop() {
                    old.on_exit(ctx);
                }
                self.push(g, ctx)?;
            }
            Transition::Quit => return Ok(Some(false)),
        }
        Ok(None)
    }

    /// Update the top state and apply its transition. After a transition the state that is now on top is
    /// updated in the same frame (with `dt = 0` and this frame's input edges removed), so what gets
    /// rendered is always a state that has drawn itself: no blank frame between a menu and a stage, and the
    /// key that closed one screen does not act on the next. Returns false when the app should quit.
    pub fn update(&mut self, ctx: &mut GameCtx) -> Result<bool, String> {
        if let Some(running) = self.step(ctx)? {
            return Ok(running);
        }
        let muted = ctx.input.without_edges();
        let mut inner = GameCtx {
            dt: 0.0,
            time: ctx.time,
            input: &muted,
            renderer: &mut *ctx.renderer,
            content: ctx.content,
            size: ctx.size,
            layout: ctx.layout,
        };
        let mut result = None;
        for _ in 1..MAX_TRANSITIONS_PER_FRAME {
            result = self.step(&mut inner)?;
            if result.is_some() {
                break;
            }
        }
        // Requests made on the copy belong to the real input.
        ctx.input.request_virtual_controls(muted.virtual_controls());
        Ok(result.unwrap_or(!self.stack.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_starts_empty() {
        let s = GameStack::new();
        assert!(s.is_empty() && s.top().is_none());
    }
}
