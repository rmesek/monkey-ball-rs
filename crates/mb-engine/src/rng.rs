//! Port of `src/rng.ts`: xorshift32.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeterministicRng {
    pub state: u32,
}

impl DeterministicRng {
    pub fn new(seed: u32) -> Self {
        DeterministicRng {
            state: if seed == 0 { 0x6d2b_79f5 } else { seed },
        }
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    pub fn next_float(&mut self) -> f64 {
        (self.next_u32() >> 8) as f64 * (1.0 / 16777216.0)
    }

    pub fn next_s16(&mut self) -> i32 {
        ((self.next_u32() >> 17) & 0x7fff) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xorshift32_sequence() {
        let mut r = DeterministicRng::new(1);
        assert_eq!(r.next_u32(), 270369);
        assert_eq!(r.next_u32(), 67634689);
        assert_eq!(DeterministicRng::new(0).state, 0x6d2b_79f5);
    }

    #[test]
    fn float_and_s16_ranges() {
        let mut r = DeterministicRng::new(12345);
        for _ in 0..1000 {
            let f = r.next_float();
            assert!((0.0..1.0).contains(&f));
            let s = r.next_s16();
            assert!((0..0x8000).contains(&s));
        }
    }
}
