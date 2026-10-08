use mb_assets::disc::DiscPack;
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use std::collections::BTreeMap;
use std::path::PathBuf;

mod bundle;
mod check_rvz;
mod serve;

const USAGE: &str = "usage:
  xtask disc-info <disc image>                       (RVZ none/Zstandard, ISO, GCM)
  xtask disc-ls <disc image> [path prefix]           (sizes and paths)
  xtask disc-cat <disc image> <disc path> <out file> [--lz]
  xtask check-rvz <disc image>                       (compare the built-in disc and RVZ readers with the
                                                      nod crate: every disc byte, the file list, every file)
  xtask bundle-web [--single] [--pkg <dir>] [--out <dir>] [--wasm-gz <file>]
                                                     (assemble web/dist from web/ and the wasm-bindgen
                                                      output; scripts/build-web runs it)
  xtask serve-web [port] [--root <dir>] [--lan] [--disc <disc image>]
                                                     (developer tool: serve web/dist on
                                                      http://localhost:8080; --lan for a phone on the same
                                                      network; see docs/WEB.md)";

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
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
        Some("disc-ls") => {
            let disc = open(args.next());
            let prefix = args.next().unwrap_or_default();
            let mut total = 0u64;
            for f in disc.list().into_iter().filter(|f| f.starts_with(&prefix)) {
                let len = disc.file_len(f).unwrap_or(0);
                total += u64::from(len);
                println!("{len:>10}  {f}");
            }
            println!("{total:>10}  total");
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
        Some("check-rvz") => {
            let Some(path) = args.next() else {
                eprintln!("{USAGE}");
                std::process::exit(2)
            };
            match check_rvz::compare(std::path::Path::new(&path)) {
                Ok(report) => println!("{path}: {report}"),
                Err(e) => {
                    eprintln!("{path}: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("bundle-web") => {
            let web = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("web");
            if let Err(e) = bundle::parse_args(args, web).and_then(bundle::run) {
                eprintln!("bundle-web: {e}");
                std::process::exit(2);
            }
        }
        Some("serve-web") => {
            // The workspace's web/ directory, wherever cargo was started from.
            let web = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("web");
            let result = serve::parse_args(args, web).and_then(serve::run);
            if let Err(e) = result {
                eprintln!("serve-web: {e}");
                std::process::exit(2);
            }
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
