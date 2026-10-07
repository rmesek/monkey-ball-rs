//! TPL/texture container parser and GX texture format decoders.
//!
//! Ports `WebMonkeyBall/src/tpl.ts` and `noclip/SuperMonkeyBall/AVTpl.ts`. Input is the already
//! LZSS-decompressed container. Decoding mirrors the reference bit for bit, including its
//! float-then-truncate colour maths and its behaviour for textures smaller than one tile.

use std::fmt;

/// GX texture formats (values as stored in the TPL header).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    I4,
    I8,
    IA4,
    IA8,
    Rgb565,
    Rgb5A3,
    Rgba8,
    C4,
    C8,
    C14X2,
    Cmpr,
    /// Any other raw value; never decodable.
    Unknown(u32),
}

impl Format {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::I4,
            1 => Self::I8,
            2 => Self::IA4,
            3 => Self::IA8,
            4 => Self::Rgb565,
            5 => Self::Rgb5A3,
            6 => Self::Rgba8,
            8 => Self::C4,
            9 => Self::C8,
            10 => Self::C14X2,
            14 => Self::Cmpr,
            o => Self::Unknown(o),
        }
    }

    /// (tile width, tile height, bits per pixel) for formats the reference decodes.
    /// The paletted formats are not handled by the reference either (AV TPLs carry no palette).
    fn tile_info(self) -> Option<(usize, usize, usize)> {
        Some(match self {
            Self::I4 | Self::Cmpr => (8, 8, 4),
            Self::I8 | Self::IA4 => (8, 4, 8),
            Self::IA8 | Self::Rgb565 | Self::Rgb5A3 => (4, 4, 16),
            Self::Rgba8 => (4, 4, 32),
            _ => return None,
        })
    }

    pub fn is_decodable(self) -> bool {
        self.tile_info().is_some()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TplError {
    /// Container or a header ends before the data it declares.
    Truncated,
    /// Header magic at +0x0E is not 0x1234 (texture index given).
    BadHeader(usize),
    /// A texture's data offset points outside the file.
    BadOffset(usize),
    UnsupportedFormat(Format),
    /// Requested mip level is >= the texture's level count.
    NoSuchLevel(usize),
    /// Texture dimensions would need an absurd allocation.
    TooLarge,
}

impl fmt::Display for TplError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "TPL truncated"),
            Self::BadHeader(i) => write!(f, "invalid TPL header for texture {i}"),
            Self::BadOffset(i) => write!(f, "texture {i} data offset outside file"),
            Self::UnsupportedFormat(x) => write!(f, "unsupported texture format {x:?}"),
            Self::NoSuchLevel(l) => write!(f, "no mip level {l}"),
            Self::TooLarge => write!(f, "texture too large"),
        }
    }
}

impl std::error::Error for TplError {}

/// One texture entry. Holds a copy of the file from its data offset onward, as the reference
/// does (mip levels follow each other).
#[derive(Clone, Debug)]
pub struct Texture {
    /// Index in the container (gaps exist where entries are empty).
    pub index: usize,
    pub format: Format,
    pub width: usize,
    pub height: usize,
    /// Mip count as stored in the header (0 is treated as 1 when decoding).
    pub mip_count: usize,
    pub data_offset: usize,
    data: Vec<u8>,
}

/// A decoded mip level, RGBA8, row-major, `width * height * 4` bytes.
#[derive(Clone, Debug)]
pub struct Level {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Tpl {
    textures: Vec<Texture>,
    /// Entry count declared by the container (including empty entries).
    pub entry_count: usize,
}

fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Parse an AV TPL container (decompressed). Empty entries (0x0 with 0 mips) are skipped.
pub fn parse(bytes: &[u8]) -> Result<Tpl, TplError> {
    let count = be32(bytes, 0).ok_or(TplError::Truncated)? as usize;
    if count > bytes.len().saturating_sub(4) / 0x10 {
        return Err(TplError::Truncated);
    }
    let mut textures = Vec::new();
    for i in 0..count {
        let h = 4 + i * 0x10;
        if be16(bytes, h + 0x0e).ok_or(TplError::Truncated)? != 0x1234 {
            return Err(TplError::BadHeader(i));
        }
        let format = Format::from_raw(be32(bytes, h).ok_or(TplError::Truncated)?);
        let offs = be32(bytes, h + 4).ok_or(TplError::Truncated)? as usize;
        let width = be16(bytes, h + 8).ok_or(TplError::Truncated)? as usize;
        let height = be16(bytes, h + 10).ok_or(TplError::Truncated)? as usize;
        let mip_count = be16(bytes, h + 12).ok_or(TplError::Truncated)? as usize;
        if width == 0 && height == 0 && mip_count == 0 {
            continue;
        }
        if offs > bytes.len() {
            return Err(TplError::BadOffset(i));
        }
        textures.push(Texture {
            index: i,
            format,
            width,
            height,
            mip_count,
            data_offset: offs,
            data: bytes[offs..].to_vec(),
        });
    }
    Ok(Tpl {
        textures,
        entry_count: count,
    })
}

impl Tpl {
    pub fn textures(&self) -> &[Texture] {
        &self.textures
    }

    /// Texture by container index.
    pub fn get(&self, index: usize) -> Option<&Texture> {
        self.textures.iter().find(|t| t.index == index)
    }
}

impl Texture {
    /// Reference name: `<tpl name>_<index padded to 3>`.
    pub fn name(&self, tpl_name: &str) -> String {
        format!("{tpl_name}_{:03}", self.index)
    }

    /// Number of levels decoded by the reference (`max(1, mip_count)`).
    pub fn level_count(&self) -> usize {
        self.mip_count.max(1)
    }

    /// Width and height of mip `level` (halved each level, clamped to 1).
    pub fn level_size(&self, level: usize) -> (usize, usize) {
        let (mut w, mut h) = (self.width, self.height);
        for _ in 0..level {
            w = (w >> 1).max(1);
            h = (h >> 1).max(1);
        }
        (w, h)
    }

    /// Decode one mip level to RGBA8.
    pub fn decode_rgba(&self, level: usize) -> Result<Vec<u8>, TplError> {
        if level >= self.level_count() {
            return Err(TplError::NoSuchLevel(level));
        }
        if !self.format.is_decodable() {
            return Err(TplError::UnsupportedFormat(self.format));
        }
        let mut offs = 0usize;
        for l in 0..level {
            let (w, h) = self.level_size(l);
            offs += level_bytes(self.format, w, h);
        }
        let (w, h) = self.level_size(level);
        decode_level(self.format, w, h, &self.data, offs)
    }

    /// Decode every level.
    pub fn decode_all(&self) -> Result<Vec<Level>, TplError> {
        (0..self.level_count())
            .map(|l| {
                let (width, height) = self.level_size(l);
                Ok(Level {
                    width,
                    height,
                    rgba: self.decode_rgba(l)?,
                })
            })
            .collect()
    }
}

fn level_bytes(format: Format, w: usize, h: usize) -> usize {
    match format.tile_info() {
        Some((tw, th, bpp)) => w.div_ceil(tw) * h.div_ceil(th) * tw * th * bpp / 8,
        None => 0,
    }
}

/// Destination writer reproducing JS typed-array semantics: out-of-range (including negative)
/// indices are silently dropped.
struct Dst<'a> {
    buf: &'a mut [u8],
}

impl Dst<'_> {
    #[inline]
    fn put(&mut self, idx: isize, v: u8) {
        if idx >= 0 {
            if let Some(p) = self.buf.get_mut(idx as usize) {
                *p = v;
            }
        }
    }
    #[inline]
    fn px(&mut self, idx: isize, c: [u8; 4]) {
        for (k, v) in c.into_iter().enumerate() {
            self.put(idx + k as isize, v);
        }
    }
}

/// Source reader: out-of-range reads yield 0 (JS would give NaN, stored as 0).
#[inline]
fn rd(src: &[u8], i: usize) -> u8 {
    src.get(i).copied().unwrap_or(0)
}
#[inline]
fn rd16(src: &[u8], i: usize) -> u16 {
    ((rd(src, i) as u16) << 8) | rd(src, i + 1) as u16
}

// The reference keeps colours as floats and truncates when storing into a Uint8Array, so the
// same f64 expressions are used here (`as u8` truncates toward zero).
fn rgb565_f(v: u16) -> [f64; 3] {
    let v = v as u32;
    [
        ((v >> 11) & 0x1f) as f64 * 255.0 / 31.0,
        ((v >> 5) & 0x3f) as f64 * 255.0 / 63.0,
        (v & 0x1f) as f64 * 255.0 / 31.0,
    ]
}

fn rgb565(v: u16) -> [u8; 4] {
    let [r, g, b] = rgb565_f(v);
    [r as u8, g as u8, b as u8, 255]
}

fn rgb5a3(v: u16) -> [u8; 4] {
    let v = v as u32;
    if v & 0x8000 != 0 {
        [
            (((v >> 10) & 0x1f) as f64 * 255.0 / 31.0) as u8,
            (((v >> 5) & 0x1f) as f64 * 255.0 / 31.0) as u8,
            ((v & 0x1f) as f64 * 255.0 / 31.0) as u8,
            255,
        ]
    } else {
        [
            (((v >> 8) & 0xf) as f64 * 255.0 / 15.0) as u8,
            (((v >> 4) & 0xf) as f64 * 255.0 / 15.0) as u8,
            ((v & 0xf) as f64 * 255.0 / 15.0) as u8,
            (((v >> 12) & 0x7) as f64 * 255.0 / 7.0) as u8,
        ]
    }
}

fn cmpr_block(dst: &mut Dst, pos: isize, stride: isize, src: &[u8], s: usize) {
    let c1 = rd16(src, s);
    let c2 = rd16(src, s + 2);
    let [r1, g1, b1] = rgb565_f(c1);
    let [r2, g2, b2] = rgb565_f(c2);
    let t = |x: f64| x as u8;
    let col0 = [t(r1), t(g1), t(b1), 255];
    let col1 = [t(r2), t(g2), t(b2), 255];
    let (col2, col3) = if c1 > c2 {
        (
            [
                t((2.0 * r1 + r2) / 3.0),
                t((2.0 * g1 + g2) / 3.0),
                t((2.0 * b1 + b2) / 3.0),
                255,
            ],
            [
                t((r1 + 2.0 * r2) / 3.0),
                t((g1 + 2.0 * g2) / 3.0),
                t((b1 + 2.0 * b2) / 3.0),
                255,
            ],
        )
    } else {
        (
            [
                t((r1 + r2) / 2.0),
                t((g1 + g2) / 2.0),
                t((b1 + b2) / 2.0),
                255,
            ],
            [t(r2), t(g2), t(b2), 0],
        )
    };
    let cols = [col0, col1, col2, col3];
    for y in 0..4usize {
        let mut val = rd(src, s + 4 + y);
        for x in 0..4isize {
            let idx = (val >> 6) & 3;
            val <<= 2;
            dst.px(pos + y as isize * stride + x * 4, cols[idx as usize]);
        }
    }
}

fn decode_tile(f: Format, dst: &mut Dst, pos: isize, stride: isize, src: &[u8], s: usize) {
    let mut d = pos;
    let mut si = s;
    match f {
        Format::I4 => {
            for _ in 0..8 {
                for x in 0..8 {
                    let byte = rd(src, si);
                    let v = if x & 1 == 0 { byte >> 4 } else { byte & 0xf };
                    let i = (v << 4) | v;
                    dst.px(d, [i, i, i, 255]);
                    d += 4;
                    if x & 1 == 1 {
                        si += 1;
                    }
                }
                d += stride - 32;
            }
        }
        Format::I8 => {
            for _ in 0..4 {
                for _ in 0..8 {
                    let i = rd(src, si);
                    si += 1;
                    dst.px(d, [i, i, i, 255]);
                    d += 4;
                }
                d += stride - 32;
            }
        }
        Format::IA4 => {
            for _ in 0..4 {
                for _ in 0..8 {
                    let b = rd(src, si);
                    si += 1;
                    let i = (b & 0xf) * 17;
                    dst.px(d, [i, i, i, (b >> 4) * 17]);
                    d += 4;
                }
                d += stride - 32;
            }
        }
        Format::IA8 => {
            for _ in 0..4 {
                for _ in 0..4 {
                    let i = rd(src, si);
                    let a = rd(src, si + 1);
                    si += 2;
                    dst.px(d, [i, i, i, a]);
                    d += 4;
                }
                d += stride - 16;
            }
        }
        Format::Rgb565 | Format::Rgb5A3 => {
            for _ in 0..4 {
                for _ in 0..4 {
                    let v = rd16(src, si);
                    si += 2;
                    let c = if f == Format::Rgb565 {
                        rgb565(v)
                    } else {
                        rgb5a3(v)
                    };
                    dst.px(d, c);
                    d += 4;
                }
                d += stride - 16;
            }
        }
        Format::Rgba8 => {
            for y in 0..4 {
                for x in 0..4 {
                    let m = s + y * 8 + x * 2;
                    dst.px(
                        d,
                        [rd(src, m + 1), rd(src, m + 32), rd(src, m + 33), rd(src, m)],
                    );
                    d += 4;
                }
                d += stride - 16;
            }
        }
        Format::Cmpr => {
            cmpr_block(dst, pos, stride, src, s);
            cmpr_block(dst, pos + 16, stride, src, s + 8);
            cmpr_block(dst, pos + 4 * stride, stride, src, s + 16);
            cmpr_block(dst, pos + 16 + 4 * stride, stride, src, s + 24);
        }
        _ => {}
    }
}

/// Decode one level of `format` from `src` starting at `src_offset` (`decodeTplLevel`).
pub fn decode_level(
    format: Format,
    width: usize,
    height: usize,
    src: &[u8],
    src_offset: usize,
) -> Result<Vec<u8>, TplError> {
    let (tw, th, bpp) = format
        .tile_info()
        .ok_or(TplError::UnsupportedFormat(format))?;
    let len = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= 1 << 30)
        .ok_or(TplError::TooLarge)?;
    let mut out = vec![0u8; len];
    let mut dst = Dst { buf: &mut out };
    let stride = (width * 4) as isize;
    let tile_bytes = tw * th * bpp / 8;
    let mut sp = src_offset;
    for ty in (0..height).step_by(th) {
        for tx in (0..width).step_by(tw) {
            let pos = ((ty * width + tx) * 4) as isize;
            decode_tile(format, &mut dst, pos, stride, src, sp);
            sp += tile_bytes;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
