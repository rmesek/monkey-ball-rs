//! Input abstraction. The game reads logical [`Action`]s and [`Axis`] values from [`InputState`]; platform
//! shells (winit keyboard/mouse today; gamepad, touch and tilt later) just feed it.

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveForward,
    MoveBack,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    Fast,
    LookLeft,
    LookRight,
    LookUp,
    LookDown,
    Confirm,
    Cancel,
    Pause,
    NextStage,
    PrevStage,
    ResetCamera,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    MoveX,
    MoveY,
    LookX,
    LookY,
    /// Device tilt (accelerometer), reserved for the Android/web shells.
    TiltX,
    TiltY,
}

impl Axis {
    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Default)]
pub struct InputState {
    held: HashSet<Action>,
    pressed: HashSet<Action>,
    axes: [f32; 6],
    mouse_delta: (f32, f32),
    mouse_look: bool,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_action(&mut self, action: Action, down: bool) {
        if down {
            if self.held.insert(action) {
                self.pressed.insert(action);
            }
        } else {
            self.held.remove(&action);
        }
    }

    /// Analog value in -1..=1 (gamepad sticks, touch joystick, tilt).
    pub fn set_axis(&mut self, axis: Axis, value: f32) {
        self.axes[axis.index()] = value.clamp(-1.0, 1.0);
    }

    pub fn axis(&self, axis: Axis) -> f32 {
        self.axes[axis.index()]
    }

    pub fn add_mouse_delta(&mut self, dx: f32, dy: f32) {
        self.mouse_delta.0 += dx;
        self.mouse_delta.1 += dy;
    }

    pub fn set_mouse_look(&mut self, on: bool) {
        self.mouse_look = on;
    }

    pub fn held(&self, action: Action) -> bool {
        self.held.contains(&action)
    }

    /// True only on the frame the action went down.
    pub fn pressed(&self, action: Action) -> bool {
        self.pressed.contains(&action)
    }

    /// Digital pair combined with the analog axis, clamped to -1..=1.
    pub fn axis_with_keys(&self, axis: Axis, negative: Action, positive: Action) -> f32 {
        let digital = self.held(positive) as i32 as f32 - self.held(negative) as i32 as f32;
        (digital + self.axes[axis.index()]).clamp(-1.0, 1.0)
    }

    /// Mouse motion since the last frame, only while mouse-look is active.
    pub fn look_delta(&self) -> (f32, f32) {
        if self.mouse_look {
            self.mouse_delta
        } else {
            (0.0, 0.0)
        }
    }

    /// Call after each update to clear per-frame state.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.mouse_delta = (0.0, 0.0);
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
        K::KeyE | K::Space => Action::MoveUp,
        K::KeyQ | K::ControlLeft => Action::MoveDown,
        K::ShiftLeft | K::ShiftRight => Action::Fast,
        K::ArrowLeft => Action::LookLeft,
        K::ArrowRight => Action::LookRight,
        K::ArrowUp => Action::LookUp,
        K::ArrowDown => Action::LookDown,
        K::Enter => Action::Confirm,
        K::Escape => Action::Cancel,
        K::KeyP => Action::Pause,
        K::PageUp | K::BracketRight => Action::NextStage,
        K::PageDown | K::BracketLeft => Action::PrevStage,
        K::KeyR => Action::ResetCamera,
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
    fn digital_and_analog_combine_and_clamp() {
        let mut i = InputState::new();
        i.set_action(Action::MoveRight, true);
        i.set_axis(Axis::MoveX, 0.5);
        assert_eq!(
            i.axis_with_keys(Axis::MoveX, Action::MoveLeft, Action::MoveRight),
            1.0
        );
        i.set_action(Action::MoveRight, false);
        assert_eq!(
            i.axis_with_keys(Axis::MoveX, Action::MoveLeft, Action::MoveRight),
            0.5
        );
    }

    #[test]
    fn mouse_delta_only_counts_in_look_mode() {
        let mut i = InputState::new();
        i.add_mouse_delta(3.0, 4.0);
        assert_eq!(i.look_delta(), (0.0, 0.0));
        i.set_mouse_look(true);
        assert_eq!(i.look_delta(), (3.0, 4.0));
        i.end_frame();
        assert_eq!(i.look_delta(), (0.0, 0.0));
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
