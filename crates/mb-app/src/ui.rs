//! Responsive, touch-first UI primitives: safe-area insets, portrait/landscape layout, HUD scale, big tap
//! targets and an on-screen virtual stick + buttons that feed [`InputState`]. Pure maths (no GPU, no
//! windowing) so it is unit-tested and shared by desktop, Android and web shells.

use std::collections::HashMap;

use crate::input::{Action, Axis, InputState};

/// Reference height the HUD is designed for (the game's 480p-ish virtual screen scaled to 720).
pub const REFERENCE_HEIGHT: f32 = 720.0;
/// Minimum tap target edge in density-independent pixels (Material guideline).
pub const MIN_TAP_DP: f32 = 48.0;

/// Space covered by notches, system bars or rounded corners, in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Landscape,
    Portrait,
}

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub size: (f32, f32),
    pub insets: Insets,
    /// Device pixels per density-independent pixel (window scale factor).
    pub dpi_scale: f32,
}

impl Layout {
    pub fn new(size: (u32, u32), insets: Insets, dpi_scale: f32) -> Self {
        Self {
            size: (size.0 as f32, size.1 as f32),
            insets,
            dpi_scale: dpi_scale.max(0.5),
        }
    }

    pub fn orientation(&self) -> Orientation {
        if self.size.0 >= self.size.1 {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        }
    }

    /// Area free of notches and system bars; all interactive UI lives inside it.
    pub fn safe_rect(&self) -> Rect {
        let i = self.insets;
        Rect {
            x: i.left,
            y: i.top,
            w: (self.size.0 - i.left - i.right).max(0.0),
            h: (self.size.1 - i.top - i.bottom).max(0.0),
        }
    }

    /// HUD scale factor: the shorter safe edge relative to the design size, so the HUD keeps its proportions
    /// in portrait and landscape and on any resolution.
    pub fn hud_scale(&self) -> f32 {
        let s = self.safe_rect();
        let short = s.w.min(s.h);
        // Landscape designs are 720 high, portrait ones 720 wide.
        (short / REFERENCE_HEIGHT).max(0.1)
    }

    /// Smallest allowed tap target edge in pixels.
    pub fn min_tap(&self) -> f32 {
        MIN_TAP_DP * self.dpi_scale
    }

    fn button_size(&self) -> f32 {
        (self.safe_rect().w.min(self.safe_rect().h) * 0.16).max(self.min_tap() * 1.5)
    }

    /// Virtual stick zone: bottom-left quarter of the safe area (bottom half in portrait).
    pub fn stick_zone(&self) -> Rect {
        let s = self.safe_rect();
        match self.orientation() {
            Orientation::Landscape => Rect {
                x: s.x,
                y: s.y + s.h * 0.35,
                w: s.w * 0.45,
                h: s.h * 0.65,
            },
            Orientation::Portrait => Rect {
                x: s.x,
                y: s.y + s.h * 0.6,
                w: s.w * 0.55,
                h: s.h * 0.4,
            },
        }
    }

    /// Maximum stick deflection radius in pixels.
    pub fn stick_radius(&self) -> f32 {
        (self.button_size() * 0.9).max(self.min_tap())
    }

    /// Action buttons, bottom-right, stacked in a column from the bottom. Each is at least the minimum tap size.
    pub fn buttons(&self) -> Vec<(Action, Rect)> {
        let s = self.safe_rect();
        let size = self.button_size();
        let gap = size * 0.2;
        let actions = [Action::Confirm, Action::Pause, Action::ResetCamera];
        actions
            .iter()
            .enumerate()
            .map(|(i, a)| {
                (
                    *a,
                    Rect {
                        x: s.x + s.w - size - gap,
                        y: s.y + s.h - (size + gap) * (i as f32 + 1.0),
                        w: size,
                        h: size,
                    },
                )
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
}

#[derive(Clone, Copy, Debug)]
enum Role {
    Stick { origin: (f32, f32) },
    Look { last: (f32, f32) },
    Button(Action),
}

/// Maps raw touches to input: a floating stick in the stick zone drives the move axes, a drag anywhere else
/// looks around, and taps on buttons raise actions.
#[derive(Default)]
pub struct TouchControls {
    active: HashMap<u64, Role>,
}

impl TouchControls {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn touch(
        &mut self,
        id: u64,
        phase: TouchPhase,
        pos: (f32, f32),
        layout: &Layout,
        input: &mut InputState,
    ) {
        match phase {
            TouchPhase::Started => {
                let role = if let Some((a, _)) =
                    layout.buttons().into_iter().find(|(_, r)| r.contains(pos))
                {
                    input.set_action(a, true);
                    Role::Button(a)
                } else if layout.stick_zone().contains(pos)
                    && !self
                        .active
                        .values()
                        .any(|r| matches!(r, Role::Stick { .. }))
                {
                    Role::Stick { origin: pos }
                } else {
                    Role::Look { last: pos }
                };
                self.active.insert(id, role);
            }
            TouchPhase::Moved => match self.active.get_mut(&id) {
                Some(Role::Stick { origin }) => {
                    let r = layout.stick_radius();
                    let dx = ((pos.0 - origin.0) / r).clamp(-1.0, 1.0);
                    let dy = ((pos.1 - origin.1) / r).clamp(-1.0, 1.0);
                    input.set_axis(Axis::MoveX, dx);
                    // Screen Y grows downward; pushing up means forward.
                    input.set_axis(Axis::MoveY, -dy);
                }
                Some(Role::Look { last }) => {
                    input.add_mouse_delta(pos.0 - last.0, pos.1 - last.1);
                    input.set_mouse_look(true);
                    *last = pos;
                }
                _ => {}
            },
            TouchPhase::Ended => match self.active.remove(&id) {
                Some(Role::Stick { .. }) => {
                    input.set_axis(Axis::MoveX, 0.0);
                    input.set_axis(Axis::MoveY, 0.0);
                }
                Some(Role::Look { .. }) => {
                    if !self.active.values().any(|r| matches!(r, Role::Look { .. })) {
                        input.set_mouse_look(false);
                    }
                }
                Some(Role::Button(a)) => input.set_action(a, false),
                None => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landscape() -> Layout {
        Layout::new((1280, 720), Insets::default(), 2.0)
    }

    #[test]
    fn orientation_follows_aspect() {
        assert_eq!(landscape().orientation(), Orientation::Landscape);
        let p = Layout::new((720, 1280), Insets::default(), 2.0);
        assert_eq!(p.orientation(), Orientation::Portrait);
    }

    #[test]
    fn safe_rect_shrinks_by_insets() {
        let l = Layout::new(
            (1000, 500),
            Insets {
                left: 40.0,
                top: 10.0,
                right: 30.0,
                bottom: 20.0,
            },
            1.0,
        );
        let s = l.safe_rect();
        assert_eq!((s.x, s.y, s.w, s.h), (40.0, 10.0, 930.0, 470.0));
    }

    #[test]
    fn hud_scale_is_orientation_independent() {
        let a = Layout::new((1280, 720), Insets::default(), 1.0).hud_scale();
        let b = Layout::new((720, 1280), Insets::default(), 1.0).hud_scale();
        assert!((a - b).abs() < 1e-6 && (a - 1.0).abs() < 1e-6);
        assert!(Layout::new((2560, 1440), Insets::default(), 1.0).hud_scale() > 1.9);
    }

    #[test]
    fn buttons_are_big_and_inside_the_safe_area() {
        for l in [
            landscape(),
            Layout::new(
                (720, 1280),
                Insets {
                    left: 0.0,
                    top: 60.0,
                    right: 0.0,
                    bottom: 40.0,
                },
                3.0,
            ),
        ] {
            let safe = l.safe_rect();
            for (_, r) in l.buttons() {
                assert!(r.w >= l.min_tap() && r.h >= l.min_tap());
                assert!(r.x >= safe.x && r.y >= safe.y);
                assert!(r.x + r.w <= safe.x + safe.w && r.y + r.h <= safe.y + safe.h);
            }
        }
    }

    #[test]
    fn stick_drives_axes_and_releases() {
        let l = landscape();
        let mut input = InputState::new();
        let mut t = TouchControls::new();
        let z = l.stick_zone().center();
        t.touch(1, TouchPhase::Started, z, &l, &mut input);
        t.touch(
            1,
            TouchPhase::Moved,
            (z.0 + 1000.0, z.1 - 1000.0),
            &l,
            &mut input,
        );
        assert_eq!(input.axis(Axis::MoveX), 1.0);
        assert_eq!(input.axis(Axis::MoveY), 1.0);
        t.touch(1, TouchPhase::Ended, z, &l, &mut input);
        assert_eq!(input.axis(Axis::MoveX), 0.0);
    }

    #[test]
    fn button_tap_raises_and_clears_action() {
        let l = landscape();
        let mut input = InputState::new();
        let mut t = TouchControls::new();
        let (a, r) = l.buttons()[0];
        t.touch(7, TouchPhase::Started, r.center(), &l, &mut input);
        assert!(input.pressed(a) && input.held(a));
        t.touch(7, TouchPhase::Ended, r.center(), &l, &mut input);
        assert!(!input.held(a));
    }

    #[test]
    fn drag_outside_zones_looks_around() {
        let l = landscape();
        let mut input = InputState::new();
        let mut t = TouchControls::new();
        t.touch(2, TouchPhase::Started, (900.0, 100.0), &l, &mut input);
        t.touch(2, TouchPhase::Moved, (910.0, 95.0), &l, &mut input);
        assert_eq!(input.look_delta(), (10.0, -5.0));
        t.touch(2, TouchPhase::Ended, (910.0, 95.0), &l, &mut input);
        input.end_frame();
        assert_eq!(input.look_delta(), (0.0, 0.0));
    }
}
