//! 2D drawing for the HUD and menus: the game's own sprite fonts and images, read from the disc's
//! `test/bmp/bmp_nml.tpl` and `bmp_com.tpl`, drawn through `Renderer::draw_sprite`.
//!
//! Font cell layouts are the reference's `FONT_PARAMS` (`hud.ts`). The reference loads PNGs exported from
//! those TPLs (`assets/hud/smb1/<tpl>/<index>_<name>.png`); here the textures are decoded from the TPLs at
//! run time, so nothing is shipped.

use mb_render::{Renderer, Sprite, TextureId};

use crate::content::Content;
use crate::ui::Rect;

pub type Color = [f32; 4];

pub const WHITE: Color = [1.0; 4];

/// `0xRRGGBB` to an opaque colour.
pub const fn rgb(hex: u32) -> Color {
    [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        1.0,
    ]
}

pub fn with_alpha(c: Color, a: f32) -> Color {
    [c[0], c[1], c[2], c[3] * a.clamp(0.0, 1.0)]
}

/// Glyph grid of a font texture (field for field `FontParams` of the reference).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontParams {
    pub space_width: f32,
    pub line_height: f32,
    pub first_char: u8,
    pub last_char: u8,
    pub columns: u32,
    pub u_step: f32,
    pub v_step: f32,
    pub u_margin: f32,
    pub v_margin: f32,
    /// Left, right, top, bottom in margin units.
    pub pad: [f32; 4],
}

#[allow(clippy::too_many_arguments)]
const fn params(
    space_width: f32,
    line_height: f32,
    chars: (u8, u8),
    columns: u32,
    step: (f32, f32),
    margin: (f32, f32),
    pad: [f32; 4],
) -> FontParams {
    FontParams {
        space_width,
        line_height,
        first_char: chars.0,
        last_char: chars.1,
        columns,
        u_step: step.0,
        v_step: step.1,
        u_margin: margin.0,
        v_margin: margin.1,
        pad,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontId {
    /// Timer seconds (digits only).
    Num24x37,
    /// Timer centiseconds (digits and `:`).
    Num12x19,
    /// Score digits and punctuation `'`..`?`.
    Num22x22,
    /// Upper-case ASCII ` `..`[` (HUD labels).
    Asc20,
    /// Upper-case ASCII ` `..`_` (menus, messages).
    Asc32,
    /// Big banner letters `0`..`` ` ``.
    Asc72,
    /// Full ASCII ` `..`z` (small print).
    Asc16,
}

impl FontId {
    const ALL: [FontId; 7] = [
        FontId::Num24x37,
        FontId::Num12x19,
        FontId::Num22x22,
        FontId::Asc20,
        FontId::Asc32,
        FontId::Asc72,
        FontId::Asc16,
    ];

    /// `(in bmp_com.tpl, texture index)`.
    fn source(self) -> (bool, usize) {
        match self {
            FontId::Num24x37 => (false, 12),
            FontId::Num12x19 => (false, 13),
            FontId::Num22x22 => (false, 5),
            FontId::Asc20 => (false, 9),
            FontId::Asc32 => (false, 7),
            FontId::Asc72 => (true, 80),
            FontId::Asc16 => (false, 6),
        }
    }

    pub fn params(self) -> FontParams {
        const Z: [f32; 4] = [0.0; 4];
        match self {
            FontId::Num24x37 => params(
                26.0,
                38.0,
                (48, 57),
                5,
                (0.191_176_47, 0.475),
                (0.007_352_941, 0.0125),
                Z,
            ),
            FontId::Num12x19 => params(
                16.0,
                22.0,
                (48, 59),
                6,
                (0.166_666_67, 0.458_333_34),
                (0.010_416_667, 0.020_833_334),
                Z,
            ),
            FontId::Num22x22 => params(
                22.0,
                22.0,
                (39, 63),
                5,
                (0.171_875, 0.171_875),
                (0.007_812_5, 0.007_812_5),
                [0.0, 0.0, 1.0, 0.0],
            ),
            FontId::Asc20 => params(
                20.0,
                20.0,
                (32, 91),
                12,
                (0.078_125, 0.078_125),
                (0.003_906_25, 0.003_906_25),
                Z,
            ),
            FontId::Asc32 => params(
                32.0,
                32.0,
                (32, 95),
                8,
                (0.125, 0.125),
                (0.003_906_25, 0.003_906_25),
                [0.0, 1.0, 0.0, 2.0],
            ),
            FontId::Asc72 => params(
                72.0,
                64.0,
                (48, 96),
                7,
                (0.140_625, 0.142_857_15),
                (0.001_953_125, 0.002_232_143),
                [0.0, 1.0, 0.0, 2.0],
            ),
            FontId::Asc16 => params(
                16.0,
                16.0,
                (32, 122),
                16,
                (0.062_5, 0.166_666_67),
                (0.003_906_25, 0.010_416_667),
                Z,
            ),
        }
    }
}

/// A texture with its pixel size.
#[derive(Clone, Copy, Debug)]
pub struct Image {
    pub tex: TextureId,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Font {
    pub image: Image,
    pub p: FontParams,
}

impl FontParams {
    /// `[u0, v0, u1, v1]` of a character, `None` outside the font's range.
    pub fn glyph_uv(&self, code: u8) -> Option<[f32; 4]> {
        if code < self.first_char || code > self.last_char {
            return None;
        }
        let index = (code - self.first_char) as u32;
        let (row, col) = ((index / self.columns) as f32, (index % self.columns) as f32);
        let u1 = self.u_step * col + self.u_margin * self.pad[0];
        let v1 = self.v_step * row + self.v_margin * self.pad[2];
        let u2 = self.u_step + (self.u_step * col - self.u_margin * self.pad[1]);
        let v2 = self.v_step + (self.v_step * row - self.v_margin * self.pad[3]);
        Some([u1, v1, u2, v2])
    }

    /// The character to draw for `c`: lower case falls back to upper case in upper-case-only fonts.
    pub fn resolve(&self, c: char) -> Option<u8> {
        let code = u8::try_from(c as u32).ok()?;
        if self.glyph_uv(code).is_some() {
            return Some(code);
        }
        let upper = code.to_ascii_uppercase();
        self.glyph_uv(upper).map(|_| upper)
    }
}

impl FontParams {
    /// Advance width of `text` (fixed pitch, like the reference's `measureText`).
    pub fn measure(&self, text: &str, scale: f32) -> f32 {
        text.chars().count() as f32 * self.space_width * scale
    }
}

impl Font {
    pub fn measure(&self, text: &str, scale: f32) -> f32 {
        self.p.measure(text, scale)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Everything the HUD and menus draw with. Load it before any stage so stage teardown
/// (`Renderer::truncate_textures`) leaves it alone.
pub struct UiAssets {
    fonts: [Font; 7],
    pub banana: Image,
    pub banana_bunch: Image,
    pub mph: Image,
    pub goal: Image,
    pub bomb: Image,
    pub bomb_crack: Image,
    pub bomb_parts: Vec<Image>,
    /// Difficulty icon sheet (4 x 2 cells of 64 px).
    pub lv_icon: Image,
    /// Player icon for the lives counter.
    pub face: Image,
    /// 1 x 1 white, for panels.
    pub white: Image,
    /// Soft-edged filled circle (computed, not game data), for the touch stick and buttons.
    pub disc: Image,
}

/// The texture sheets [`UiAssets::load`] reads (`test/bmp/<name>.tpl`): fonts and HUD pieces.
pub const UI_SHEETS: [&str; 2] = ["bmp_nml", "bmp_com"];

const DISC_SIZE: usize = 64;

/// Anti-aliased white disc with a slightly brighter rim.
fn disc_pixels() -> Vec<u8> {
    let mut px = vec![0u8; DISC_SIZE * DISC_SIZE * 4];
    let c = (DISC_SIZE as f32 - 1.0) / 2.0;
    let radius = c - 1.0;
    for y in 0..DISC_SIZE {
        for x in 0..DISC_SIZE {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let coverage = (radius - d + 0.5).clamp(0.0, 1.0);
            let rim = 1.0 - ((radius - d) / 5.0).clamp(0.0, 1.0);
            let alpha = coverage * (0.55 + 0.45 * rim);
            let o = (y * DISC_SIZE + x) * 4;
            px[o..o + 3].copy_from_slice(&[255, 255, 255]);
            px[o + 3] = (alpha * 255.0) as u8;
        }
    }
    px
}

impl UiAssets {
    pub fn load(r: &mut Renderer, content: &Content) -> Result<Self, String> {
        let tpl = |name: &str| -> Result<mb_tpl::Tpl, String> {
            let path = crate::content::ui_sheet_path(name);
            let bytes = content
                .read_maybe_lz(&path)
                .map_err(|e| format!("{path}: {e}"))?;
            mb_tpl::parse(&bytes).map_err(|e| format!("{path}: {e}"))
        };
        let nml = tpl(UI_SHEETS[0])?;
        let com = tpl(UI_SHEETS[1])?;
        let mut image = |from_com: bool, index: usize| -> Result<Image, String> {
            let (t, name) = if from_com {
                (&com, "bmp_com")
            } else {
                (&nml, "bmp_nml")
            };
            let tex = t
                .get(index)
                .ok_or_else(|| format!("{name}.tpl has no texture {index}"))?;
            let rgba = tex
                .decode_rgba(0)
                .map_err(|e| format!("{name}.tpl texture {index}: {e}"))?;
            let (w, h) = tex.level_size(0);
            Ok(Image {
                tex: r.upload_rgba(w as u32, h as u32, &rgba),
                w: w as f32,
                h: h as f32,
            })
        };
        let mut fonts = Vec::with_capacity(FontId::ALL.len());
        for id in FontId::ALL {
            let (from_com, index) = id.source();
            fonts.push(Font {
                image: image(from_com, index)?,
                p: id.params(),
            });
        }
        let fonts: [Font; 7] = fonts
            .try_into()
            .map_err(|_| "font table size".to_string())?;
        let banana = image(true, 12)?;
        let banana_bunch = image(true, 85)?;
        let mph = image(true, 88)?;
        let face = image(true, 3)?;
        let goal = image(false, 4)?;
        let bomb = image(false, 0)?;
        let bomb_crack = image(false, 16)?;
        let lv_icon = image(false, 1)?;
        let bomb_parts = (17..=26)
            .map(|i| image(false, i))
            .collect::<Result<Vec<_>, _>>()?;
        let white = Image {
            tex: r.upload_rgba(1, 1, &[255; 4]),
            w: 1.0,
            h: 1.0,
        };
        let disc = Image {
            tex: r.upload_rgba(DISC_SIZE as u32, DISC_SIZE as u32, &disc_pixels()),
            w: DISC_SIZE as f32,
            h: DISC_SIZE as f32,
        };
        Ok(Self {
            fonts,
            banana,
            banana_bunch,
            mph,
            goal,
            bomb,
            bomb_crack,
            bomb_parts,
            lv_icon,
            face,
            white,
            disc,
        })
    }

    pub fn font(&self, id: FontId) -> &Font {
        &self.fonts[FontId::ALL.iter().position(|f| *f == id).unwrap_or(0)]
    }

    /// Queue one sprite without size from every UI texture. The renderer creates what the sprites of a
    /// texture need (a bind group) when the first one is drawn; a stage does this in its first frame, so
    /// that the `GOAL` banner or the time-over bomb create nothing when they first appear in play.
    pub fn warm(&self, r: &mut Renderer) {
        let images = (self.fonts.iter().map(|f| f.image))
            .chain([
                self.banana,
                self.banana_bunch,
                self.mph,
                self.goal,
                self.bomb,
                self.bomb_crack,
                self.lv_icon,
                self.face,
                self.white,
                self.disc,
            ])
            .chain(self.bomb_parts.iter().copied());
        for image in images {
            r.draw_sprite(Sprite {
                texture: image.tex,
                dst: [0.0; 4],
                uv: [0.0; 4],
                color: [0.0; 4],
            });
        }
    }
}

/// Immediate-mode 2D drawing in target pixels (origin top-left). Draw order is submission order.
pub struct Painter<'a> {
    pub r: &'a mut Renderer,
    pub ui: &'a UiAssets,
}

impl Painter<'_> {
    pub fn rect(&mut self, rect: Rect, color: Color) {
        if color[3] <= 0.0 {
            return;
        }
        self.r.draw_sprite(Sprite {
            texture: self.ui.white.tex,
            dst: [rect.x, rect.y, rect.w, rect.h],
            uv: [0.0, 0.0, 1.0, 1.0],
            color,
        });
    }

    /// Outline of `rect`, `t` pixels thick, drawn inside it.
    pub fn frame(&mut self, rect: Rect, t: f32, color: Color) {
        let Rect { x, y, w, h } = rect;
        for (x, y, w, h) in [
            (x, y, w, t),
            (x, y + h - t, w, t),
            (x, y + t, t, h - 2.0 * t),
            (x + w - t, y + t, t, h - 2.0 * t),
        ] {
            self.rect(Rect { x, y, w, h }, color);
        }
    }

    /// `img` centred on `(cx, cy)`.
    pub fn image(&mut self, img: Image, cx: f32, cy: f32, sx: f32, sy: f32, color: Color) {
        self.image_part(img, [0.0, 0.0, img.w, img.h], cx, cy, sx, sy, color);
    }

    /// The source pixel rectangle `[x, y, w, h]` of `img`, centred on `(cx, cy)`.
    #[allow(clippy::too_many_arguments)]
    pub fn image_part(
        &mut self,
        img: Image,
        src: [f32; 4],
        cx: f32,
        cy: f32,
        sx: f32,
        sy: f32,
        color: Color,
    ) {
        if color[3] <= 0.0 {
            return;
        }
        let (w, h) = (src[2] * sx, src[3] * sy);
        self.r.draw_sprite(Sprite {
            texture: img.tex,
            dst: [cx - w / 2.0, cy - h / 2.0, w, h],
            uv: [
                src[0] / img.w,
                src[1] / img.h,
                (src[0] + src[2]) / img.w,
                (src[1] + src[3]) / img.h,
            ],
            color,
        });
    }

    /// One glyph with its left edge at `x`, vertically centred on `y` (the reference's `drawGlyph`).
    #[allow(clippy::too_many_arguments)]
    pub fn glyph(
        &mut self,
        font: FontId,
        code: u8,
        x: f32,
        y: f32,
        sx: f32,
        sy: f32,
        color: Color,
    ) {
        let f = *self.ui.font(font);
        let Some(uv) = f.p.glyph_uv(code) else {
            return;
        };
        if color[3] <= 0.0 {
            return;
        }
        let w = (uv[2] - uv[0]) * f.image.w * sx;
        let h = (uv[3] - uv[1]) * f.image.h * sy;
        self.r.draw_sprite(Sprite {
            texture: f.image.tex,
            dst: [x, y - h / 2.0, w, h],
            uv,
            color,
        });
    }

    /// A line of text, vertically centred on `y`; `x` is the left edge, centre or right edge per `align`.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        font: FontId,
        text: &str,
        x: f32,
        y: f32,
        sx: f32,
        sy: f32,
        color: Color,
        align: Align,
    ) {
        let f = *self.ui.font(font);
        let width = f.measure(text, sx);
        let mut x = match align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        for c in text.chars() {
            // The space cell of some sheets holds artwork ("SP"); a space only advances.
            if let Some(code) = f.p.resolve(c).filter(|code| *code != b' ') {
                self.glyph(font, code, x, y, sx, sy, color);
            }
            x += f.p.space_width * sx;
        }
    }

    pub fn measure(&self, font: FontId, text: &str, scale: f32) -> f32 {
        self.ui.font(font).measure(text, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_cells_follow_the_reference_formula() {
        let p = FontId::Asc20.params();
        // 'A' = 65 - 32 = 33 -> row 2, column 9 of a 12-column sheet.
        let uv = p.glyph_uv(b'A').unwrap();
        assert!((uv[0] - 0.078_125 * 9.0).abs() < 1e-6);
        assert!((uv[1] - 0.078_125 * 2.0).abs() < 1e-6);
        assert!((uv[2] - 0.078_125 * 10.0).abs() < 1e-6);
        assert!(p.glyph_uv(b'a').is_none());
        // 22x22 digits start at "'" and have one margin of top padding.
        let d = FontId::Num22x22.params();
        let zero = d.glyph_uv(b'0').unwrap();
        assert!((zero[0] - 0.171_875 * 4.0).abs() < 1e-6);
        assert!((zero[1] - (0.171_875 + 0.007_812_5)).abs() < 1e-6);
    }

    #[test]
    fn lower_case_falls_back_to_upper_case() {
        let p = FontId::Asc32.params();
        assert_eq!(p.resolve('a'), Some(b'A'));
        assert_eq!(p.resolve('Z'), Some(b'Z'));
        assert_eq!(FontId::Asc16.params().resolve('a'), Some(b'a'));
        assert_eq!(p.resolve('\u{e9}'), None);
        assert_eq!(FontId::Num24x37.params().resolve('x'), None);
    }

    #[test]
    fn colours_and_measure() {
        assert_eq!(rgb(0xff8000), [1.0, 128.0 / 255.0, 0.0, 1.0]);
        assert_eq!(with_alpha(WHITE, 2.0)[3], 1.0);
        assert_eq!(FontId::Asc20.params().measure("SCORE", 2.0), 200.0);
    }

    #[test]
    fn disc_is_opaque_in_the_middle_and_clear_in_the_corners() {
        let px = disc_pixels();
        let at = |x: usize, y: usize| px[(y * DISC_SIZE + x) * 4 + 3];
        assert_eq!(at(0, 0), 0);
        assert!(at(DISC_SIZE / 2, DISC_SIZE / 2) > 100);
        assert!(at(2, DISC_SIZE / 2) > at(DISC_SIZE / 2, DISC_SIZE / 2));
    }
}
