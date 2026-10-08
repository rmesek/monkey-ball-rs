//! Proof that hardening the parsers did not change what valid data parses to: a hash over the `Debug` form
//! of every model (and its decoded meshes) on the disc, compared with the value recorded before the
//! hardening. Skips without a disc.

mod common;
use common::*;

/// FNV-1a over every parsed GMA / NaomiLib archive, USA disc, recorded at the pre-hardening parser.
const EXPECTED_GMA: (usize, u64) = (197, 0xc686_475a_aa54_8950);
const EXPECTED_NL: (usize, u64) = (38, 0x8153_c305_ebad_0956);

#[test]
fn every_gma_parses_exactly_as_before() {
    let Some(disc) = open_disc() else { return };
    let paths = disc_paths(&disc, |f| f.ends_with(".gma") || f.ends_with(".gma.lz"));
    let mut h = Fnv::default();
    for p in &paths {
        h.bytes(p.as_bytes());
        match mb_gma::parse_gma(&fetch(&disc, p)) {
            Ok(g) => {
                h.debug(&g.skipped);
                for m in g.iter() {
                    h.debug(m);
                    h.debug(&m.meshes());
                    assert!(std::ptr::eq(g.model_by_id(m.id).unwrap(), m));
                }
            }
            Err(e) => h.debug(&e),
        }
    }
    eprintln!("gma: ({}, {:#018x})", paths.len(), h.0);
    assert_eq!((paths.len(), h.0), EXPECTED_GMA);
}

#[test]
fn every_naomi_archive_parses_exactly_as_before() {
    let Some(disc) = open_disc() else { return };
    let paths = disc_paths(&disc, |f| f.ends_with("_p.lz"));
    let mut h = Fnv::default();
    for p in &paths {
        h.bytes(p.as_bytes());
        match mb_gma::parse_naomi_obj(&fetch(&disc, p)) {
            Ok(o) => {
                h.debug(&o.models);
                let mut names: Vec<_> = o.names.iter().collect();
                names.sort();
                h.debug(&names);
            }
            Err(e) => h.debug(&e),
        }
    }
    eprintln!("naomi: ({}, {:#018x})", paths.len(), h.0);
    assert_eq!((paths.len(), h.0), EXPECTED_NL);
}
