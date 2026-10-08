//! Port of `src/determinism.ts`: stick quantization (the unit stored in replays).

use crate::math::js_round;

/// Quantized stick input, each axis -127..=127.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuantizedInput {
    pub x: i32,
    pub y: i32,
}

/// Bit 0 of the reference's button bits (`Input.getButtonsBitmask`, the optional `buttons` of its
/// `QuantizedInput`): the primary action is held (`isPrimaryActionDown`: Space, Enter, Z, the touch action or
/// gamepad button 0). Passed to [`crate::Sim::step_with_buttons`].
pub const BUTTON_PRIMARY_ACTION: u32 = 1;

pub fn quantize_stick_axis(value: f64) -> i32 {
    let clamped = value.clamp(-1.0, 1.0);
    (js_round(clamped * 127.0) as i32).clamp(-127, 127)
}

pub fn dequantize_stick_axis(value: i32) -> f64 {
    value as f64 / 127.0
}

pub fn quantize_input(x: f64, y: f64) -> QuantizedInput {
    QuantizedInput {
        x: quantize_stick_axis(x),
        y: quantize_stick_axis(y),
    }
}

impl QuantizedInput {
    pub fn dequantize(self) -> (f64, f64) {
        (dequantize_stick_axis(self.x), dequantize_stick_axis(self.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_matches_js_round() {
        assert_eq!(quantize_stick_axis(1.0), 127);
        assert_eq!(quantize_stick_axis(-2.0), -127);
        assert_eq!(quantize_stick_axis(0.0), 0);
        // 0.5 * 127 = 63.5 -> JS Math.round gives 64, -0.5*127 = -63.5 -> -63
        assert_eq!(quantize_stick_axis(0.5), 64);
        assert_eq!(quantize_stick_axis(-0.5), -63);
        assert_eq!(
            quantize_input(0.25, -1.0),
            QuantizedInput { x: 32, y: -127 }
        );
    }

    #[test]
    fn dequantize_roundtrip() {
        assert_eq!(dequantize_stick_axis(127), 1.0);
        assert_eq!(QuantizedInput { x: -127, y: 0 }.dequantize(), (-1.0, 0.0));
    }
}
