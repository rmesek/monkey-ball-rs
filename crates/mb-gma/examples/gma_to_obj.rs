//! Export a GMA (or NaomiLib `_p.lz`) file to Wavefront OBJ for eyeballing in any viewer.
//!
//! usage: cargo run --release -p mb-gma --example gma_to_obj -- <input .gma|.gma.lz|_p.lz> <out.obj> [model name]
//!
//! Inputs ending in `.lz` are LZSS-decompressed first. Write output outside the repository
//! (game data must never be committed). Each shape/display list becomes an OBJ group; mirrored `Back`
//! cull duplicates are exported too, so open the result with back-face culling off.

use mb_gma::{parse_gma, parse_naomi_obj, Vertices};
use std::fmt::Write as _;

fn emit(out: &mut String, base: &mut usize, group: &str, v: &Vertices, indices: &[u32]) {
    writeln!(out, "g {group}").unwrap();
    for p in &v.positions {
        writeln!(out, "v {} {} {}", p[0], p[1], p[2]).unwrap();
    }
    for t in &v.uv0 {
        writeln!(out, "vt {} {}", t[0], 1.0 - t[1]).unwrap();
    }
    for n in &v.normals {
        writeln!(out, "vn {} {} {}", n[0], n[1], n[2]).unwrap();
    }
    for t in indices.chunks(3) {
        let f = |i: u32| {
            let i = *base + i as usize + 1;
            format!("{i}/{i}/{i}")
        };
        writeln!(out, "f {} {} {}", f(t[0]), f(t[1]), f(t[2])).unwrap();
    }
    *base += v.len();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: gma_to_obj <input> <out.obj> [model name]");
        std::process::exit(2);
    }
    let mut bytes = std::fs::read(&args[1]).expect("read input");
    if args[1].ends_with(".lz") {
        bytes = mb_assets::lzss::decompress(&bytes).expect("lzss");
    }
    let filter = args.get(3);
    let mut out = String::new();
    let mut base = 0usize;
    if args[1].ends_with("_p.lz") {
        let obj = parse_naomi_obj(&bytes).expect("parse NaomiLib");
        for m in &obj.models {
            for (i, mesh) in m.meshes.iter().enumerate() {
                emit(
                    &mut out,
                    &mut base,
                    &format!("nl{}_{i}", m.id),
                    &mesh.vertices,
                    &mesh.indices,
                );
            }
        }
    } else {
        let gma = parse_gma(&bytes).expect("parse GMA");
        for m in gma.iter().filter(|m| filter.is_none_or(|f| *f == m.name)) {
            for mesh in m.meshes().expect("decode") {
                let group = format!(
                    "{}_s{}_d{}_{:?}",
                    m.name, mesh.shape_index, mesh.dlist_index, mesh.cull
                );
                emit(&mut out, &mut base, &group, &mesh.vertices, &mesh.indices);
            }
        }
    }
    std::fs::write(&args[2], out).expect("write obj");
    eprintln!("wrote {} ({} vertices)", args[2], base);
}
