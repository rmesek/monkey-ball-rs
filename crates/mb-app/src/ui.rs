//! Responsive, touch-first UI primitives: safe-area insets, portrait/landscape layout, HUD scale, big tap
//! targets and an on-screen virtual stick + buttons that feed [`InputState`]. Pure maths (no GPU, no
//! windowing) so it is unit-tested and shared by the desktop and web shells.

use std::collections::HashMap;

use crate::input::{Action, Axis, InputState, TouchStick};

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

    /// On-screen action buttons, bottom-right, stacked in a column from the bottom. Each is at least the
    /// minimum tap size. Gameplay only needs pause; the stick does the rest. While the device's tilt steers
    /// (`tilt`) there is no stick, and a second button takes the current pose as level.
    pub fn buttons(&self, tilt: bool) -> Vec<(Action, Rect)> {
        let s = self.safe_rect();
        let size = self.button_size();
        let gap = size * 0.2;
        let actions: &[Action] = if tilt {
            &[Action::Pause, Action::Recenter]
        } else {
            &[Action::Pause]
        };
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

    /// Where the stick is drawn while no finger holds it.
    pub fn stick_rest(&self) -> (f32, f32) {
        let z = self.stick_zone();
        let r = self.stick_radius();
        (z.x + r * 1.6, z.y + z.h - r * 1.6)
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
    Stick {
        origin: (f32, f32),
    },
    Button(Action),
    /// Started while virtual controls were off, or outside the stick zone and the buttons: pointer only.
    Pointer,
}

/// Maps raw touches to input. Every touch marks touch as the active device and the first finger doubles as
/// the pointer (menus). While the running state asks for virtual controls
/// ([`InputState::request_virtual_controls`]) a floating stick in the stick zone drives the move axes and
/// taps on buttons raise actions; otherwise, and anywhere else on the screen, touches are pointer-only.
#[derive(Default)]
pub struct TouchControls {
    active: HashMap<u64, Role>,
    /// The finger that currently acts as the pointer.
    pointer_id: Option<u64>,
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
        // A touch that starts while its id is still known lost its end somewhere (browsers and drivers do
        // drop them): let go of what the old one held first, or a stick, a button or the pointer would
        // stay held for good.
        if phase == TouchPhase::Started {
            self.cancel(id, input);
        }
        self.route(id, phase, pos, layout, input);
        // Last, because raising a button action counts as key use, which clears the flag.
        input.set_touch_active(true);
    }

    /// The touch `id` is gone without a proper end (the system cancelled it): release what it held. Unlike
    /// an end this is not a tap, so nothing is activated.
    pub fn cancel(&mut self, id: u64, input: &mut InputState) {
        if self.pointer_id == Some(id) {
            self.pointer_id = None;
            input.cancel_pointer();
        }
        self.release(id, input);
    }

    /// Every touch is gone (focus lost, window hidden, app suspended): release everything they held.
    pub fn cancel_all(&mut self, input: &mut InputState) {
        let ids: Vec<u64> = self.active.keys().copied().chain(self.pointer_id).collect();
        for id in ids {
            self.cancel(id, input);
        }
    }

    /// Let go of whatever the touch `id` was doing.
    fn release(&mut self, id: u64, input: &mut InputState) {
        match self.active.remove(&id) {
            Some(Role::Stick { .. }) => {
                input.set_axis(Axis::MoveX, 0.0);
                input.set_axis(Axis::MoveY, 0.0);
                input.set_touch_stick(None);
            }
            Some(Role::Button(a)) => input.set_action(a, false),
            Some(Role::Pointer) | None => {}
        }
    }

    fn route(
        &mut self,
        id: u64,
        phase: TouchPhase,
        pos: (f32, f32),
        layout: &Layout,
        input: &mut InputState,
    ) {
        match phase {
            TouchPhase::Started if self.pointer_id.is_none() => {
                self.pointer_id = Some(id);
                input.set_pointer(Some(pos));
                input.set_pointer_down(true);
            }
            TouchPhase::Moved if self.pointer_id == Some(id) => input.set_pointer(Some(pos)),
            TouchPhase::Ended if self.pointer_id == Some(id) => {
                self.pointer_id = None;
                input.set_pointer(Some(pos));
                input.set_pointer_down(false);
            }
            _ => {}
        }
        match phase {
            TouchPhase::Started => {
                if !input.virtual_controls() {
                    self.active.insert(id, Role::Pointer);
                    return;
                }
                let role = if let Some((a, _)) = layout
                    .buttons(input.tilt_steering())
                    .into_iter()
                    .find(|(_, r)| r.contains(pos))
                {
                    input.set_action(a, true);
                    Role::Button(a)
                } else if !input.tilt_steering()
                    && layout.stick_zone().contains(pos)
                    && !self
                        .active
                        .values()
                        .any(|r| matches!(r, Role::Stick { .. }))
                {
                    input.set_touch_stick(Some(TouchStick { origin: pos, pos }));
                    Role::Stick { origin: pos }
                } else {
                    Role::Pointer
                };
                self.active.insert(id, role);
            }
            TouchPhase::Moved => {
                if let Some(Role::Stick { origin }) = self.active.get_mut(&id) {
                    let r = layout.stick_radius();
                    // Clamped to the circle: the knob stays on its base and a diagonal is not faster.
                    let (mut dx, mut dy) = ((pos.0 - origin.0) / r, (pos.1 - origin.1) / r);
                    let len = (dx * dx + dy * dy).sqrt();
                    if len > 1.0 {
                        dx /= len;
                        dy /= len;
                    }
                    input.set_touch_stick(Some(TouchStick {
                        origin: *origin,
                        pos: (origin.0 + dx * r, origin.1 + dy * r),
                    }));
                    input.set_axis(Axis::MoveX, dx);
                    // Screen Y grows downward; pushing up means forward.
                    input.set_axis(Axis::MoveY, -dy);
                }
            }
            TouchPhase::Ended => self.release(id, input),
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
            for (_, r) in l.buttons(true) {
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
        input.request_virtual_controls(true);
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
        // A diagonal is clamped to the circle: the knob stays on the base disc.
        let (x, y) = (input.axis(Axis::MoveX), input.axis(Axis::MoveY));
        assert!((x - y).abs() < 1e-6 && ((x * x + y * y).sqrt() - 1.0).abs() < 1e-5);
        let stick = input.touch_stick().unwrap();
        let reach = ((stick.pos.0 - stick.origin.0).powi(2)
            + (stick.pos.1 - stick.origin.1).powi(2))
        .sqrt();
        assert!((reach - l.stick_radius()).abs() < 1e-2, "knob {reach}");
        // Straight along one axis still reaches full deflection.
        t.touch(1, TouchPhase::Moved, (z.0 + 1000.0, z.1), &l, &mut input);
        assert_eq!(
            (input.axis(Axis::MoveX), input.axis(Axis::MoveY)),
            (1.0, 0.0)
        );
        t.touch(1, TouchPhase::Ended, z, &l, &mut input);
        assert_eq!(input.axis(Axis::MoveX), 0.0);
    }

    /// A touch whose end was lost and whose id starts again must not leave the stick, a button or the
    /// pointer held.
    #[test]
    fn a_repeated_start_releases_what_the_lost_touch_held() {
        let l = landscape();
        let mut input = InputState::new();
        input.request_virtual_controls(true);
        let mut t = TouchControls::new();
        let z = l.stick_zone().center();
        let (pause, button) = l.buttons(false)[0];
        // Stick held by touch 1, then touch 1 "starts" again on the pause button.
        t.touch(1, TouchPhase::Started, z, &l, &mut input);
        t.touch(1, TouchPhase::Moved, (z.0 + 1000.0, z.1), &l, &mut input);
        assert_eq!(input.axis(Axis::MoveX), 1.0);
        t.touch(1, TouchPhase::Started, button.center(), &l, &mut input);
        assert_eq!(input.axis(Axis::MoveX), 0.0, "the stale stick was released");
        assert!(input.touch_stick().is_none() && input.held(pause));
        // The button held by touch 1, then touch 1 starts again in the stick zone: pause is released and
        // a new stick is accepted (it used to be refused for ever).
        t.touch(1, TouchPhase::Started, z, &l, &mut input);
        assert!(!input.held(pause));
        t.touch(1, TouchPhase::Moved, (z.0, z.1 - 1000.0), &l, &mut input);
        assert_eq!(input.axis(Axis::MoveY), 1.0);
        t.touch(1, TouchPhase::Ended, z, &l, &mut input);
        input.end_frame();
        // Pause still works afterwards.
        t.touch(2, TouchPhase::Started, button.center(), &l, &mut input);
        assert!(input.pressed(pause));
        t.touch(2, TouchPhase::Ended, button.center(), &l, &mut input);

        // Menus: the pointer of a lost touch is let go without a click, and the next touch is a new press.
        input.request_virtual_controls(false);
        input.end_frame();
        t.touch(5, TouchPhase::Started, (10.0, 10.0), &l, &mut input);
        input.end_frame();
        t.touch(5, TouchPhase::Started, (300.0, 300.0), &l, &mut input);
        assert!(input.pointer_pressed() && !input.pointer_released());
        assert_eq!(input.pointer(), Some((300.0, 300.0)));
        t.touch(5, TouchPhase::Ended, (300.0, 300.0), &l, &mut input);
        assert!(input.pointer_released() && !input.pointer_held());
    }

    #[test]
    fn cancelling_releases_everything_without_a_click() {
        let l = landscape();
        let mut input = InputState::new();
        input.request_virtual_controls(true);
        let mut t = TouchControls::new();
        let z = l.stick_zone().center();
        let (pause, button) = l.buttons(false)[0];
        t.touch(1, TouchPhase::Started, z, &l, &mut input);
        t.touch(1, TouchPhase::Moved, (z.0 - 1000.0, z.1), &l, &mut input);
        t.touch(2, TouchPhase::Started, button.center(), &l, &mut input);
        t.touch(3, TouchPhase::Started, (900.0, 100.0), &l, &mut input);
        t.touch(3, TouchPhase::Moved, (910.0, 100.0), &l, &mut input);
        input.end_frame();
        assert!(input.held(pause) && input.pointer_held() && input.axis(Axis::MoveX) == -1.0);
        t.cancel_all(&mut input);
        assert!(!input.held(pause) && !input.pointer_held() && !input.pointer_released());
        assert_eq!(input.axis(Axis::MoveX), 0.0);
        assert!(input.touch_stick().is_none());
        // One cancelled touch (the system took it for a gesture) leaves the others alone.
        t.touch(1, TouchPhase::Started, z, &l, &mut input);
        t.touch(2, TouchPhase::Started, button.center(), &l, &mut input);
        t.cancel(2, &mut input);
        assert!(!input.held(pause) && input.touch_stick().is_some());
    }

    #[test]
    fn button_tap_raises_and_clears_action() {
        let l = landscape();
        let mut input = InputState::new();
        input.request_virtual_controls(true);
        let mut t = TouchControls::new();
        let (a, r) = l.buttons(false)[0];
        t.touch(7, TouchPhase::Started, r.center(), &l, &mut input);
        assert!(input.pressed(a) && input.held(a));
        t.touch(7, TouchPhase::Ended, r.center(), &l, &mut input);
        assert!(!input.held(a));
    }

    #[test]
    fn touches_are_pointer_only_without_virtual_controls() {
        let l = landscape();
        let mut input = InputState::new();
        let mut t = TouchControls::new();
        let (a, r) = l.buttons(false)[0];
        t.touch(1, TouchPhase::Started, r.center(), &l, &mut input);
        assert!(!input.held(a) && input.touch_active());
        assert!(input.pointer_pressed() && input.pointer() == Some(r.center()));
        t.touch(1, TouchPhase::Moved, (5.0, 6.0), &l, &mut input);
        assert_eq!(input.pointer(), Some((5.0, 6.0)));
        assert_eq!(input.axis(Axis::MoveX), 0.0);
        t.touch(1, TouchPhase::Ended, (5.0, 6.0), &l, &mut input);
        assert!(input.pointer_released() && !input.pointer_held());
    }

    #[test]
    fn a_drag_outside_the_stick_zone_and_the_buttons_steers_nothing() {
        let l = landscape();
        let mut input = InputState::new();
        input.request_virtual_controls(true);
        let mut t = TouchControls::new();
        t.touch(2, TouchPhase::Started, (900.0, 100.0), &l, &mut input);
        t.touch(2, TouchPhase::Moved, (910.0, 95.0), &l, &mut input);
        assert_eq!(
            (input.axis(Axis::MoveX), input.axis(Axis::MoveY)),
            (0.0, 0.0)
        );
        assert!(input.touch_stick().is_none() && input.pointer() == Some((910.0, 95.0)));
        t.touch(2, TouchPhase::Ended, (910.0, 95.0), &l, &mut input);
        assert!(!input.pointer_held());
    }

    #[test]
    fn tilt_steering_replaces_the_stick_with_a_recentre_button() {
        let l = landscape();
        let mut input = InputState::new();
        input.request_virtual_controls(true);
        input.request_tilt_steering(true);
        let mut t = TouchControls::new();
        // A touch in the stick zone is no stick now.
        let z = l.stick_zone().center();
        t.touch(1, TouchPhase::Started, z, &l, &mut input);
        t.touch(1, TouchPhase::Moved, (z.0 + 500.0, z.1), &l, &mut input);
        assert!(input.touch_stick().is_none());
        assert_eq!(input.axis(Axis::MoveX), 0.0);
        t.touch(1, TouchPhase::Ended, z, &l, &mut input);
        // Pause is still there, and the second button recentres.
        let buttons = l.buttons(true);
        assert_eq!(buttons[0].0, Action::Pause);
        let (a, r) = buttons[1];
        assert_eq!(a, Action::Recenter);
        assert!(!r.contains(buttons[0].1.center()));
        t.touch(2, TouchPhase::Started, r.center(), &l, &mut input);
        assert!(input.pressed(Action::Recenter));
        t.touch(2, TouchPhase::Ended, r.center(), &l, &mut input);
        assert!(!input.held(Action::Recenter));
        // Without tilt there is no such button.
        assert_eq!(l.buttons(false).len(), 1);
    }
}
