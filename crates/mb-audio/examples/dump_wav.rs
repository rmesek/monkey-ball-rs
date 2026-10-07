//! Decode audio from a disc image to WAV files for listening/inspection.
//!
//! `cargo run --release -p mb-audio --example dump_wav -- <disc> <out_dir> [adp|sfx|all]`
//! Write only to a scratch directory; never commit the output.

use mb_assets::disc::DiscPack;
use mb_assets::pack::PackProvider;
use mb_audio::adp::decode_adp;
use mb_audio::musyx::load_library;
use mb_audio::wav::wav_bytes_i16;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let disc = args.next().expect("disc path");
    let out = PathBuf::from(args.next().expect("out dir"));
    let what = args.next().unwrap_or_else(|| "all".into());
    std::fs::create_dir_all(&out).unwrap();
    let pack = DiscPack::open(&disc).expect("open disc");

    if what == "adp" || what == "all" {
        for f in pack.list() {
            if let Some(name) = f
                .strip_prefix("test/snd/adp/")
                .and_then(|n| n.strip_suffix(".adp"))
            {
                let a = decode_adp(&pack.fetch(f).unwrap()).unwrap();
                let peak = a
                    .samples
                    .iter()
                    .map(|s| s.unsigned_abs())
                    .max()
                    .unwrap_or(0);
                println!("{name}: {:.2}s peak {peak}", a.duration_secs());
                let wav = wav_bytes_i16(&a.samples, 2, a.sample_rate);
                std::fs::write(out.join(format!("{name}.wav")), wav).unwrap();
            }
        }
    }
    if what == "sfx" || what == "all" {
        let lib = load_library(|p| pack.fetch(p)).expect("banks");
        for id in lib.sfx_ids().collect::<Vec<_>>() {
            if let Some(r) = lib.render(id).unwrap() {
                let s = &r.sample;
                println!(
                    "sfx {id:#05x} sample {} {} Hz {} frames loop {:?}",
                    s.sample_id,
                    s.sample_rate,
                    s.pcm.len(),
                    s.loop_range
                );
                let wav = wav_bytes_i16(&s.pcm, 1, s.sample_rate);
                std::fs::write(out.join(format!("sfx_{id:04x}.wav")), wav).unwrap();
            }
        }
    }
}
