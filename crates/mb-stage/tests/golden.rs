//! Proof that hardening the parser did not change what valid data parses to: a hash over the `Debug` form of
//! every stagedef on the disc, compared with the value recorded before the hardening. Skips without a disc.

mod common;
use common::*;

/// FNV-1a over the `Debug` output of every parsed `STAGE*.lz`, USA disc, recorded at the pre-hardening parser.
const EXPECTED: (usize, u64) = (200, 0xa956_7762_8723_3ad9);

#[test]
fn every_stagedef_parses_exactly_as_before() {
    let Some(disc) = open_disc() else { return };
    let paths = disc_paths(&disc, |f| {
        f.starts_with("test/st")
            && f.rsplit('/')
                .next()
                .is_some_and(|n| n.starts_with("STAGE") && n.ends_with(".lz"))
    });
    let mut h = Fnv::default();
    for p in &paths {
        h.bytes(p.as_bytes());
        h.debug(&mb_stage::parse_stagedef_smb1(&fetch(&disc, p)));
    }
    eprintln!("stagedefs: ({}, {:#018x})", paths.len(), h.0);
    assert_eq!((paths.len(), h.0), EXPECTED);
}
