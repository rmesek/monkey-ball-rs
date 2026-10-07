use super::*;

fn px(rgba: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
    let i = (y * w + x) * 4;
    rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn i4_tile() {
    let mut src = [0u8; 32];
    src[0] = 0x1F; // pixels (0,0)=1, (1,0)=15
    src[4] = 0xA0; // row 1 starts at byte 4: (0,1)=10
    let out = decode_level(Format::I4, 8, 8, &src, 0).unwrap();
    assert_eq!(px(&out, 8, 0, 0), [0x11, 0x11, 0x11, 255]);
    assert_eq!(px(&out, 8, 1, 0), [255, 255, 255, 255]);
    assert_eq!(px(&out, 8, 2, 0), [0, 0, 0, 255]);
    assert_eq!(px(&out, 8, 0, 1), [0xAA, 0xAA, 0xAA, 255]);
}

#[test]
fn i8_tile() {
    let src: Vec<u8> = (0..32).collect();
    let out = decode_level(Format::I8, 8, 4, &src, 0).unwrap();
    assert_eq!(px(&out, 8, 3, 2), [19, 19, 19, 255]);
    assert_eq!(px(&out, 8, 7, 3), [31, 31, 31, 255]);
}

#[test]
fn ia4_tile() {
    let mut src = [0u8; 32];
    src[0] = 0x8F; // alpha nibble 8, intensity nibble 15
    src[9] = 0x31; // (1,1): alpha 3, intensity 1
    let out = decode_level(Format::IA4, 8, 4, &src, 0).unwrap();
    assert_eq!(px(&out, 8, 0, 0), [255, 255, 255, 136]);
    assert_eq!(px(&out, 8, 1, 1), [17, 17, 17, 51]);
}

#[test]
fn ia8_tile() {
    let mut src = [0u8; 32];
    src[0] = 10;
    src[1] = 20; // intensity, alpha
    src[2 * 5] = 7; // pixel index 5 = (1,1)
    src[2 * 5 + 1] = 9;
    let out = decode_level(Format::IA8, 4, 4, &src, 0).unwrap();
    assert_eq!(px(&out, 4, 0, 0), [10, 10, 10, 20]);
    assert_eq!(px(&out, 4, 1, 1), [7, 7, 7, 9]);
}

#[test]
fn rgb565_tile() {
    let vals: [u16; 4] = [0xF800, 0x07E0, 0x001F, 0x8410];
    let mut src = [0u8; 32];
    for (i, v) in vals.iter().enumerate() {
        src[i * 2..i * 2 + 2].copy_from_slice(&v.to_be_bytes());
    }
    let out = decode_level(Format::Rgb565, 4, 4, &src, 0).unwrap();
    assert_eq!(px(&out, 4, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px(&out, 4, 1, 0), [0, 255, 0, 255]);
    assert_eq!(px(&out, 4, 2, 0), [0, 0, 255, 255]);
    // 16*255/31 = 131.6 -> 131 ; 32*255/63 = 129.5 -> 129 (truncated, like Uint8Array stores)
    assert_eq!(px(&out, 4, 3, 0), [131, 129, 131, 255]);
}

#[test]
fn rgb5a3_tile() {
    let vals: [u16; 3] = [0x8000 | (31 << 10), 0x7F0F, 0x2123];
    let mut src = [0u8; 32];
    for (i, v) in vals.iter().enumerate() {
        src[i * 2..i * 2 + 2].copy_from_slice(&v.to_be_bytes());
    }
    let out = decode_level(Format::Rgb5A3, 4, 4, &src, 0).unwrap();
    assert_eq!(px(&out, 4, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px(&out, 4, 1, 0), [255, 0, 255, 255]);
    // a = 2*255/7 = 72.86 -> 72, r = 17, g = 34, b = 51
    assert_eq!(px(&out, 4, 2, 0), [17, 34, 51, 72]);
}

#[test]
fn rgba8_tile() {
    let mut src = [0u8; 64];
    // pixel (1,0): AR pair at 2..4, GB pair at 34..36
    src[2] = 0x80;
    src[3] = 0x11;
    src[34] = 0x22;
    src[35] = 0x33;
    // pixel (3,2): AR at 2*8+6 = 22, GB at 54
    src[22] = 1;
    src[23] = 2;
    src[54] = 3;
    src[55] = 4;
    let out = decode_level(Format::Rgba8, 4, 4, &src, 0).unwrap();
    assert_eq!(px(&out, 4, 1, 0), [0x11, 0x22, 0x33, 0x80]);
    assert_eq!(px(&out, 4, 3, 2), [2, 3, 4, 1]);
}

#[test]
fn cmpr_tile() {
    let mut src = [0u8; 32];
    // block 0: red > blue -> 4-colour mode
    src[0..4].copy_from_slice(&[0xF8, 0x00, 0x00, 0x1F]);
    src[4] = 0b00_01_10_11;
    // block 1 (top right): c1 <= c2 -> 3-colour + transparent
    src[8..12].copy_from_slice(&[0x00, 0x00, 0x00, 0x1F]);
    src[12] = 0b11_10_00_01;
    let out = decode_level(Format::Cmpr, 8, 8, &src, 0).unwrap();
    assert_eq!(px(&out, 8, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px(&out, 8, 1, 0), [0, 0, 255, 255]);
    assert_eq!(px(&out, 8, 2, 0), [170, 0, 85, 255]);
    assert_eq!(px(&out, 8, 3, 0), [85, 0, 170, 255]);
    assert_eq!(px(&out, 8, 0, 1), [255, 0, 0, 255]); // index 0 rows
    assert_eq!(px(&out, 8, 4, 0), [0, 0, 255, 0]); // idx 3: c2 with alpha 0
    assert_eq!(px(&out, 8, 5, 0), [0, 0, 127, 255]); // idx 2: midpoint 127.5 -> 127
    assert_eq!(px(&out, 8, 6, 0), [0, 0, 0, 255]);
    assert_eq!(px(&out, 8, 7, 0), [0, 0, 255, 255]);
}

#[test]
fn multi_tile_placement() {
    // 8x4 RGB565 = two 4x4 tiles side by side; the second tile's first pixel lands at x=4.
    let mut src = [0u8; 64];
    src[32..34].copy_from_slice(&0xF800u16.to_be_bytes());
    let out = decode_level(Format::Rgb565, 8, 4, &src, 0).unwrap();
    assert_eq!(px(&out, 8, 4, 0), [255, 0, 0, 255]);
    assert_eq!(px(&out, 8, 0, 0), [0, 0, 0, 255]);
}

#[test]
fn smaller_than_tile_matches_reference_clipping() {
    // 1x1 I8: the reference writes the first pixel and drops everything out of range.
    let src = [200u8; 32];
    let out = decode_level(Format::I8, 1, 1, &src, 0).unwrap();
    assert_eq!(out, vec![200, 200, 200, 255]);
}

fn container(entries: &[(u32, u32, u16, u16, u16)], data: &[u8]) -> Vec<u8> {
    let mut v = (entries.len() as u32).to_be_bytes().to_vec();
    for &(f, o, w, h, m) in entries {
        v.extend_from_slice(&f.to_be_bytes());
        v.extend_from_slice(&o.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&m.to_be_bytes());
        v.extend_from_slice(&0x1234u16.to_be_bytes());
    }
    v.extend_from_slice(data);
    v
}

#[test]
fn parse_and_mipmaps() {
    // I8 8x8 with 2 mips: level 0 = 64 bytes, level 1 (4x4, padded to an 8x4 tile) = 32 bytes.
    let mut data: Vec<u8> = (0..64).collect();
    data.extend((100..132).collect::<Vec<u8>>());
    let base = 4 + 3 * 0x10;
    let bytes = container(
        &[
            (1, base as u32, 8, 8, 2),
            (0, 0, 0, 0, 0), // empty entry is skipped
            (4, base as u32, 4, 4, 0),
        ],
        &data,
    );
    let tpl = parse(&bytes).unwrap();
    assert_eq!(tpl.entry_count, 3);
    assert_eq!(tpl.textures().len(), 2);
    assert!(tpl.get(1).is_none());
    let t = tpl.get(0).unwrap();
    assert_eq!(
        (t.format, t.width, t.height, t.mip_count),
        (Format::I8, 8, 8, 2)
    );
    assert_eq!(t.name("foo"), "foo_000");
    assert_eq!(t.level_size(1), (4, 4));
    let l0 = t.decode_rgba(0).unwrap();
    assert_eq!(px(&l0, 8, 0, 4), [32, 32, 32, 255]); // second 8x4 tile row
    let l1 = t.decode_rgba(1).unwrap();
    assert_eq!(l1.len(), 4 * 4 * 4);
    assert_eq!(px(&l1, 4, 0, 0), [100, 100, 100, 255]);
    assert_eq!(px(&l1, 4, 3, 1), [111, 111, 111, 255]); // tile is 8 wide, row 1 starts at +8 -> 108+3
    assert_eq!(t.decode_rgba(2), Err(TplError::NoSuchLevel(2)));
    assert_eq!(t.decode_all().unwrap().len(), 2);
    // mip_count 0 still decodes one level
    let t2 = tpl.get(2).unwrap();
    assert_eq!(t2.level_count(), 1);
    assert!(t2.decode_rgba(0).is_ok());
}

#[test]
fn unsupported_and_errors() {
    let bytes = container(&[(8, 20, 8, 8, 1)], &[0u8; 64]);
    let tpl = parse(&bytes).unwrap();
    let t = &tpl.textures()[0];
    assert_eq!(t.format, Format::C4);
    assert_eq!(
        t.decode_rgba(0),
        Err(TplError::UnsupportedFormat(Format::C4))
    );
    assert_eq!(parse(&[]).unwrap_err(), TplError::Truncated);
    assert_eq!(parse(&[0, 0, 0, 5]).unwrap_err(), TplError::Truncated);
    let mut bad = container(&[(0, 20, 8, 8, 1)], &[0u8; 64]);
    bad[4 + 0x0e] = 0;
    assert_eq!(parse(&bad).unwrap_err(), TplError::BadHeader(0));
    let off = container(&[(0, 9999, 8, 8, 1)], &[0u8; 8]);
    assert_eq!(parse(&off).unwrap_err(), TplError::BadOffset(0));
}
