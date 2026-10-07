fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).unwrap();
    let tpl = mb_tpl::parse(&bytes).unwrap();
    let idx: usize = a[1].parse().unwrap();
    let t = tpl.get(idx).unwrap();
    let rgba = t.decode_rgba(0).unwrap();
    let (w, _h) = t.level_size(0);
    // row 1 (digits), cell 1 = '1' region x 8..16, y 16..32
    for y in 16..32 { let mut s = String::new(); for x in 8..24 { let p = &rgba[(y*w+x)*4..][..4]; s += &format!("{:02x}{:02x}{:02x}{:02x} ", p[0],p[1],p[2],p[3]); } println!("{s}"); }
}
