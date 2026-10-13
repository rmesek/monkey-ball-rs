//! Real-disc tests. They run only when a Super Monkey Ball 1 image is available and skip otherwise.
//! Point `MB_DISC` at an image, or put a `Super Monkey Ball*.rvz|iso` next to this repository.
//! The Deno parity test additionally needs `deno` on PATH and the read-only `../WebMonkeyBall` checkout
//! (override with `MB_REF`); dumps go to `MB_SCRATCH` (default: the OS temp dir), never into the repo.

use mb_assets::disc::DiscPack;
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use mb_gma::{parse_gma, parse_naomi_obj};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Triangle count and position checksum (same formula and order as `tests/ref/gma_ref.ts`).
fn checksum(model: &mb_gma::Model) -> (usize, f64) {
    let (mut tris, mut sum) = (0usize, 0.0f64);
    for mesh in model.meshes().unwrap() {
        tris += mesh.triangle_count();
        for &i in &mesh.indices {
            let p = mesh.vertices.positions[i as usize];
            sum += p[0] as f64 + 2.0 * p[1] as f64 + 3.0 * p[2] as f64;
        }
    }
    (tris, sum)
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

fn scratch_dir() -> PathBuf {
    let d = std::env::var_os("MB_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("mb-gma-test"));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn matches_reference_triangle_counts_and_checksums() {
    let disc = disc_or_skip!();
    let ref_dir = std::env::var_os("MB_REF")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../WebMonkeyBall")
        });
    if !Path::new(&ref_dir).join("src/gma.ts").exists() {
        eprintln!(
            "skipped: reference checkout not found at {}",
            ref_dir.display()
        );
        return;
    }
    if Command::new("deno").arg("--version").output().is_err() {
        eprintln!("skipped: deno not available");
        return;
    }
    let scratch = scratch_dir();
    let wanted = [
        "test/st001/st001.gma",
        "test/st002/st002.gma",
        "test/st040/st040.gma",
        "test/init/common.gma",
        "test/bg/bg_jun.gma",
        "test/bg/bg_wat.gma",
        "test/init/commend.gma",
        "test/init/m_bowl.gma",
        "test/init/common.gma.lz",
    ];
    let mut ours = BTreeMap::new();
    let mut args = vec![];
    for (path, bytes) in all_gmas(&disc) {
        if !wanted.contains(&path.as_str()) {
            continue;
        }
        let local = scratch.join(path.replace('/', "_"));
        std::fs::write(&local, &bytes).unwrap();
        let gma = parse_gma(&bytes).unwrap();
        let file = local.file_name().unwrap().to_string_lossy().into_owned();
        for m in gma.iter() {
            ours.insert((file.clone(), m.name.clone()), checksum(m));
        }
        args.push(local);
    }
    assert!(args.len() >= 3, "expected at least 3 wanted gma files");
    let out = Command::new("deno")
        .arg("run")
        .arg("-A")
        .arg("--no-check")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ref/gma_ref.ts"))
        .arg(&ref_dir)
        .arg(&scratch)
        .args(&args)
        .output()
        .expect("run deno");
    assert!(
        out.status.success(),
        "deno failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut compared, mut tri_total) = (0, 0usize);
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f.len(), 6, "bad line {line}");
        let key = (f[0].to_string(), f[1].to_string());
        let (tris, sum) = *ours
            .get(&key)
            .unwrap_or_else(|| panic!("reference model {key:?} missing from ours"));
        let ref_tris = f[4].parse::<f64>().unwrap() as usize;
        let ref_sum: f64 = f[5].parse().unwrap();
        assert_eq!(tris, ref_tris, "triangle count {key:?}");
        assert!(
            (sum - ref_sum).abs() <= 1e-6 * ref_sum.abs().max(1.0),
            "checksum {key:?}: {sum} vs {ref_sum}"
        );
        compared += 1;
        tri_total += tris;
    }
    assert_eq!(compared, ours.len(), "model sets differ");
    eprintln!("parity: {compared} models, {tri_total} triangles identical to reference");
    assert!(compared >= 3);
}
