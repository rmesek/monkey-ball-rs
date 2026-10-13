//! Real-disc tests. They run only when a Super Monkey Ball 1 image is available and skip otherwise.
//! Point `MB_DISC` at an image, or put a `Super Monkey Ball*.rvz|iso` next to this repository.

use mb_assets::disc::DiscPack;
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use mb_gma::{parse_gma, parse_naomi_obj};
use std::path::PathBuf;

fn find_disc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MB_DISC") {
        return Some(PathBuf::from(p));
    }
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for entry in std::fs::read_dir(parent).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("super monkey ball")
            && (name.ends_with(".rvz") || name.ends_with(".iso") || name.ends_with(".gcm"))
        {
            let disc = DiscPack::open(entry.path()).ok()?;
            if disc.smb1_region().is_some() {
                return Some(entry.path());
            }
        }
    }
    None
}

macro_rules! disc_or_skip {
    () => {
        match find_disc() {
            Some(p) => DiscPack::open(p).expect("open disc"),
            None => {
                eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
                return;
            }
        }
    };
}

/// (disc path, decompressed bytes) of every GMA on the disc, including `.gma.lz`.
fn all_gmas(disc: &DiscPack) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for f in disc.list() {
        let lz = f.ends_with(".gma.lz");
        if !(f.ends_with(".gma") || lz) {
            continue;
        }
        let raw = disc.fetch(f).unwrap();
        let bytes = if lz {
            lzss::decompress(&raw).unwrap_or_else(|e| panic!("{f}: {e}"))
        } else {
            raw
        };
        out.push((f.to_string(), bytes));
    }
    out
}

#[test]
fn every_gma_parses_with_sane_geometry() {
    let disc = disc_or_skip!();
    let gmas = all_gmas(&disc);
    assert!(gmas.len() > 100, "found only {} gma files", gmas.len());
    let (mut models, mut tris) = (0usize, 0usize);
    let (mut nan_normals, mut total_vertices) = (0usize, 0usize);
    for (path, bytes) in &gmas {
        let gma = parse_gma(bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        for model in gma.iter() {
            models += 1;
            assert!(
                model.bound_sphere_radius.is_finite(),
                "{path}/{}",
                model.name
            );
            for m in &model.matrices {
                assert!(m.iter().all(|v| v.is_finite()), "{path}/{}", model.name);
            }
            for shape in &model.shapes {
                assert!(shape.origin.iter().all(|v| v.is_finite()));
                assert!((0.0..=1.0).contains(&shape.material.alpha));
                for &idx in shape
                    .material
                    .tev_layer_idxs
                    .iter()
                    .take(shape.material.tev_layer_count as usize)
                {
                    assert!(
                        idx < 0 || (idx as usize) < model.tev_layers.len(),
                        "{path}/{}",
                        model.name
                    );
                }
            }
            for mesh in model
                .meshes()
                .unwrap_or_else(|e| panic!("{path}/{}: {e}", model.name))
            {
                let v = &mesh.vertices;
                let n = v.len();
                assert!(
                    [
                        v.normals.len(),
                        v.colors.len(),
                        v.uv0.len(),
                        v.uv1.len(),
                        v.uv2.len()
                    ]
                    .iter()
                    .all(|&l| l == n)
                );
                assert_eq!(mesh.indices.len() % 3, 0);
                assert!(
                    mesh.indices.iter().all(|&i| (i as usize) < n),
                    "{path}/{}",
                    model.name
                );
                for p in &v.positions {
                    assert!(
                        p.iter().all(|c| c.is_finite() && c.abs() < 1.0e7),
                        "{path}/{}: {p:?}",
                        model.name
                    );
                }
                nan_normals += v
                    .normals
                    .iter()
                    .filter(|n| !n.iter().all(|c| c.is_finite()))
                    .count();
                total_vertices += n;
                assert!(v.uv0.iter().flatten().all(|c| c.is_finite()));
                tris += mesh.triangle_count();
            }
        }
    }
    eprintln!(
        "{} gma files, {models} models, {tris} triangles, {nan_normals}/{total_vertices} NaN normals",
        gmas.len()
    );
    // The original data contains a few NaN normals (degenerate faces); positions must always be finite.
    assert!(nan_normals * 100 < total_vertices);
    assert!(models > 500 && tris > 100_000);
}

#[test]
fn stage_one_model_lookup() {
    let disc = disc_or_skip!();
    let bytes = disc.fetch("test/st001/st001.gma").unwrap();
    let gma = parse_gma(&bytes).unwrap();
    let m = gma.model("STG001").expect("STG001");
    assert!(!m.shapes.is_empty());
    assert!(gma.names().any(|n| n == "STG001_TREE00_NOCOLI"));
}

#[test]
fn every_naomilib_archive_parses() {
    let disc = disc_or_skip!();
    let (mut files, mut models, mut tris) = (0, 0, 0);
    for f in disc.list() {
        if !f.ends_with("_p.lz") {
            continue;
        }
        let raw = disc.fetch(f).unwrap();
        let Ok(bytes) = lzss::decompress(&raw) else {
            continue;
        };
        let obj = parse_naomi_obj(&bytes).unwrap_or_else(|e| panic!("{f}: {e}"));
        files += 1;
        for m in &obj.models {
            models += 1;
            assert!(m.bound_sphere_radius.is_finite());
            for mesh in &m.meshes {
                let n = mesh.vertices.len();
                assert!(mesh.indices.iter().all(|&i| (i as usize) < n), "{f}");
                assert!(
                    mesh.vertices
                        .positions
                        .iter()
                        .flatten()
                        .all(|c| c.is_finite()),
                    "{f}"
                );
                assert!(mesh.material_color.iter().all(|c| c.is_finite()));
                tris += mesh.triangle_count();
            }
        }
    }
    eprintln!("{files} NaomiLib archives, {models} models, {tris} triangles");
    assert!(files > 5 && models > 10);
}
