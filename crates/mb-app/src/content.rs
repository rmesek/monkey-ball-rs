//! Where game files come from. Desktop opens a disc image or a content directory; Android/web will add
//! file-picker based sources behind the same trait.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mb_assets::pack::{DirPack, PackProvider};

/// Something that can be opened into a [`Content`] handle (possibly slow: reads the disc FST, etc.).
pub trait ContentSource {
    fn describe(&self) -> String;
    fn open(&self) -> io::Result<Content>;
}

/// Disc image (RVZ, ISO, ...) read in place through `mb-assets`.
pub struct DiscImageSource(pub PathBuf);

impl ContentSource for DiscImageSource {
    fn describe(&self) -> String {
        format!("disc image {}", self.0.display())
    }
    fn open(&self) -> io::Result<Content> {
        let disc = mb_assets::disc::DiscPack::open(&self.0)?;
        if disc.smb1_region().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} is not a Super Monkey Ball 1 disc ({})",
                    self.0.display(),
                    disc.game_id()
                ),
            ));
        }
        Ok(Content::new(Arc::new(disc)))
    }
}

/// A directory laid out like the web build's content root.
pub struct DirSource(pub PathBuf);

impl ContentSource for DirSource {
    fn describe(&self) -> String {
        format!("content directory {}", self.0.display())
    }
    fn open(&self) -> io::Result<Content> {
        Ok(Content::new(Arc::new(DirPack::new(&self.0))))
    }
}

/// Pick a source from a user supplied path: directories are content roots, files are disc images.
pub fn source_from_path(path: impl AsRef<Path>) -> Box<dyn ContentSource> {
    let p = path.as_ref().to_path_buf();
    if p.is_dir() {
        Box::new(DirSource(p))
    } else {
        Box::new(DiscImageSource(p))
    }
}

/// Opened content: raw file access plus the `.lz` conveniences every loader needs.
#[derive(Clone)]
pub struct Content {
    pack: Arc<dyn PackProvider>,
}

impl Content {
    pub fn new(pack: Arc<dyn PackProvider>) -> Self {
        Self { pack }
    }

    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.pack.fetch(path)
    }

    /// Read and LZSS-decompress a `.lz` file.
    pub fn read_lz(&self, path: &str) -> io::Result<Vec<u8>> {
        let raw = self.read(path)?;
        mb_assets::lzss::decompress(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{path}: {e}")))
    }

    /// Read `path`; if it does not exist, try `path + ".lz"` and decompress that.
    pub fn read_maybe_lz(&self, path: &str) -> io::Result<Vec<u8>> {
        match self.read(path) {
            Ok(b) => Ok(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.read_lz(&format!("{path}.lz")),
            Err(e) => Err(e),
        }
    }
}
