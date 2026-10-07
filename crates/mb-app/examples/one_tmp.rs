fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).unwrap();
    let tpl = mb_tpl::parse(&bytes).unwrap();
    let idx: usize = a[2].parse().unwrap();
    let t = tpl.get(idx).unwrap();
    let rgba = t.decode_rgba(0).unwrap();
    let (w, h) = t.level_size(0);
    let mut img = vec![0u8; w * h * 4];
    for (i, p) in img.chunks_mut(4).enumerate() {
        let s = &rgba[i * 4..][..4];
        let al = s[3] as u32;
        for c in 0..3 { p[c] = ((s[c] as u32 * al + 70 * (255 - al)) / 255) as u8; }
        p[3] = 255;
    }
    std::fs::write(&a[1], mb_app::image::encode_png(w as u32, h as u32, &img)).unwrap();
}
