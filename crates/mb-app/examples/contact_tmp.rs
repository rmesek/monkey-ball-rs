fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).unwrap();
    let tpl = mb_tpl::parse(&bytes).unwrap();
    let cols = 8usize;
    let cell = 136usize;
    let n = tpl.textures().iter().map(|t| t.index + 1).max().unwrap_or(0);
    let rows = n.div_ceil(cols);
    let (w, h) = (cols * cell, rows * cell);
    let mut img = vec![0u8; w * h * 4];
    for p in img.chunks_mut(4) { p.copy_from_slice(&[90, 90, 140, 255]); }
    for t in tpl.textures().iter() {
        let Ok(rgba) = t.decode_rgba(0) else { continue };
        let (tw, th) = t.level_size(0);
        let s = (cell - 8) as f32 / (tw.max(th) as f32);
        let s = s.min(1.0);
        let (dw, dh) = ((tw as f32 * s) as usize, (th as f32 * s) as usize);
        let ox = (t.index % cols) * cell + 4;
        let oy = (t.index / cols) * cell + 4;
        for y in 0..dh { for x in 0..dw {
            let sx = (x as f32 / s) as usize; let sy = (y as f32 / s) as usize;
            let p = &rgba[(sy * tw + sx) * 4..][..4];
            let o = ((oy + y) * w + ox + x) * 4;
            let al = p[3] as u32;
            for c in 0..3 { img[o + c] = ((p[c] as u32 * al + img[o + c] as u32 * (255 - al)) / 255) as u8; }
        }}
    }
    std::fs::write(&a[1], mb_app::image::encode_png(w as u32, h as u32, &img)).unwrap();
}
