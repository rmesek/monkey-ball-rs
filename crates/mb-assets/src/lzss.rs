//! LZSS decompressor for `.lz` files. Port of WebMonkeyBall `src/lzs.ts`.
//!
//! Format: `u32le compressed_size` (includes the 8-byte header), `u32le decompressed_size`,
//! then flag-byte groups (bit set = literal, clear = back-reference into a 4096-byte ring
//! buffer whose write position starts at 4078).

use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum LzssError {
    TooShort,
    BadHeader,
    /// The header promises more output than the input could ever expand to, or than any game file needs.
    TooLarge,
}

impl fmt::Display for LzssError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LzssError::TooShort => write!(f, "lzss: input shorter than 8-byte header"),
            LzssError::BadHeader => write!(f, "lzss: compressed size exceeds input"),
            LzssError::TooLarge => write!(f, "lzss: declared size is impossible for this input"),
        }
    }
}

impl std::error::Error for LzssError {}

/// Largest output accepted (the game's biggest `.lz` file expands to a few megabytes).
pub const MAX_OUTPUT: usize = 64 << 20;

/// Most bytes `payload_len` bytes of LZSS data can expand to: a flag byte and eight two-byte references of
/// eighteen bytes each.
fn max_expansion(payload_len: usize) -> usize {
    payload_len.div_ceil(17) * 8 * 18
}

/// Decompress `.lz` data. A header with a zero size yields an empty buffer, like the reference. The output
/// buffer is sized by the header, so a header that asks for more than the payload can produce is refused
/// instead of allocated.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, LzssError> {
    if input.len() < 8 {
        return Err(LzssError::TooShort);
    }
    let src_size = u32::from_le_bytes(input[0..4].try_into().unwrap()) as usize;
    let dest_size = u32::from_le_bytes(input[4..8].try_into().unwrap()) as usize;
    if src_size == 0 || dest_size == 0 {
        return Ok(Vec::new());
    }
    if src_size < 8 || src_size > input.len() {
        return Err(LzssError::BadHeader);
    }
    let src = &input[8..src_size];
    if dest_size > MAX_OUTPUT || dest_size > max_expansion(src.len()) {
        return Err(LzssError::TooLarge);
    }
    let mut dest = vec![0u8; dest_size];
    let mut ring = [0u8; 4096];
    let mut buf_pos = 4078usize;
    let mut flags = 0u32;
    let (mut srcp, mut destp) = (0usize, 0usize);

    // A macro (not a closure) so it can touch `dest`/`ring` without holding borrows across the loop.
    macro_rules! emit {
        ($byte:expr) => {{
            let byte: u8 = $byte;
            if destp < dest.len() {
                dest[destp] = byte;
            }
            ring[buf_pos] = byte;
            buf_pos = (buf_pos + 1) & 4095;
            destp += 1;
        }};
    }

    loop {
        flags >>= 1;
        if flags & 0x100 == 0 {
            if srcp >= src.len() {
                break;
            }
            flags = src[srcp] as u32 | 0xff00;
            srcp += 1;
        }
        if flags & 1 != 0 {
            if srcp >= src.len() {
                break;
            }
            let byte = src[srcp];
            srcp += 1;
            emit!(byte);
        } else {
            if srcp + 1 >= src.len() {
                break;
            }
            let mut offset = src[srcp] as usize;
            let r8 = src[srcp + 1] as usize;
            srcp += 2;
            let length = (r8 & 0x0f) + 2;
            offset |= (r8 & 0xf0) << 4;
            for i in 0..=length {
                let byte = ring[(offset + i) & 4095];
                emit!(byte);
            }
        }
        if destp >= dest.len() {
            break;
        }
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(payload: &[u8], dest_size: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&((payload.len() + 8) as u32).to_le_bytes());
        v.extend_from_slice(&dest_size.to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn literals() {
        let out = decompress(&wrap(&[0xFF, b'a', b'b', b'c'], 3)).unwrap();
        assert_eq!(out, b"abc");
    }

    #[test]
    fn back_reference_into_ring() {
        // 3 literals, then a match at ring offset 4078 (where they were written), 3 bytes long.
        let out = decompress(&wrap(&[0x07, b'a', b'b', b'c', 0xEE, 0xF0], 6)).unwrap();
        assert_eq!(out, b"abcabc");
    }

    #[test]
    fn zero_size_is_empty() {
        assert_eq!(decompress(&[0; 8]).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn rejects_short_and_bad() {
        assert_eq!(decompress(&[1, 2]), Err(LzssError::TooShort));
        assert_eq!(
            decompress(&[99, 0, 0, 0, 4, 0, 0, 0]),
            Err(LzssError::BadHeader)
        );
    }

    #[test]
    fn hostile_sizes_are_refused_without_allocating() {
        // 4 GiB promised from four bytes of payload.
        let bomb = wrap(&[0xFF, 1, 2, 3], u32::MAX);
        let (result, peak) = crate::test_alloc::peak_during(|| decompress(&bomb));
        assert_eq!(result, Err(LzssError::TooLarge));
        assert!(peak < 1 << 16, "allocated {peak} bytes");
        // Just over what the payload can expand to.
        let payload = [0u8; 17];
        assert!(decompress(&wrap(&payload, 8 * 18)).is_ok());
        assert_eq!(
            decompress(&wrap(&payload, 8 * 18 + 1)),
            Err(LzssError::TooLarge)
        );
        // Every truncation and every bit flip of a valid stream: a result, never a panic.
        let good = wrap(
            &[
                0x07, b'a', b'b', b'c', 0xEE, 0xF0, 0xFF, 1, 2, 3, 4, 5, 6, 7, 8,
            ],
            14,
        );
        for len in 0..good.len() {
            let _ = decompress(&good[..len]);
        }
        for at in 0..good.len() {
            for bit in 0..8 {
                let mut f = good.clone();
                f[at] ^= 1 << bit;
                let (_, peak) = crate::test_alloc::peak_during(|| decompress(&f));
                assert!(
                    peak < 1 << 20,
                    "byte {at} bit {bit}: allocated {peak} bytes"
                );
            }
        }
    }
}
