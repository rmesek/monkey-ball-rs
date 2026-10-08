//! `cargo run -p xtask -- bundle-web`: assemble the static site in `web/dist/` (see `bundle.rs`).
//! `scripts/build-web.*` call this after building the wasm module. Standard library only.

use std::path::PathBuf;

mod bundle;

const USAGE: &str = "usage:
  xtask bundle-web [--single] [--pkg <dir>] [--out <dir>] [--wasm-gz <file>]
      assemble web/dist from web/ and the wasm-bindgen output (scripts/build-web runs it)";

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("bundle-web") => {
            let web = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("web");
            if let Err(e) = bundle::parse_args(args, web).and_then(bundle::run) {
                eprintln!("bundle-web: {e}");
                std::process::exit(2);
            }
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}
