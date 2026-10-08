//! Bounds-checked big-endian reader helpers.

use crate::Error;

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    pub data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub fn bytes(&self, off: usize, len: usize) -> Result<&'a [u8], Error> {
        let end = off
            .checked_add(len)
            .ok_or(Error::OutOfBounds { offset: off, len })?;
        self.data
            .get(off..end)
            .ok_or(Error::OutOfBounds { offset: off, len })
    }

    fn arr<const N: usize>(&self, off: usize) -> Result<[u8; N], Error> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.bytes(off, N)?);
        Ok(a)
    }

    pub fn u8(&self, off: usize) -> Result<u8, Error> {
        Ok(self.bytes(off, 1)?[0])
    }
    pub fn i8(&self, off: usize) -> Result<i8, Error> {
        Ok(self.u8(off)? as i8)
    }
    pub fn u16(&self, off: usize) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.arr(off)?))
    }
    pub fn i16(&self, off: usize) -> Result<i16, Error> {
        Ok(i16::from_be_bytes(self.arr(off)?))
    }
    pub fn u32(&self, off: usize) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.arr(off)?))
    }
    pub fn i32(&self, off: usize) -> Result<i32, Error> {
        Ok(i32::from_be_bytes(self.arr(off)?))
    }
    pub fn f32(&self, off: usize) -> Result<f32, Error> {
        Ok(f32::from_be_bytes(self.arr(off)?))
    }
    pub fn vec3(&self, off: usize) -> Result<[f32; 3], Error> {
        Ok([self.f32(off)?, self.f32(off + 4)?, self.f32(off + 8)?])
    }
    /// NUL-terminated string starting at `off`, limited to `data[..limit]`.
    pub fn cstr(&self, off: usize, limit: usize) -> String {
        let limit = limit.min(self.data.len());
        let slice = self.data.get(off..limit).unwrap_or(&[]);
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        String::from_utf8_lossy(&slice[..end]).into_owned()
    }
}

/// Cap on decoded output. Models are found through offsets, so a corrupt archive can make thousands of table
/// entries share one model (or one long name); without a cap the decoded size grows with the square of the
/// file size. Each parser charges what it is about to allocate and fails once the cap is passed.
pub struct Budget {
    left: usize,
}

impl Budget {
    /// `per_byte` decoded bytes for every input byte, plus 64 KiB so tiny files are not special.
    pub fn new(input_len: usize, per_byte: usize) -> Self {
        Self {
            left: input_len.saturating_mul(per_byte).saturating_add(0x1_0000),
        }
    }

    pub fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        self.left = self.left.checked_sub(bytes).ok_or(Error::Invalid(
            "entries overlap: the file describes more data than it can hold",
        ))?;
        Ok(())
    }

    /// `count` records of `size` bytes each.
    pub fn charge_n(&mut self, count: usize, size: usize) -> Result<(), Error> {
        self.charge(count.saturating_mul(size))
    }
}
