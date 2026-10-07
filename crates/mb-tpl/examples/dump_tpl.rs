//! Dump one texture level of a (decompressed) TPL file as a binary PPM (alpha is dropped,
//! composited over a checkerboard) for eyeballing.
//!
//! usage: cargo run -p mb-tpl --example dump_tpl -- <file.tpl> [texture index] [level] [out.ppm]
//! With no index it lists the textures instead.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: dump_tpl <file.tpl> [index] [level] [out.ppm]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(path).expect("read tpl");
    let tpl = mb_tpl::parse(&bytes).expect("parse tpl");
    let Some(idx) = args.get(1) else {
        for t in tpl.textures() {
            println!(
                "{:3} {:?} {}x{} mips={}",
                t.index, t.format, t.width, t.height, t.mip_count
            );
        }
        return;
    };
    let idx: usize = idx.parse().expect("index");
    let level: usize = args.get(2).map_or(0, |s| s.parse().expect("level"));
    let out = args.get(3).map_or("out.ppm", |s| s.as_str());
    let tex = tpl.get(idx).expect("no such texture");
    let (w, h) = tex.level_size(level);
    let rgba = tex.decode_rgba(level).expect("decode");
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for y in 0..h {
        for x in 0..w {
            let p = &rgba[(y * w + x) * 4..][..4];
            let bg = if ((x / 8) + (y / 8)) % 2 == 0 {
                160
            } else {
                224
            };
            let a = p[3] as u32;
            for c in &p[..3] {
                ppm.push(((*c as u32 * a + bg * (255 - a)) / 255) as u8);
            }
        }
    }
    std::fs::write(out, ppm).expect("write ppm");
    println!("wrote {out} ({w}x{h})");
}
