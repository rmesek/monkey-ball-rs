//! Read game files straight out of the user's own disc image (RVZ, WIA, ISO/GCM, CISO, ...),
//! so no pre-extracted folder or zip is needed. Nothing is written to disk: files are read
//! on demand through the same [`PackProvider`] interface as every other backend.

use crate::pack::{normalize_path, PackProvider};
/// Random-access byte source nod reads a disc image from; implement it to feed [`DiscPack::from_stream`].
pub use nod::read::DiscStream;
use nod::read::{DiscOptions, DiscReader, PartitionOptions, PartitionReader};
use nod::{common::PartitionKind, disc::fst::Node};
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Mutex;

/// Super Monkey Ball 1 (GameCube) disc IDs.
pub const SMB1_IDS: [(&str, &str); 3] =
    [("GMBE8P", "USA"), ("GMBP8P", "Europe"), ("GMBJ8P", "Japan")];

/// Compression method of a WIA/RVZ container (the `compression` field of `wia_disc_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WiaCompression {
    None,
    Purge,
    Bzip2,
    Lzma,
    Lzma2,
    Zstandard,
}

impl WiaCompression {
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Purge => "purge",
            Self::Bzip2 => "bzip2",
            Self::Lzma => "LZMA",
            Self::Lzma2 => "LZMA2",
            Self::Zstandard => "Zstandard",
        }
    }

    /// Whether it can be decoded. `builtin`: by the pure-Rust RVZ reader ([`crate::rvz`], used on wasm32),
    /// which does "none" and Zstandard; otherwise by nod with its C codecs (everything but purge).
    pub fn supported(self, builtin: bool) -> bool {
        match self {
            Self::None | Self::Zstandard => true,
            Self::Bzip2 | Self::Lzma | Self::Lzma2 => !builtin,
            Self::Purge => false,
        }
    }
}

/// True where nod cannot decode compressed containers and the built-in RVZ reader is used instead (wasm32).
const BUILTIN_CONTAINERS: bool = cfg!(target_arch = "wasm32");

/// What the first bytes of a disc image say about its container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// `rvz` is false for the older WIA variant.
    Wia {
        rvz: bool,
        compression: Option<WiaCompression>,
    },
    /// Dolphin GCZ (zlib blocks).
    Gcz,
    /// Anything else: plain ISO/GCM, CISO, WBFS, TGC, ... (left to nod to identify).
    Other,
}

/// Bytes of the image [`ImageKind::sniff`] wants to see (fewer is fine, the result is just less specific).
pub const SNIFF_LEN: usize = 0x50;

impl ImageKind {
    /// Identify the container from the start of the file without opening it.
    pub fn sniff(header: &[u8]) -> Self {
        let be32 = |at: usize| {
            header
                .get(at..at + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        match header.get(..4) {
            Some(b"RVZ\x01") | Some(b"WIA\x01") => {
                // `wia_file_head_t` is 0x48 bytes; `wia_disc_t` follows: disc type, then compression.
                let compression = be32(0x4C).and_then(|c| {
                    Some(match c {
                        0 => WiaCompression::None,
                        1 => WiaCompression::Purge,
                        2 => WiaCompression::Bzip2,
                        3 => WiaCompression::Lzma,
                        4 => WiaCompression::Lzma2,
                        5 => WiaCompression::Zstandard,
                        _ => return None,
                    })
                });
                Self::Wia {
                    rvz: header[0] == b'R',
                    compression,
                }
            }
            Some([0x01, 0xC0, 0x0B, 0xB1]) => Self::Gcz,
            _ => Self::Other,
        }
    }

    /// `Err` with a message for the user when this build cannot read the container.
    pub fn check_supported(self) -> Result<(), String> {
        self.check_supported_by(BUILTIN_CONTAINERS)
    }

    /// [`Self::check_supported`] for a given reader: `builtin` = the pure-Rust path used in the browser
    /// (RVZ with "none"/Zstandard, and the plain containers), otherwise nod with its C codecs.
    pub fn check_supported_by(self, builtin: bool) -> Result<(), String> {
        const CONVERT: &str =
            "Convert it in Dolphin (right-click the game, Convert File, format RVZ, \
                               compression Zstandard), or use a plain ISO.";
        let build = if builtin {
            "in the browser build"
        } else {
            "in this build"
        };
        match self {
            Self::Wia {
                compression: Some(c),
                rvz,
            } if !c.supported(builtin) => Err(format!(
                "this {} image is compressed with {}, which is not supported {build} (supported: Zstandard, \
                 Dolphin's default, and none). {CONVERT}",
                if rvz { "RVZ" } else { "WIA" },
                c.name()
            )),
            Self::Wia { rvz: false, .. } if builtin => {
                Err(format!("WIA images are not supported {build}. {CONVERT}"))
            }
            Self::Gcz if builtin => Err(format!("GCZ images are not supported {build}. {CONVERT}")),
            _ => Ok(()),
        }
    }
}

/// A file as a [`DiscStream`] (clones share the handle; reads are serialised).
#[derive(Clone)]
pub struct FileStream(std::sync::Arc<Mutex<std::fs::File>>);

impl FileStream {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        Ok(Self(std::sync::Arc::new(Mutex::new(file))))
    }
}

impl DiscStream for FileStream {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        use std::io::Seek;
        let mut file = self.0.lock().unwrap();
        file.seek(io::SeekFrom::Start(offset))?;
        file.read_exact(buf)
    }

    fn stream_len(&mut self) -> io::Result<u64> {
        Ok(self.0.lock().unwrap().metadata()?.len())
    }
}

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

    /// Open a disc image from any random-access [`DiscStream`] (e.g. one backed by browser memory).
    ///
    /// On wasm32 an RVZ container is decoded by the built-in reader ([`crate::rvz`]) and nod parses the ISO it
    /// yields; containers that path cannot read are refused with a message that names them.
    pub fn from_stream(mut stream: Box<dyn DiscStream>) -> io::Result<Self> {
        if BUILTIN_CONTAINERS {
            let mut header = vec![0u8; SNIFF_LEN.min(stream.stream_len()? as usize)];
            stream.read_exact_at(&mut header, 0)?;
            let kind = ImageKind::sniff(&header);
            kind.check_supported()
                .map_err(|e| io::Error::new(io::ErrorKind::Unsupported, e))?;
            if matches!(kind, ImageKind::Wia { .. }) {
                stream = Box::new(crate::rvz::RvzReader::open(stream)?);
            }
        }
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

    fn wia_header(magic: &[u8; 4], compression: u32) -> Vec<u8> {
        let mut h = vec![0u8; SNIFF_LEN];
        h[..4].copy_from_slice(magic);
        h[0x4C..0x50].copy_from_slice(&compression.to_be_bytes());
        h
    }

    #[test]
    fn sniffs_wia_and_rvz_compression() {
        assert_eq!(
            ImageKind::sniff(&wia_header(b"RVZ\x01", 5)),
            ImageKind::Wia {
                rvz: true,
                compression: Some(WiaCompression::Zstandard)
            }
        );
        assert_eq!(
            ImageKind::sniff(&wia_header(b"WIA\x01", 0)),
            ImageKind::Wia {
                rvz: false,
                compression: Some(WiaCompression::None)
            }
        );
        // Unknown method number and a header cut short: still a WIA/RVZ, compression unknown.
        let unknown = ImageKind::Wia {
            rvz: true,
            compression: None,
        };
        assert_eq!(ImageKind::sniff(&wia_header(b"RVZ\x01", 9)), unknown);
        assert_eq!(ImageKind::sniff(b"RVZ\x01"), unknown);
        assert_eq!(
            ImageKind::sniff(&[0x01, 0xC0, 0x0B, 0xB1, 0, 0]),
            ImageKind::Gcz
        );
        assert_eq!(ImageKind::sniff(b"GMBE8P\0\0"), ImageKind::Other);
        assert_eq!(ImageKind::sniff(&[]), ImageKind::Other);
    }

    #[test]
    fn unsupported_compression_is_reported_by_name() {
        assert!(ImageKind::sniff(&wia_header(b"RVZ\x01", 0))
            .check_supported()
            .is_ok());
        assert!(ImageKind::sniff(&wia_header(b"RVZ\x01", 5))
            .check_supported()
            .is_ok());
        assert!(ImageKind::Other.check_supported().is_ok());
        let purge = ImageKind::sniff(&wia_header(b"WIA\x01", 1))
            .check_supported()
            .unwrap_err();
        assert!(purge.contains("WIA") && purge.contains("purge"), "{purge}");
        // nod with its C codecs (every host that runs this test) reads LZMA and WIA; the built-in reader
        // (browser) reads RVZ with none/Zstandard only and says what to do about the rest.
        let lzma = ImageKind::sniff(&wia_header(b"RVZ\x01", 4));
        assert!(lzma.check_supported_by(false).is_ok());
        let msg = lzma.check_supported_by(true).unwrap_err();
        assert!(msg.contains("LZMA2") && msg.contains("browser"), "{msg}");
        let wia = ImageKind::sniff(&wia_header(b"WIA\x01", 0));
        assert!(wia.check_supported_by(false).is_ok());
        assert!(wia.check_supported_by(true).unwrap_err().contains("WIA"));
        assert!(ImageKind::Gcz.check_supported_by(false).is_ok());
        assert!(ImageKind::Gcz
            .check_supported_by(true)
            .unwrap_err()
            .contains("GCZ"));
        for zstd_or_none in [0, 5] {
            let kind = ImageKind::sniff(&wia_header(b"RVZ\x01", zstd_or_none));
            assert!(kind.check_supported_by(true).is_ok());
        }
    }
}
