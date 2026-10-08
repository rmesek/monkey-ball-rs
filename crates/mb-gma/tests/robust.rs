//! The GMA, NaomiLib and display-list parsers on hostile input: an error or a model, never a panic, and never
//! more heap than a small multiple of the input. Inputs are built in code; the last tests repeat the campaign
//! on archives read from the user's disc (in memory only) and skip without one.

mod common;
use common::*;
use mb_gma::{attr, decode_display_list, parse_gma, parse_naomi_obj, Error};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// Parsing is capped at 16 decoded bytes per input byte plus 64 KiB; a decoded display list is at most 64
/// bytes of vertices and indices per 6 bytes of input. `Vec` growth can double either.
fn heap_limit(len: usize) -> usize {
    len * 48 + (1 << 18)
}

/// Parse and decode every mesh, as the renderer does.
fn gma_all(bytes: &[u8]) -> Result<usize, Error> {
    let gma = parse_gma(bytes)?;
    let mut tris = 0;
    for m in gma.iter() {
        for mesh in m.meshes()? {
            assert!(mesh
                .indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len()));
            tris += mesh.triangle_count();
        }
        let _ = m.shapes.iter().map(|s| m.shape_tev_layers(s).count());
    }
    Ok(tris)
}

fn nl_all(bytes: &[u8]) -> Result<usize, Error> {
    let obj = parse_naomi_obj(bytes)?;
    let mut tris = 0;
    for m in &obj.models {
        for mesh in &m.meshes {
            assert!(mesh
                .indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len()));
            tris += mesh.triangle_count();
        }
    }
    Ok(tris)
}

fn checked<T>(what: &str, bytes: &[u8], f: impl Fn(&[u8]) -> T) -> T {
    let (r, heap) = run_guarded(what, bytes, f);
    assert!(
        heap <= heap_limit(bytes.len()),
        "{what}: {heap} bytes of heap for {} bytes of input",
        bytes.len()
    );
    r
}

fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn tri_dlist(cmd: u8, verts: u16, attrs_bytes: usize) -> Vec<u8> {
    let mut dl = vec![0u8, cmd];
    dl.extend_from_slice(&verts.to_be_bytes());
    for i in 0..verts as usize * attrs_bytes {
        dl.push((i * 7) as u8);
    }
    while dl.len() % 0x20 != 0 {
        dl.push(0);
    }
    dl
}

/// One shape: 0x60 header, then the display lists selected by `dlist_flags`.
fn shape(dlist_flags: u8, vtx_attrs: u32, vertex_bytes: usize) -> Vec<u8> {
    let front = tri_dlist(0x98, 4, vertex_bytes);
    let back = tri_dlist(0x90, 3, vertex_bytes);
    let mut s = vec![0u8; 0x60];
    s[0x11] = 255;
    s[0x12] = 1;
    s[0x13] = dlist_flags;
    s[0x18..0x1a].copy_from_slice(&(-1i16).to_be_bytes());
    s[0x1c..0x20].copy_from_slice(&vtx_attrs.to_be_bytes());
    s[0x28..0x2c].copy_from_slice(&(front.len() as u32).to_be_bytes());
    s[0x2c..0x30].copy_from_slice(&(back.len() as u32).to_be_bytes());
    if dlist_flags & 1 != 0 {
        s.extend_from_slice(&front);
    }
    if dlist_flags & 2 != 0 {
        s.extend_from_slice(&back);
    }
    if dlist_flags & 0xc != 0 {
        let mut extra = vec![0u8; 0x20];
        extra[0x08..0x0c].copy_from_slice(&(front.len() as u32).to_be_bytes());
        extra[0x0c..0x10].copy_from_slice(&(back.len() as u32).to_be_bytes());
        s.extend_from_slice(&extra);
        s.extend_from_slice(&front);
        s.extend_from_slice(&back);
    }
    s
}

/// Three table entries (one empty), two models: tev layers, a matrix, opaque and translucent shapes, every
/// display-list slot, float and attribute-rich vertices.
fn synthetic_gma() -> Vec<u8> {
    let pos = 1u32 << attr::POS;
    let rich = pos | (1 << attr::NRM) | (1 << attr::CLR0) | (1 << attr::TEX0) | (1 << attr::TEX1);
    let mut models = Vec::new();
    for (shapes, translucent) in [
        (vec![shape(3, pos, 12), shape(0xf, rich, 44)], 1i16),
        (vec![shape(1, pos, 12)], 0),
    ] {
        let mut m = vec![0u8; 0x40];
        m[..4].copy_from_slice(b"GCMF");
        m[0x14..0x18].copy_from_slice(&2.0f32.to_be_bytes());
        m[0x18..0x1a].copy_from_slice(&2i16.to_be_bytes());
        m[0x1a..0x1c].copy_from_slice(&(shapes.len() as i16 - translucent).to_be_bytes());
        m[0x1c..0x1e].copy_from_slice(&translucent.to_be_bytes());
        m[0x1e] = 1;
        m[0x20..0x24].copy_from_slice(&(0x40u32 + 2 * 0x20 + 0x30).to_be_bytes());
        m.extend_from_slice(&[0u8; 2 * 0x20 + 0x30]);
        for s in shapes {
            m.extend_from_slice(&s);
        }
        models.push(m);
    }
    let names = b"MODEL_A\0MODEL_B\0";
    let mut v = Vec::new();
    put32(&mut v, 3);
    put32(&mut v, 8 + 24 + names.len() as u32);
    put32(&mut v, 0);
    put32(&mut v, 0);
    put32(&mut v, u32::MAX);
    put32(&mut v, 0);
    put32(&mut v, models[0].len() as u32);
    put32(&mut v, 8);
    v.extend_from_slice(names);
    v.extend_from_slice(&models[0]);
    v.extend_from_slice(&models[1]);
    v
}

fn nl_vertex(v: &mut Vec<u8>, x: f32) {
    put32(v, x.to_bits() | 1);
    for f in [1.0f32, 2.0, 0.0, 1.0, 0.0, 0.5, 0.75] {
        put32(v, f.to_bits());
    }
}

/// Two models (the second marked invalid), two meshes, strip / triangle / quad lists, a back reference.
fn synthetic_nl() -> Vec<u8> {
    let mut b = vec![0u8; 4];
    put32(&mut b, 0x30);
    put32(&mut b, 0x20);
    put32(&mut b, 0);
    b.resize(0x20, 0);
    put32(&mut b, u32::MAX); // invalid model at 0x20
    b.resize(0x28, 0);
    put32(&mut b, 0x400); // name pointer of the model at 0x30
    put32(&mut b, 0);
    put32(&mut b, 0);
    put32(&mut b, 0);
    for _ in 0..4 {
        put32(&mut b, 1.0f32.to_bits());
    }
    for lists in [vec![(1u32 << 4, 4u32)], vec![(1 << 3, 1), (1 << 2, 1)]] {
        let mut dl = Vec::new();
        for (flags, n) in lists {
            put32(&mut dl, flags | 2);
            put32(&mut dl, n);
            let verts = match flags {
                0x10 => n,
                0x08 => n * 3,
                _ => n * 4,
            };
            let first = dl.len();
            for i in 0..verts - 1 {
                nl_vertex(&mut dl, i as f32);
            }
            // The last vertex of each list refers back to its first.
            let at = dl.len();
            put32(&mut dl, 0);
            dl.extend_from_slice(&(first as i32 - at as i32 - 8).to_be_bytes());
        }
        let mut mesh = vec![0u8; 0x50];
        mesh[..4].copy_from_slice(&1u32.to_be_bytes());
        mesh[0x20..0x24].copy_from_slice(&3i32.to_be_bytes());
        mesh[0x4c..0x50].copy_from_slice(&(dl.len() as u32).to_be_bytes());
        b.extend_from_slice(&mesh);
        b.extend_from_slice(&dl);
    }
    put32(&mut b, 0);
    assert!(b.len() < 0x400);
    b.resize(0x404, 0);
    b.extend_from_slice(b"obj_TEST\0");
    b
}

#[test]
fn synthetic_inputs_are_valid() {
    let _g = serial();
    let g = parse_gma(&synthetic_gma()).unwrap();
    assert_eq!(g.models.len(), 2);
    let a = g.model("MODEL_A").unwrap();
    assert_eq!((a.id, a.shapes.len(), a.tev_layers.len()), (0, 2, 2));
    assert_eq!(a.shapes[1].dlists.len(), 4);
    assert!(a.shapes[1].translucent && !a.shapes[0].translucent);
    assert_eq!(g.model("MODEL_B").unwrap().id, 2);
    // strip of 4 = 2 triangles, list of 3 = 1; shape 0 has both, shape 1 has both twice, model B one strip.
    assert_eq!(gma_all(&synthetic_gma()).unwrap(), 3 + 6 + 2);

    let o = parse_naomi_obj(&synthetic_nl()).unwrap();
    assert_eq!(o.models.len(), 1);
    assert_eq!(o.models[0].meshes.len(), 2);
    assert_eq!(o.model_by_name("TEST").unwrap().id, 0);
    assert_eq!(nl_all(&synthetic_nl()).unwrap(), 2 + 1 + 2);
    let m = &o.models[0].meshes[0];
    assert_eq!(m.vertices.positions[3], m.vertices.positions[0]);
}

fn campaign<T>(name: &str, src: &[u8], f: impl Fn(&[u8]) -> Result<T, Error> + Copy, seed: u64) {
    for len in 0..src.len() {
        let _ = checked(&format!("{name}: truncation to {len}"), &src[..len], f);
    }
    let mut b = src.to_vec();
    for i in 0..src.len() {
        for v in [0x00, 0x01, 0x7f, 0x80, 0xff, src[i] ^ 0x55] {
            b[i] = v;
            let _ = checked(&format!("{name}: byte {i:#x} = {v:#x}"), &b, f);
        }
        b[i] = src[i];
    }
    for o in (0..src.len().saturating_sub(3)).step_by(2) {
        let mut values = hostile_u32(src.len());
        values.extend([o as u32, (o as u32).wrapping_sub(8)]);
        for v in values {
            b[o..o + 4].copy_from_slice(&v.to_be_bytes());
            let _ = checked(&format!("{name}: word {o:#x} = {v:#x}"), &b, f);
        }
        b[o..o + 4].copy_from_slice(&src[o..o + 4]);
    }
    let mut rng = Rng(seed);
    let (mut ok, mut err) = (0, 0);
    for i in 0..20_000 {
        let m = mutate(src, &mut rng);
        match checked(&format!("{name}: random mutation {i}"), &m, f) {
            Ok(_) => ok += 1,
            Err(_) => err += 1,
        }
    }
    assert!(
        ok > 500 && err > 500,
        "{name}: ok {ok}, err {err}: the campaign is one-sided"
    );
}

#[test]
fn gma_mutation_campaign() {
    let _g = serial();
    campaign("gma", &synthetic_gma(), gma_all, 0x5eed_0010);
}

#[test]
fn naomi_mutation_campaign() {
    let _g = serial();
    campaign("naomi", &synthetic_nl(), nl_all, 0x5eed_0011);
}

#[test]
fn display_list_mutation_campaign() {
    let _g = serial();
    let rich = (1 << attr::PNMTXIDX)
        | (1 << attr::POS)
        | (1 << attr::NRM)
        | (1 << attr::CLR0)
        | (1 << attr::TEX0);
    let mut src = tri_dlist(0x98, 6, 37);
    src.extend_from_slice(&tri_dlist(0x81, 8, 21));
    src.extend_from_slice(&tri_dlist(0xa0, 5, 37));
    assert_eq!(decode_display_list(&src, rich).unwrap().vertices.len(), 19);
    let mut rng = Rng(0x5eed_0012);
    for i in 0..30_000 {
        let b = mutate(&src, &mut rng);
        // Any attribute mask, including NBT and none at all.
        let attrs = match i % 4 {
            0 => rich,
            1 => rng.next() as u32,
            2 => rng.next() as u32 & 0x03ff_ffff | (1 << attr::POS),
            _ => 1 << rng.below(32),
        };
        let r = checked(&format!("display list mutation {i}"), &b, |d| {
            decode_display_list(d, attrs)
        });
        if let Ok(l) = r {
            assert!(l.indices.iter().all(|&x| (x as usize) < l.vertices.len()));
            assert_eq!(l.vertices.normals.len(), l.vertices.len());
            assert_eq!(l.vertices.uv2.len(), l.vertices.len());
        }
    }
    for len in 0..src.len() {
        let _ = checked("display list truncation", &src[..len], |d| {
            decode_display_list(d, rich)
        });
    }
}

/// A vertex with no attributes is zero bytes long: before the fix a 3-byte command decoded to 65535 vertices
/// (3.4 MB), and a display list of such commands to a million times its size.
#[test]
fn zero_size_vertices_are_rejected() {
    let _g = serial();
    let mut dl = Vec::new();
    for _ in 0..1000 {
        dl.extend_from_slice(&[0x98, 0xff, 0xff]);
    }
    for attrs in [0, 1 << attr::NBT, 1 << 30] {
        let r = checked("zero-size vertices", &dl, |d| decode_display_list(d, attrs));
        assert_eq!(
            r,
            Err(Error::Invalid("display list vertices have no position"))
        );
    }
    // A vertex count the buffer cannot hold is refused before any vertex is decoded.
    let r = checked("oversized count", &[0x90, 0xff, 0xff, 0, 0, 0, 0], |d| {
        decode_display_list(d, 1 << attr::POS)
    });
    assert!(matches!(r, Err(Error::OutOfBounds { .. })));
}

/// Thousands of table entries that all point at the same large model.
#[test]
fn shared_models_are_rejected_before_allocating() {
    let _g = serial();
    let entries = 8192u32;
    let base = 8 + entries * 8;
    let mut v = Vec::new();
    put32(&mut v, entries);
    put32(&mut v, base);
    for _ in 0..entries {
        put32(&mut v, 0);
        put32(&mut v, u32::MAX);
    }
    let dl_len = 0x40000usize;
    let mut m = vec![0u8; 0x40];
    m[..4].copy_from_slice(b"GCMF");
    m[0x1a..0x1c].copy_from_slice(&1i16.to_be_bytes());
    m[0x20..0x24].copy_from_slice(&0x40u32.to_be_bytes());
    let mut s = vec![0u8; 0x60];
    s[0x13] = 1;
    s[0x1c..0x20].copy_from_slice(&(1u32 << attr::POS).to_be_bytes());
    s[0x28..0x2c].copy_from_slice(&(dl_len as u32).to_be_bytes());
    v.extend_from_slice(&m);
    v.extend_from_slice(&s);
    v.resize(v.len() + dl_len, 0);
    let r = checked("shared models", &v, parse_gma);
    assert!(matches!(r, Err(Error::Invalid(_))), "{:?}", r.err());
}

/// Thousands of NaomiLib table entries sharing one model, and separately one very long shared name.
#[test]
fn shared_naomi_models_and_names_are_rejected_before_allocating() {
    let _g = serial();
    let entries = 16384usize;
    let model_at = 4 + entries * 4 + 4 + 8;
    let mut b = vec![0u8; 4];
    for _ in 0..entries {
        put32(&mut b, model_at as u32);
    }
    put32(&mut b, 0);
    put32(&mut b, 0); // name pointer (none)
    put32(&mut b, 0);
    assert_eq!(b.len(), model_at);
    b.extend_from_slice(&[0u8; 0x18]);
    let quads = 2048u32;
    let mut mesh = vec![0u8; 0x50];
    mesh[..4].copy_from_slice(&1u32.to_be_bytes());
    mesh[0x4c..0x50].copy_from_slice(&(8 + quads * 4 * 0x20).to_be_bytes());
    b.extend_from_slice(&mesh);
    put32(&mut b, 1 << 2);
    put32(&mut b, quads);
    for i in 0..quads * 4 {
        nl_vertex(&mut b, i as f32);
    }
    put32(&mut b, 0);
    let r = checked("shared naomi models", &b, parse_naomi_obj);
    assert!(matches!(r, Err(Error::Invalid(_))), "{:?}", r.err());

    let name_at = b.len();
    let header = model_at - 8;
    b[header..header + 4].copy_from_slice(&(name_at as u32).to_be_bytes());
    b.extend(std::iter::repeat_n(b'x', 0x20000));
    let r = checked("shared naomi names", &b, parse_naomi_obj);
    assert!(matches!(r, Err(Error::Invalid(_))), "{:?}", r.err());
    let (map, heap) = peak_heap_growth(|| mb_gma::build_obj_name_map(&b));
    assert!(map.len() <= 2 && heap <= heap_limit(b.len()));
}

fn real_campaign(
    keep: impl Fn(&str) -> bool,
    step: usize,
    f: impl Fn(&[u8]) -> Result<usize, Error> + Copy,
    seed: u64,
) {
    let _g = serial();
    let Some(disc) = open_disc() else { return };
    let paths = disc_paths(&disc, keep);
    let mut rng = Rng(seed);
    let (mut ok, mut err, mut files) = (0, 0, 0);
    for p in paths.iter().step_by(step) {
        let src = fetch(&disc, p);
        // Keep the run short: the largest archives are tens of megabytes.
        if src.len() > 400_000 {
            continue;
        }
        files += 1;
        assert!(checked(p, &src, f).is_ok(), "{p}");
        for i in 0..60 {
            let b = mutate(&src, &mut rng);
            match checked(&format!("{p} mutation {i}"), &b, f) {
                Ok(_) => ok += 1,
                Err(_) => err += 1,
            }
        }
        for cut in [0, 7, 8, src.len() / 3, src.len() / 2, src.len() - 1] {
            let _ = checked(&format!("{p} cut at {cut}"), &src[..cut], f);
        }
    }
    eprintln!("{files} real files: {ok} mutations parsed, {err} rejected");
    assert!(files > 3 && err > 0);
}

#[test]
fn real_gmas_survive_the_same_campaign() {
    real_campaign(
        |f| f.ends_with(".gma") || f.ends_with(".gma.lz"),
        4,
        gma_all,
        0x5eed_0013,
    );
}

#[test]
fn real_naomi_archives_survive_the_same_campaign() {
    real_campaign(|f| f.ends_with("_p.lz"), 2, nl_all, 0x5eed_0014);
}
