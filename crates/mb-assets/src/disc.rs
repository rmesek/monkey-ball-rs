//! Read game files straight out of the user's own disc image (RVZ, WIA, ISO/GCM, CISO, ...),
//! so no pre-extracted folder or zip is needed. Nothing is written to disk: files are read
//! on demand through the same [`PackProvider`] interface as every other backend.

use crate::pack::{normalize_path, PackProvider};
use nod::read::{DiscOptions, DiscReader, PartitionOptions, PartitionReader};
use nod::{common::PartitionKind, disc::fst::Node};
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Mutex;

/// Super Monkey Ball 1 (GameCube) disc IDs.
pub const SMB1_IDS: [(&str, &str); 3] =
    [("GMBE8P", "USA"), ("GMBP8P", "Europe"), ("GMBJ8P", "Japan")];

/// Cheaply cloneable view of one big buffer (nod needs `AsRef<[u8]> + Clone + Send`).
#[derive(Clone)]
struct SharedBytes(std::sync::Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

pub struct DiscPack {
    partition: Mutex<Box<dyn PartitionReader>>,
    files: HashMap<String, Node>,
    game_id: String,
}

fn other<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(e.to_string())
}

impl DiscPack {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let disc = DiscReader::new(path, &DiscOptions::default()).map_err(other)?;
        Self::from_reader(disc)
    }

    /// Open a disc image held in memory (browser file picker, Android content copied into memory).
    /// `Arc<[u8]>` clones are cheap, which lets nod hand the stream to several readers.
    pub fn from_bytes(bytes: std::sync::Arc<[u8]>) -> io::Result<Self> {
        Self::from_stream(Box::new(bytes))
    }

    /// Like [`Self::from_bytes`] but takes ownership of a `Vec` without copying (a copy would double the
    /// memory of a ~0.5-1.4 GB image, which matters in a browser tab).
    pub fn from_vec(bytes: Vec<u8>) -> io::Result<Self> {
        Self::from_stream(Box::new(SharedBytes(std::sync::Arc::new(bytes))))
    }

    /// Open a disc image from any random-access [`nod::read::DiscStream`] (e.g. a Blob-backed stream).
    pub fn from_stream(stream: Box<dyn nod::read::DiscStream>) -> io::Result<Self> {
        let disc = DiscReader::new_stream(stream, &DiscOptions::default()).map_err(other)?;
        Self::from_reader(disc)
    }

    fn from_reader(disc: DiscReader) -> io::Result<Self> {
        let game_id = disc.header().game_id_str().to_string();
        let mut partition = disc
            .open_partition_kind(PartitionKind::Data, &PartitionOptions::default())
            .map_err(other)?;
        let meta = partition.meta().map_err(other)?;
        let fst = meta.fst().map_err(other)?;
        let files = fst
            .iter()
            .filter(|(_, node, _)| node.is_file())
            .map(|(_, node, path)| (normalize_path(&path), node))
            .collect();
        Ok(Self {
            partition: Mutex::new(partition),
            files,
            game_id,
        })
    }

    pub fn game_id(&self) -> &str {
        &self.game_id
    }

    /// Region name if this is a Super Monkey Ball 1 disc.
    pub fn smb1_region(&self) -> Option<&'static str> {
        SMB1_IDS
            .iter()
            .find(|(id, _)| *id == self.game_id)
            .map(|(_, r)| *r)
    }

    /// The game executable (`main.dol`) from the disc. The engine reads its S16 trig tables from it.
    pub fn main_dol(&self) -> io::Result<Vec<u8>> {
        let mut part = self.partition.lock().unwrap();
        let meta = part.meta().map_err(other)?;
        Ok(meta.raw_dol.to_vec())
    }

    /// All file paths on the disc, sorted.
    pub fn list(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.files.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    /// Content-root path (`smb1_content/test/x`) -> disc path (`test/x`).
    fn disc_path(path: &str) -> String {
        let norm = normalize_path(path);
        match norm.split_once('/') {
            Some((first, rest)) if first.ends_with("_content") => rest.to_string(),
            _ => norm,
        }
    }
}

impl PackProvider for DiscPack {
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
        let disc_path = Self::disc_path(path);
        let node = *self.files.get(&disc_path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("not on disc: {disc_path}"))
        })?;
        let mut part = self.partition.lock().unwrap();
        let mut out = Vec::with_capacity(node.length() as usize);
        part.open_file(node)?.read_to_end(&mut out)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_content_paths_to_disc_paths() {
        assert_eq!(
            DiscPack::disc_path("smb1_content/test/st001/STAGE001.lz"),
            "test/st001/STAGE001.lz"
        );
        assert_eq!(
            DiscPack::disc_path("./test/init/common.gma"),
            "test/init/common.gma"
        );
    }
}
