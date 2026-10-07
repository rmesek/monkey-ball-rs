//! Debug helper: list disc files or the models of a GMA.
//!
//! `cargo run --release -p mb-app --example inspect -- <disc> ls [prefix]`
//! `cargo run --release -p mb-app --example inspect -- <disc> gma <path in content root, e.g. smb1_content/test/st001/st001.gma>`

use mb_app::content::source_from_path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: inspect <disc> ls [prefix] | gma <path>");
        return;
    }
    if args[1] == "ls" {
        let disc = mb_assets::disc::DiscPack::open(&args[0]).expect("open disc");
        let prefix = args.get(2).map(String::as_str).unwrap_or("");
        for f in disc.list().into_iter().filter(|f| f.starts_with(prefix)) {
            println!("{f}");
        }
    } else if args[1] == "stage" {
        let content = source_from_path(&args[0]).open().expect("open");
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
        let content = source_from_path(&args[0]).open().expect("open");
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
        let content = source_from_path(&args[0]).open().expect("open");
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
        let content = source_from_path(&args[0]).open().expect("open");
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
