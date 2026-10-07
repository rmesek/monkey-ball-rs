fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).unwrap();
    let tpl = mb_tpl::parse(&bytes).unwrap();
    let idx: usize = a[1].parse().unwrap();
    let t = tpl.get(idx).unwrap();
    let rgba = t.decode_rgba(0).unwrap();
    let (w, h) = t.level_size(0);
    let ink = |x: usize, y: usize| rgba[(y * w + x) * 4 + 3] > 40;
    let rows: Vec<usize> = (0..h).map(|y| (0..w).filter(|&x| ink(x, y)).count()).collect();
    let cols: Vec<usize> = (0..w).map(|x| (0..h).filter(|&y| ink(x, y)).count()).collect();
    let runs = |v: &[usize]| { let mut r = vec![]; let mut s = None; for (i, &c) in v.iter().enumerate() { match (c > 0, s) { (true, None) => s = Some(i), (false, Some(b)) => { r.push((b, i)); s = None } _ => {} } } if let Some(b) = s { r.push((b, v.len())); } r };
    println!("{}x{}\nrows {:?}\ncols {:?}", w, h, runs(&rows), runs(&cols));
}
