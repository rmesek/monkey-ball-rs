use mb_assets::disc::DiscPack;
use mb_assets::layout::{stage_files, GameSource};
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use std::collections::BTreeMap;
use std::path::PathBuf;

const USAGE: &str = "usage:
  xtask check-content [content_root] [stage_id] [bg_name]
  xtask disc-info <disc image>                       (RVZ, ISO, GCM, CISO, ...)
  xtask check-disc <disc image> [stage_id] [bg_name] (stage files + LZSS test, read in place)
  xtask disc-ls <disc image> [path prefix]
  xtask disc-cat <disc image> <disc path> <out file> [--lz]";

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("check-content") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| "content".into()));
            let stage: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            let bg = args.next().unwrap_or_else(|| "bg_jun".into());
            let missing: Vec<_> = stage_files(GameSource::Smb1, stage, &bg, false)
                .into_iter()
                .filter(|f| !root.join(f).is_file())
                .collect();
            report_missing(stage, &missing, &root.display().to_string());
        }
        Some("disc-info") => {
            let disc = open(args.next());
            println!("game id: {}", disc.game_id());
            match disc.smb1_region() {
                Some(r) => println!("Super Monkey Ball 1, {r}"),
                None => println!("not a known Super Monkey Ball 1 disc"),
            }
            let files = disc.list();
            println!("{} files", files.len());
            let mut dirs: BTreeMap<&str, usize> = BTreeMap::new();
            for f in &files {
                *dirs.entry(f.split('/').next().unwrap_or("")).or_default() += 1;
            }
            for (d, n) in dirs {
                println!("  {d}: {n}");
            }
        }
        Some("check-disc") => {
            let disc = open(args.next());
            let stage: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            let bg = args.next().unwrap_or_else(|| "bg_jun".into());
            let missing: Vec<_> = stage_files(GameSource::Smb1, stage, &bg, false)
                .into_iter()
                .filter(|f| disc.fetch(f).is_err())
                .collect();
            report_missing(stage, &missing, "disc");
            let path = format!("smb1_content/test/st{stage:03}/STAGE{stage:03}.lz");
            match disc.fetch(&path).map(|b| (b.len(), lzss::decompress(&b))) {
                Ok((n, Ok(out))) => {
                    println!("{path}: {n} bytes -> {} bytes decompressed", out.len())
                }
                Ok((_, Err(e))) => println!("{path}: LZSS error: {e}"),
                Err(e) => println!("{path}: {e}"),
            }
        }
        Some("disc-ls") => {
            let disc = open(args.next());
            let prefix = args.next().unwrap_or_default();
            for f in disc.list().into_iter().filter(|f| f.starts_with(&prefix)) {
                println!("{f}");
            }
        }
        Some("disc-cat") => {
            // Dump one file from the disc (optionally LZSS-decompressed) for local comparison.
            let disc = open(args.next());
            let (Some(path), Some(out)) = (args.next(), args.next()) else {
                eprintln!("{USAGE}");
                std::process::exit(2)
            };
            let mut bytes = disc.fetch(&path).expect("read file from disc");
            if args.next().as_deref() == Some("--lz") {
                bytes = lzss::decompress(&bytes).expect("lzss");
            }
            std::fs::write(&out, &bytes).expect("write output");
            println!("{} bytes -> {out}", bytes.len());
        }
        _ => eprintln!("{USAGE}"),
    }
}

fn open(path: Option<String>) -> DiscPack {
    let path = path.unwrap_or_else(|| {
        eprintln!("{USAGE}");
        std::process::exit(2)
    });
    DiscPack::open(&path).unwrap_or_else(|e| {
        eprintln!("cannot open {path}: {e}");
        std::process::exit(2)
    })
}

fn report_missing(stage: u32, missing: &[String], where_: &str) {
    if missing.is_empty() {
        println!("stage {stage:03}: all files present in {where_}");
    } else {
        println!("stage {stage:03}: {} missing file(s):", missing.len());
        for f in missing {
            println!("  {f}");
        }
        std::process::exit(1);
    }
}
