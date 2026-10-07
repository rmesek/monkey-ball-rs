//! Game-flow skeleton: a stack of [`Game`] states (menu, stage select, play, pause, ...) driven by the
//! platform shell. States get the renderer, content and input through [`GameCtx`] and answer with a
//! [`Transition`]; nothing here knows about windows, so it runs identically headless.

use mb_render::{Camera, Renderer};

use crate::content::Content;
use crate::input::InputState;

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
}

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

    /// Update the top state and apply its transition. Returns false when the app should quit.
    pub fn update(&mut self, ctx: &mut GameCtx) -> Result<bool, String> {
        let Some(top) = self.stack.last_mut() else {
            return Ok(false);
        };
        match top.update(ctx) {
            Transition::None => {}
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
            Transition::Quit => return Ok(false),
        }
        Ok(!self.stack.is_empty())
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
