//! Debug helper: list disc files or the models of a GMA.
//!
//! `cargo run --release -p mb-app --example inspect -- <disc> ls [prefix]`
//! `cargo run --release -p mb-app --example inspect -- <disc> gma <path in content root, e.g. smb1_content/test/st001/st001.gma>`
//! `cargo run --release -p mb-app --example inspect -- <disc> tpl <path> <out.png> [index]`: contact sheet of a
//! TPL (or one texture at full size) composited over grey, for working out HUD sprites. Write it outside the repo.

use mb_app::content::open_disc_image;

/// All textures in a grid (136 px cells, shrunk to fit), or texture `only` at its native size.
fn tpl_sheet(tpl: &mb_tpl::Tpl, only: Option<usize>) -> (usize, usize, Vec<u8>) {
    const BG: [u8; 4] = [90, 90, 140, 255];
    let blit = |img: &mut [u8], iw: usize, t: &mb_tpl::Texture, ox: usize, oy: usize, max: f32| {
        let Ok(rgba) = t.decode_rgba(0) else { return };
        let (tw, th) = t.level_size(0);
        let s = (max / tw.max(th) as f32).min(1.0);
        let (dw, dh) = ((tw as f32 * s) as usize, (th as f32 * s) as usize);
        for y in 0..dh {
            for x in 0..dw {
                let (sx, sy) = ((x as f32 / s) as usize, (y as f32 / s) as usize);
                let p = &rgba[(sy.min(th - 1) * tw + sx.min(tw - 1)) * 4..][..4];
                let o = ((oy + y) * iw + ox + x) * 4;
                let al = p[3] as u32;
                for c in 0..3 {
                    img[o + c] = ((p[c] as u32 * al + img[o + c] as u32 * (255 - al)) / 255) as u8;
                }
            }
        }
    };
    if let Some(t) = only.and_then(|i| tpl.get(i)) {
        let (w, h) = t.level_size(0);
        let mut img = BG.repeat(w * h);
        blit(&mut img, w, t, 0, 0, f32::MAX);
        return (w, h, img);
    }
    let (cols, cell) = (8usize, 136usize);
    let n = tpl
        .textures()
        .iter()
        .map(|t| t.index + 1)
        .max()
        .unwrap_or(1);
    let (w, h) = (cols * cell, n.div_ceil(cols) * cell);
    let mut img = BG.repeat(w * h);
    for t in tpl.textures() {
        let (ox, oy) = ((t.index % cols) * cell + 4, (t.index / cols) * cell + 4);
        blit(&mut img, w, t, ox, oy, (cell - 8) as f32);
    }
    (w, h, img)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: inspect <disc> ls [prefix] | gma <path> | tpl <path> <out.png> [index]");
        return;
    }
    if args[1] == "ls" {
        let disc = mb_assets::disc::DiscPack::open(&args[0]).expect("open disc");
        let prefix = args.get(2).map(String::as_str).unwrap_or("");
        for f in disc.list().into_iter().filter(|f| f.starts_with(prefix)) {
            println!("{f}");
        }
    } else if args[1] == "tpl" {
        let content = open_disc_image(&args[0]).expect("open");
        let tpl = mb_tpl::parse(&content.read_maybe_lz(&args[2]).expect("read tpl")).expect("tpl");
        let only: Option<usize> = args.get(4).and_then(|s| s.parse().ok());
        let (w, h, img) = tpl_sheet(&tpl, only);
        std::fs::write(
            &args[3],
            mb_app::image::encode_png(w as u32, h as u32, &img),
        )
        .expect("write png");
        for t in tpl.textures() {
            println!("{:3} {:?} {}x{}", t.index, t.format, t.width, t.height);
        }
    } else if args[1] == "stage" {
        let content = open_disc_image(&args[0]).expect("open");
        let id: u32 = args[2].parse().expect("stage id");
        let a = mb_app::stage_scene::StageAssets::load(&content, id, None).expect("load stage");
        println!(
            "bg {} start {:?}",
            a.bg.file_name,
            a.stagedef.start_positions.first()
        );
        for (i, o) in a.stagedef.bg_objects.iter().enumerate() {
            println!(
                "bg[{i}] {:<24} flags {:#010x} pos {:?} scale {:?} transl {} anim {}",
                o.name,
                o.flags,
                o.pos,
                o.scale,
                o.translucency,
                o.anim.is_some()
            );
        }
        for (i, o) in a.stagedef.fg_objects.iter().enumerate() {
            println!("fg[{i}] {:<24} flags {:#010x}", o.name, o.flags);
        }
        for (i, g) in a.stagedef.anim_groups.iter().enumerate() {
            println!(
                "group {i}: models {:?}",
                g.models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()
            );
        }
    } else if args[1] == "model" {
        let content = open_disc_image(&args[0]).expect("open");
        let bytes = content.read_maybe_lz(&args[2]).expect("read gma");
        let gma = mb_gma::parse_gma(&bytes).expect("parse gma");
        let m = gma.model(&args[3]).expect("model");
        println!(
            "{} bound {:?} r {}",
            m.name, m.bound_sphere_center, m.bound_sphere_radius
        );
        for (i, s) in m.shapes.iter().enumerate() {
            println!(
                "shape {i} translucent {} origin {:?} dlists {}",
                s.translucent,
                s.origin,
                s.dlists.len()
            );
            println!("  {:?}", s.material);
            for l in m.shape_tev_layers(s) {
                println!("  layer {:?} wrap {:?}/{:?}", l, l.wrap_s(), l.wrap_t());
            }
            if let Ok(meshes) = m.meshes() {
                for me in meshes.iter().filter(|x| x.shape_index == i) {
                    let v = &me.vertices;
                    println!(
                        "  mesh cull {:?} verts {} idx {} colors {:?} uv0 {:?} pos {:?}",
                        me.cull,
                        v.len(),
                        me.indices.len(),
                        &v.colors[..v.len().min(4)],
                        &v.uv0[..v.len().min(3)],
                        &v.positions[..v.len().min(3)]
                    );
                }
            }
        }
    } else if args[1] == "nl" {
        let content = open_disc_image(&args[0]).expect("open");
        let bytes = content.read_lz(&args[2]).expect("read nl");
        let obj = mb_gma::parse_naomi_obj(&bytes).expect("parse nl");
        let mut names: Vec<_> = obj.names.iter().collect();
        names.sort_by_key(|(_, v)| **v);
        for m in &obj.models {
            let name = names
                .iter()
                .find(|(_, v)| **v == m.id)
                .map(|(k, _)| k.as_str())
                .unwrap_or("?");
            println!(
                "model {} {} kind {:?} flags {:#x} meshes {}",
                m.id,
                name,
                m.kind,
                m.flags,
                m.meshes.len()
            );
            for me in &m.meshes {
                println!(
                    "  mesh type {} amb {} color {:?} texflags {:#x} tex {:?} cull {:?} z {} dw {} tris {} n0 {:?} c0 {:?} uv0 {:?}",
                    me.mesh_type, me.ambient_color_scale, me.material_color, me.tex_flags, me.tex_idx, me.cull,
                    me.z_compare, me.depth_write, me.triangle_count(),
                    me.vertices.normals.first(), me.vertices.colors.first(), me.vertices.uv0.first()
                );
            }
        }
    } else if args[1] == "gma" {
        let content = open_disc_image(&args[0]).expect("open");
        let bytes = content.read_maybe_lz(&args[2]).expect("read gma");
        let gma = mb_gma::parse_gma(&bytes).expect("parse gma");
        for m in gma.iter() {
            let tris: usize = m
                .meshes()
                .map(|v| v.iter().map(|x| x.triangle_count()).sum())
                .unwrap_or(0);
            println!(
                "{:4} {:<24} shapes {:3} (opaque {:3}) layers {:2} tris {:6} flags {:#x}",
                m.id,
                m.name,
                m.shapes.len(),
                m.opaque_shape_count,
                m.tev_layers.len(),
                tris,
                m.flags
            );
        }
        for s in &gma.skipped {
            println!("skipped {} {} flags {:#x}", s.id, s.name, s.flags);
        }
    }
}
