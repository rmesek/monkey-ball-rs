//! Input abstraction. The game reads logical [`Action`]s and [`Axis`] values from [`InputState`]; platform
//! shells (winit keyboard/mouse, gamepads, touch, device tilt) just feed it.
//!
//! What a shell feeds, once per event or per frame:
//! * keys / buttons: [`InputState::set_action`]; sticks: [`InputState::set_axis`] (`MoveX`/`MoveY`, +Y = forward);
//!   device tilt (`tilt`): `TiltX`/`TiltY`, same sign convention, non-zero tilt wins over the stick.
//! * pointer (mouse cursor or the primary touch), in target pixels with the origin top-left:
//!   [`InputState::set_pointer`] on every move (`None` when the cursor leaves the window),
//!   [`InputState::set_pointer_down`] for the primary button or touch contact, [`InputState::add_scroll`]
//!   for the wheel in pixels (positive = content moves down). `ui::TouchControls` already does this for
//!   touches, so a shell only has to add the mouse.
//! * [`InputState::end_frame`] after each update.
//!
//! What game states read: [`InputState::tilt`] (the only thing gameplay steers with), actions, the pointer
//! (`pointer`, `pointer_pressed`, `pointer_released`, `scroll`) and [`InputState::menu_nav`].

use std::cell::Cell;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveForward,
    MoveBack,
    MoveLeft,
    MoveRight,
    /// The arrow keys: a second set of direction keys (steering and menu steps, like WASD).
    LookLeft,
    LookRight,
    LookUp,
    LookDown,
    Confirm,
    Cancel,
    Pause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    MoveX,
    MoveY,
    /// Device tilt (the browser shell feeds it from `deviceorientation`, see `tilt`).
    TiltX,
    TiltY,
}

impl Axis {
    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Default, Clone)]
pub struct InputState {
    held: HashSet<Action>,
    pressed: HashSet<Action>,
    axes: [f32; 4],
    pointer: Option<(f32, f32)>,
    pointer_held: bool,
    pointer_pressed: bool,
    pointer_released: bool,
    scroll: f32,
    touch_active: bool,
    touch_stick: Option<TouchStick>,
    /// Requested by the running state each frame; read by `ui::TouchControls` when a touch starts.
    virtual_controls: Cell<bool>,
    nav_axis_prev: (i8, i8),
}

/// The floating on-screen stick while a finger holds it (for drawing it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchStick {
    pub origin: (f32, f32),
    pub pos: (f32, f32),
}

/// Stick deflection that counts as a menu step.
const NAV_THRESHOLD: f32 = 0.6;

fn clamp_circle(x: f32, y: f32) -> (f32, f32) {
    let len = (x * x + y * y).sqrt();
    if len > 1.0 {
        (x / len, y / len)
    } else {
        (x, y)
    }
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// A key or button changed. A new press also marks keys / buttons as the device in use, which hides
    /// the on-screen touch controls until the next touch.
    pub fn set_action(&mut self, action: Action, down: bool) {
        if down {
            if self.held.insert(action) {
                self.pressed.insert(action);
                self.touch_active = false;
            }
        } else {
            self.held.remove(&action);
        }
    }

    /// Analog value in -1..=1 (gamepad sticks, touch joystick, tilt).
    ///
    /// A value that is not a number (a sensor without a reading) counts as centred: `clamp` would keep the
    /// NaN, and a NaN tilt axis would win over the stick and the keys for good.
    pub fn set_axis(&mut self, axis: Axis, value: f32) {
        self.axes[axis.index()] = if value.is_nan() {
            0.0
        } else {
            value.clamp(-1.0, 1.0)
        };
    }

    pub fn axis(&self, axis: Axis) -> f32 {
        self.axes[axis.index()]
    }

    pub fn held(&self, action: Action) -> bool {
        self.held.contains(&action)
    }

    /// True only on the frame the action went down.
    pub fn pressed(&self, action: Action) -> bool {
        self.pressed.contains(&action)
    }

    /// Normalized steering vector for gameplay: `x` right, `y` forward, length <= 1 for analog sources.
    /// Priority like the reference's `getStick`: device tilt, then the stick (gamepad or touch), then keys
    /// (WASD / arrows, full deflection per axis, unramped as in the reference: the world tilt itself eases).
    pub fn tilt(&self) -> (f32, f32) {
        let (tx, ty) = (self.axis(Axis::TiltX), self.axis(Axis::TiltY));
        if tx != 0.0 || ty != 0.0 {
            return clamp_circle(tx, ty);
        }
        let (sx, sy) = (self.axis(Axis::MoveX), self.axis(Axis::MoveY));
        if sx != 0.0 || sy != 0.0 {
            return clamp_circle(sx, sy);
        }
        let key = |neg: [Action; 2], pos: [Action; 2]| {
            pos.iter().any(|a| self.held(*a)) as i32 as f32
                - neg.iter().any(|a| self.held(*a)) as i32 as f32
        };
        (
            key(
                [Action::MoveLeft, Action::LookLeft],
                [Action::MoveRight, Action::LookRight],
            ),
            key(
                [Action::MoveBack, Action::LookDown],
                [Action::MoveForward, Action::LookUp],
            ),
        )
    }

    fn nav_axis(&self) -> (i8, i8) {
        let step = |v: f32| (v > NAV_THRESHOLD) as i8 - (v < -NAV_THRESHOLD) as i8;
        (step(self.axis(Axis::MoveX)), step(self.axis(Axis::MoveY)))
    }

    /// Edge-triggered menu step `(dx, dy)`, `dy = 1` meaning down the list: WASD, arrows, or a stick flick.
    pub fn menu_nav(&self) -> (i32, i32) {
        let key = |a: Action, b: Action| (self.pressed(a) || self.pressed(b)) as i32;
        let mut dx =
            key(Action::MoveRight, Action::LookRight) - key(Action::MoveLeft, Action::LookLeft);
        let mut dy =
            key(Action::MoveBack, Action::LookDown) - key(Action::MoveForward, Action::LookUp);
        let now = self.nav_axis();
        if now.0 != self.nav_axis_prev.0 {
            dx += now.0 as i32;
        }
        if now.1 != self.nav_axis_prev.1 {
            dy -= now.1 as i32;
        }
        (dx.clamp(-1, 1), dy.clamp(-1, 1))
    }

    // ---- pointer (mouse cursor or primary touch) -----------------------------------------------------

    pub fn set_pointer(&mut self, pos: Option<(f32, f32)>) {
        self.pointer = pos;
    }

    pub fn set_pointer_down(&mut self, down: bool) {
        if down && !self.pointer_held {
            self.pointer_pressed = true;
        }
        if !down && self.pointer_held {
            self.pointer_released = true;
        }
        self.pointer_held = down;
    }

    /// The primary button / touch is gone without having been released (the system took the touch away,
    /// the window lost focus): no longer held, and no release edge, so nothing is clicked.
    pub fn cancel_pointer(&mut self) {
        self.pointer_held = false;
        self.pointer_pressed = false;
    }

    pub fn add_scroll(&mut self, dy: f32) {
        self.scroll += dy;
    }

    /// Pointer position in target pixels (origin top-left), if there is one.
    pub fn pointer(&self) -> Option<(f32, f32)> {
        self.pointer
    }

    pub fn pointer_held(&self) -> bool {
        self.pointer_held
    }

    /// True on the frame the primary button / touch went down.
    pub fn pointer_pressed(&self) -> bool {
        self.pointer_pressed
    }

    /// True on the frame it was released (a click or tap completes here).
    pub fn pointer_released(&self) -> bool {
        self.pointer_released
    }

    /// Wheel movement since the last frame, in pixels.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    // ---- touch ---------------------------------------------------------------------------------------

    /// True from a touch until the next key or button press: states draw the on-screen controls only
    /// while it is set. `ui::TouchControls` and [`InputState::set_action`] maintain it.
    pub fn set_touch_active(&mut self, on: bool) {
        self.touch_active = on;
    }

    pub fn touch_active(&self) -> bool {
        self.touch_active
    }

    pub fn set_touch_stick(&mut self, stick: Option<TouchStick>) {
        self.touch_stick = stick;
    }

    pub fn touch_stick(&self) -> Option<TouchStick> {
        self.touch_stick
    }

    /// Called by the running state every frame: `true` while it wants the virtual stick and buttons
    /// (gameplay), `false` when touches should only act as a pointer (menus, pause).
    pub fn request_virtual_controls(&self, on: bool) {
        self.virtual_controls.set(on);
    }

    pub fn virtual_controls(&self) -> bool {
        self.virtual_controls.get()
    }

    /// A copy without this frame's edges (presses, clicks, wheel). The state stack hands it to a state that
    /// starts or resumes in the middle of a frame, so the key that closed one screen does not also act on
    /// the next.
    pub fn without_edges(&self) -> InputState {
        let mut copy = self.clone();
        copy.pressed.clear();
        copy.pointer_pressed = false;
        copy.pointer_released = false;
        copy.scroll = 0.0;
        copy.nav_axis_prev = copy.nav_axis();
        copy
    }

    /// Let go of every held action and the pointer button (the window lost focus, so the matching "up"
    /// events will never arrive).
    pub fn release_all(&mut self) {
        self.held.clear();
        self.set_pointer_down(false);
    }

    /// Call after each update to clear per-frame state.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.pointer_pressed = false;
        self.pointer_released = false;
        self.scroll = 0.0;
        self.nav_axis_prev = self.nav_axis();
    }
}

/// Default desktop keyboard bindings.
pub fn action_for_key(key: winit::keyboard::KeyCode) -> Option<Action> {
    use winit::keyboard::KeyCode as K;
    Some(match key {
        K::KeyW => Action::MoveForward,
        K::KeyS => Action::MoveBack,
        K::KeyA => Action::MoveLeft,
        K::KeyD => Action::MoveRight,
        K::ArrowLeft => Action::LookLeft,
        K::ArrowRight => Action::LookRight,
        K::ArrowUp => Action::LookUp,
        K::ArrowDown => Action::LookDown,
        K::Enter => Action::Confirm,
        K::Escape => Action::Cancel,
        K::KeyP => Action::Pause,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressed_is_edge_triggered() {
        let mut i = InputState::new();
        i.set_action(Action::Confirm, true);
        assert!(i.pressed(Action::Confirm) && i.held(Action::Confirm));
        i.end_frame();
        assert!(!i.pressed(Action::Confirm) && i.held(Action::Confirm));
        // Key repeat must not re-trigger.
        i.set_action(Action::Confirm, true);
        assert!(!i.pressed(Action::Confirm));
        i.set_action(Action::Confirm, false);
        assert!(!i.held(Action::Confirm));
    }

    #[test]
    fn tilt_prefers_device_tilt_then_stick_then_keys() {
        let mut i = InputState::new();
        i.set_action(Action::MoveForward, true);
        i.set_action(Action::LookRight, true);
        assert_eq!(i.tilt(), (1.0, 1.0));
        i.set_axis(Axis::MoveX, -0.5);
        assert_eq!(i.tilt(), (-0.5, 0.0));
        i.set_axis(Axis::TiltY, 0.25);
        assert_eq!(i.tilt(), (0.0, 0.25));
    }

    #[test]
    fn analog_tilt_is_clamped_to_the_unit_circle() {
        let mut i = InputState::new();
        i.set_axis(Axis::MoveX, 1.0);
        i.set_axis(Axis::MoveY, 1.0);
        let (x, y) = i.tilt();
        assert!(((x * x + y * y).sqrt() - 1.0).abs() < 1e-6 && x > 0.7 && y > 0.7);
    }

    #[test]
    fn axis_values_that_are_not_numbers_count_as_centred() {
        let mut i = InputState::new();
        i.set_action(Action::MoveForward, true);
        i.set_axis(Axis::TiltX, f32::NAN);
        i.set_axis(Axis::MoveY, f32::NAN);
        assert_eq!(i.axis(Axis::TiltX), 0.0);
        // The keys still steer.
        assert_eq!(i.tilt(), (0.0, 1.0));
        i.set_axis(Axis::MoveX, f32::INFINITY);
        i.set_axis(Axis::MoveY, f32::NEG_INFINITY);
        let (x, y) = i.tilt();
        assert!((x * x + y * y).sqrt() <= 1.0 + 1e-6 && x > 0.0 && y < 0.0);
        // Every source at once: each component stays in range for the quantiser.
        i.set_axis(Axis::TiltX, 1.0);
        i.set_axis(Axis::TiltY, -1.0);
        i.set_action(Action::LookLeft, true);
        let (x, y) = i.tilt();
        assert!(x.abs() <= 1.0 && y.abs() <= 1.0 && (x * x + y * y).sqrt() <= 1.0 + 1e-6);
    }

    #[test]
    fn pointer_edges_last_one_frame() {
        let mut i = InputState::new();
        i.set_pointer(Some((10.0, 20.0)));
        i.set_pointer_down(true);
        assert!(i.pointer_pressed() && i.pointer_held() && !i.pointer_released());
        i.end_frame();
        assert!(!i.pointer_pressed() && i.pointer_held());
        i.set_pointer_down(false);
        assert!(i.pointer_released() && !i.pointer_held());
        assert_eq!(i.pointer(), Some((10.0, 20.0)));
        i.add_scroll(12.0);
        assert_eq!(i.scroll(), 12.0);
        i.end_frame();
        assert!(!i.pointer_released() && i.scroll() == 0.0);
    }

    #[test]
    fn menu_nav_is_edge_triggered_for_keys_and_stick() {
        let mut i = InputState::new();
        i.set_action(Action::LookDown, true);
        assert_eq!(i.menu_nav(), (0, 1));
        i.end_frame();
        assert_eq!(i.menu_nav(), (0, 0));
        i.set_action(Action::LookDown, false);
        i.set_axis(Axis::MoveY, 1.0);
        assert_eq!(i.menu_nav(), (0, -1));
        i.end_frame();
        assert_eq!(i.menu_nav(), (0, 0));
    }

    #[test]
    fn keyboard_bindings() {
        assert_eq!(
            action_for_key(winit::keyboard::KeyCode::KeyW),
            Some(Action::MoveForward)
        );
        assert_eq!(action_for_key(winit::keyboard::KeyCode::F12), None);
    }
}
