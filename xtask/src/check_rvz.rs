//! `xtask check-rvz`: prove the project's own disc and RVZ readers (`mb_assets::disc`, `mb_assets::rvz`)
//! against the `nod` crate on a real image: every byte of the decoded disc, the file list, `main.dol` and
//! the contents of every file.

use mb_assets::disc::{DiscPack, DiscStream, FileStream, ImageStream, MAIN_DOL_PATH};
use mb_assets::pack::PackProvider;
use nod::common::PartitionKind;
use nod::read::{DiscOptions, DiscReader, PartitionOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

fn other<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub fn compare(path: &Path) -> Result<String, String> {
    const BLOCK: u64 = 1 << 20;
    let mut reference = DiscReader::new(path, &DiscOptions::default()).map_err(other)?;
    let mut stream = ImageStream::open(FileStream::open(path).map_err(other)?).map_err(other)?;
    let size = stream.stream_len().map_err(other)?;
    if reference.disc_size() != size {
        return Err(format!(
            "disc size differs: nod {} / built-in {size}",
            reference.disc_size()
        ));
    }
    let (mut a, mut b) = (vec![0u8; BLOCK as usize], vec![0u8; BLOCK as usize]);
    for offset in (0..size).step_by(BLOCK as usize) {
        let n = BLOCK.min(size - offset) as usize;
        reference.seek(SeekFrom::Start(offset)).map_err(other)?;
        reference.read_exact(&mut a[..n]).map_err(other)?;
        stream.read_exact_at(&mut b[..n], offset).map_err(other)?;
        if let Some(at) = a[..n].iter().zip(&b[..n]).position(|(x, y)| x != y) {
            return Err(format!(
                "disc bytes differ at offset {:#x}",
                offset + at as u64
            ));
        }
    }

    let ours = DiscPack::open(path).map_err(other)?;
    if ours.game_id() != reference.header().game_id_str() {
        return Err("game id differs".into());
    }
    let mut partition = reference
        .open_partition_kind(PartitionKind::Data, &PartitionOptions::default())
        .map_err(other)?;
    let meta = partition.meta().map_err(other)?;
    if ours.main_dol().map_err(other)? != meta.raw_dol.as_ref() {
        return Err("main.dol differs".into());
    }
    let fst = meta.fst().map_err(other)?;
    let mut files: Vec<_> = fst
        .iter()
        .filter(|(_, node, _)| node.is_file())
        .map(|(_, node, path)| (path.to_string(), node))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mine: Vec<&str> = ours
        .list()
        .into_iter()
        .filter(|p| *p != MAIN_DOL_PATH)
        .collect();
    if mine.len() != files.len() || mine.iter().zip(&files).any(|(m, (t, _))| m != t) {
        return Err(format!(
            "file lists differ ({} files here, {} through nod)",
            mine.len(),
            files.len()
        ));
    }
    let mut file_bytes = 0u64;
    let mut expected = Vec::new();
    for (name, node) in &files {
        expected.clear();
        partition
            .open_file(*node)
            .map_err(other)?
            .read_to_end(&mut expected)
            .map_err(other)?;
        if ours.fetch(name).map_err(|e| format!("{name}: {e}"))? != expected {
            return Err(format!("{name} differs"));
        }
        file_bytes += expected.len() as u64;
    }
    Ok(format!(
        "identical through nod and the built-in readers: {size} disc bytes, {} files ({file_bytes} bytes), main.dol",
        files.len()
    ))
}
