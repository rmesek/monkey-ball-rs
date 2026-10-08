//! Proof that hardening the parser did not change what valid data decodes to: a hash over every texture
//! header and every decoded mip level on the disc, compared with the value recorded before the hardening.
//! Skips without a disc.

mod common;
use common::*;

/// FNV-1a over every TPL, USA disc, recorded at the pre-hardening parser.
const EXPECTED: (usize, u64) = (219, 0x5adb_8598_6e33_b810);

#[test]
fn every_tpl_decodes_exactly_as_before() {
    let Some(disc) = open_disc() else { return };
    let paths = disc_paths(&disc, |f| {
        (f.ends_with(".tpl") || f.ends_with(".tpl.lz")) && !f.starts_with("test/preview/")
    });
    let mut h = Fnv::default();
    for p in &paths {
        h.bytes(p.as_bytes());
        match mb_tpl::parse(&fetch(&disc, p)) {
            Ok(tpl) => {
                h.debug(&tpl.entry_count);
                for t in tpl.textures() {
                    h.debug(&(t.index, t.format, t.width, t.height, t.mip_count));
                    h.debug(&(t.data_offset, t.level_count()));
                    for l in 0..t.level_count() {
                        h.debug(&t.level_size(l));
                        match t.decode_rgba(l) {
                            Ok(px) => h.bytes(&px),
                            Err(e) => h.debug(&e),
                        }
                    }
                }
            }
            Err(e) => h.debug(&e),
        }
    }
    eprintln!("tpl: ({}, {:#018x})", paths.len(), h.0);
    assert_eq!((paths.len(), h.0), EXPECTED);
}
