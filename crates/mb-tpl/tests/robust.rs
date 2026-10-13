//! The TPL parser and texture decoders on hostile input: an error or pixels, never a panic, and never more
//! heap than a small multiple of the input. Inputs are built in code; the last test repeats the campaign on
//! containers read from the user's disc (in memory only) and skips without one.

mod common;
use common::*;
use mb_tpl::{MAX_DIMENSION, TplError, parse};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// One shared copy of the file, plus the decoded levels of one texture at a time: at most 8 bytes of RGBA per
/// stored byte (4 bits per pixel formats).
fn heap_limit(len: usize) -> usize {
    len * 16 + (1 << 18)
}

/// Parse, then decode every level of every texture one texture at a time, as the renderer does. Returns the
/// number of decoded levels.
fn decode_everything(bytes: &[u8]) -> Result<usize, TplError> {
    let tpl = parse(bytes)?;
    let mut levels = 0;
    for t in tpl.textures() {
        assert!(t.level_count() >= 1 && t.level_count() <= 16);
        if let Ok(all) = t.decode_all() {
            for (i, l) in all.iter().enumerate() {
                assert_eq!((l.width, l.height), t.level_size(i));
                assert!(l.width >= 1 && l.width <= MAX_DIMENSION);
                assert!(l.height >= 1 && l.height <= MAX_DIMENSION);
                assert_eq!(l.rgba.len(), l.width * l.height * 4);
            }
            levels += all.len();
        }
    }
    Ok(levels)
}

fn checked(what: &str, bytes: &[u8]) -> Result<usize, TplError> {
    let (r, heap) = run_guarded(what, bytes, decode_everything);
    assert!(
        heap <= heap_limit(bytes.len()),
        "{what}: {heap} bytes of heap for {} bytes of input",
        bytes.len()
    );
    r
}

fn container(entries: &[(u32, u32, u16, u16, u16)], data_len: usize) -> Vec<u8> {
    let mut v = (entries.len() as u32).to_be_bytes().to_vec();
    for &(f, o, w, h, m) in entries {
        v.extend_from_slice(&f.to_be_bytes());
        v.extend_from_slice(&o.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&m.to_be_bytes());
        v.extend_from_slice(&0x1234u16.to_be_bytes());
    }
    let start = v.len();
    v.extend((0..data_len).map(|i| (i * 31 + 7) as u8));
    assert!(v.len() - start == data_len);
    v
}

/// Every decodable format, odd sizes, mip chains, an empty entry and a paletted (unsupported) one.
fn synthetic() -> Vec<u8> {
    let formats: [(u32, u16, u16, u16); 10] = [
        (0, 16, 8, 2),   // I4
        (1, 8, 8, 1),    // I8
        (2, 5, 3, 1),    // IA4, smaller than a tile
        (3, 8, 4, 1),    // IA8
        (4, 12, 8, 3),   // RGB565
        (5, 4, 4, 0),    // RGB5A3, mip count 0
        (6, 8, 8, 2),    // RGBA8
        (8, 8, 8, 1),    // C4: recognised, not decodable
        (14, 16, 16, 3), // CMPR
        (0, 0, 0, 0),    // empty entry
    ];
    let base = 4 + formats.len() as u32 * 0x10;
    let mut entries = Vec::new();
    let mut off = base;
    for (f, w, h, m) in formats {
        entries.push((f, if w == 0 { 0 } else { off }, w, h, m));
        // More than any of these textures needs, so entries do not overlap.
        off += if w == 0 { 0 } else { 0x200 };
    }
    container(&entries, (off - base) as usize)
}

#[test]
fn synthetic_container_is_valid() {
    let _g = serial();
    let src = synthetic();
    let tpl = parse(&src).unwrap();
    assert_eq!((tpl.entry_count, tpl.textures().len()), (10, 9));
    // 2 + 1 + 1 + 1 + 3 + 1 + 2 + 3 levels; the paletted entry fails to decode.
    assert_eq!(decode_everything(&src).unwrap(), 14);
    assert!(tpl.get(7).unwrap().decode_all().is_err());
}

#[test]
fn truncation_at_every_length() {
    let _g = serial();
    let src = synthetic();
    let full = decode_everything(&src).unwrap();
    for len in 0..src.len() {
        if let Ok(levels) = checked(&format!("truncation to {len}"), &src[..len]) {
            assert!(levels <= full);
        }
    }
    // Losing the last texture's data loses its levels: the decoder does not invent pixels.
    assert_eq!(decode_everything(&src[..src.len() - 0x1c0]), Ok(full - 3));
}

#[test]
fn every_header_byte_and_hostile_word() {
    let _g = serial();
    let src = synthetic();
    let header = 4 + 10 * 0x10;
    let mut b = src.clone();
    for i in 0..header {
        for v in (0..=255u8).step_by(3) {
            b[i] = v;
            let _ = checked(&format!("byte {i:#x} = {v:#x}"), &b);
        }
        b[i] = src[i];
    }
    for o in 0..header - 3 {
        for v in hostile_u32(src.len()) {
            b[o..o + 4].copy_from_slice(&v.to_be_bytes());
            let _ = checked(&format!("word {o:#x} = {v:#x}"), &b);
        }
        b[o..o + 4].copy_from_slice(&src[o..o + 4]);
    }
}

#[test]
fn random_corruption() {
    let _g = serial();
    let src = synthetic();
    let header = 4 + 10 * 0x10;
    let mut rng = Rng(0x5eed_0020);
    let (mut ok, mut err) = (0, 0);
    for i in 0..20_000 {
        // Half of the runs aim at the headers, where the structure is.
        let b = if i % 2 == 0 {
            let mut b = src.clone();
            let h = mutate(&src[..header], &mut rng);
            b[..header].copy_from_slice(&h);
            b
        } else {
            mutate(&src, &mut rng)
        };
        match checked(&format!("random mutation {i}"), &b) {
            Ok(_) => ok += 1,
            Err(_) => err += 1,
        }
    }
    assert!(ok > 1000 && err > 1000, "ok {ok}, err {err}");
}

/// Sizes a header can claim, for every format: zero in one direction, one pixel, not a multiple of the
/// tile, the GX maximum, just past it, and the 16-bit maximum, with mip counts past the 1x1 level.
#[test]
fn hostile_dimensions_and_mip_counts() {
    let _g = serial();
    let sizes = [0u16, 1, 2, 3, 7, 9, 1023, 1024, 1025, 4096, 0x8000, 0xffff];
    for format in [0u32, 1, 2, 3, 4, 5, 6, 14, 8, 9, 10, 7, 15, u32::MAX] {
        for &w in &sizes {
            for &h in &sizes {
                for mips in [0u16, 1, 11, 12, 0xffff] {
                    let bytes = container(&[(format, 20, w, h, mips)], 0x800);
                    let what = format!("format {format} {w}x{h} mips {mips}");
                    let levels = checked(&what, &bytes).unwrap();
                    let tpl = parse(&bytes).unwrap();
                    let Some(t) = tpl.get(0) else {
                        assert_eq!((w, h, mips), (0, 0, 0));
                        continue;
                    };
                    let bad = w == 0 || h == 0 || w > 1024 || h > 1024;
                    if bad {
                        assert_eq!(levels, 0, "{what}");
                        if t.format.is_decodable() {
                            assert_eq!(
                                t.decode_rgba(0),
                                Err(TplError::BadDimensions(w as usize, h as usize)),
                                "{what}"
                            );
                        }
                    }
                    // 0x800 bytes hold a 16x16 RGBA8 level and no more: bigger textures are truncated.
                    if !bad && w as usize * h as usize * 4 > 0x800 * 8 {
                        assert_eq!(levels, 0, "{what}");
                    }
                }
            }
        }
    }
}

/// A header alone must not buy megabytes: thousands of maximum-size textures with no pixel data behind them.
#[test]
fn headers_without_data_allocate_nothing() {
    let _g = serial();
    let n = 4096;
    let entries: Vec<_> = (0..n)
        .map(|_| (6u32, 4 + n as u32 * 0x10, 1024u16, 1024u16, 11u16))
        .collect();
    let bytes = container(&entries, 64);
    assert_eq!(checked("headers without data", &bytes), Ok(0));
    let tpl = parse(&bytes).unwrap();
    assert_eq!(tpl.textures()[0].decode_rgba(0), Err(TplError::Truncated));
}

/// Thousands of headers pointing at the same real megabyte: each would decode to megabytes of RGBA.
#[test]
fn overlapping_entries_are_rejected() {
    let _g = serial();
    let n = 4096u32;
    let entries: Vec<_> = (0..n)
        .map(|_| (14u32, 4 + n * 0x10, 1024u16, 1024u16, 1u16))
        .collect();
    let bytes = container(&entries, 1024 * 1024 / 2);
    assert_eq!(
        checked("overlapping entries", &bytes),
        Err(TplError::Overlapping)
    );
    // A handful of shared entries (the reference accepts them) still parse.
    let bytes = container(&[(14, 4 + 3 * 0x10, 1024, 1024, 1); 3], 1024 * 1024 / 2);
    let tpl = parse(&bytes).unwrap();
    assert_eq!(tpl.textures().len(), 3);
    assert_eq!(tpl.textures()[2].decode_rgba(0).unwrap().len(), 1 << 22);
}

#[test]
fn real_tpls_survive_the_same_campaign() {
    let _g = serial();
    let Some(disc) = open_disc() else { return };
    let paths = disc_paths(&disc, |f| {
        (f.ends_with(".tpl") || f.ends_with(".tpl.lz")) && !f.starts_with("test/preview/")
    });
    let mut rng = Rng(0x5eed_0021);
    let (mut ok, mut err, mut files) = (0, 0, 0);
    for p in paths.iter().step_by(5) {
        let src = fetch(&disc, p);
        if src.len() > 200_000 || src.len() < 20 {
            continue;
        }
        files += 1;
        let full = checked(p, &src).unwrap_or_else(|e| panic!("{p}: {e}"));
        let header =
            (4 + u32::from_be_bytes(src[..4].try_into().unwrap()) as usize * 0x10).min(src.len());
        for i in 0..12 {
            let mut b = src.clone();
            let h = mutate(&src[..header], &mut rng);
            b[..header].copy_from_slice(&h);
            match checked(&format!("{p} mutation {i}"), &b) {
                Ok(_) => ok += 1,
                Err(_) => err += 1,
            }
        }
        for cut in [0, 3, 4, header - 1, header, src.len() / 2, src.len() - 1] {
            if let Ok(levels) = checked(&format!("{p} cut at {cut}"), &src[..cut]) {
                assert!(levels <= full);
            }
        }
    }
    eprintln!("{files} real files: {ok} mutations parsed, {err} rejected");
    assert!(files > 3 && err > 0);
}
