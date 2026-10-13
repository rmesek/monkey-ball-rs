//! Read game files straight out of the user's own GameCube disc image.
//!
//! Supported containers: RVZ with no compression or Zstandard ([`crate::rvz`]) and plain ISO/GCM. The reader
//! is this crate's own: the boot header, the file system table (FST) and `main.dol` are all it needs, and it
//! works on any [`DiscStream`] (a file on desktop, `Blob` slices in a browser worker). Nothing is trusted:
//! every offset and size from the image is checked against the image before it is used or allocated.
//!
//! Layout reference: the GameCube disc header (`boot.bin`): game id at 0, magic `C2339F3D` at 0x1C,
//! `main.dol` offset at 0x420, FST offset / size at 0x424 / 0x428. FST entries are 12 bytes: a type byte and
//! a 24-bit name offset, then (file) data offset and length, or (directory) parent index and the index one
//! past its last entry; the string table follows the entries.

use crate::pack::{PackProvider, disc_path};
use crate::rvz::RvzReader;
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Mutex;

/// Super Monkey Ball 1 (GameCube) disc IDs.
pub const SMB1_IDS: [(&str, &str); 3] =
    [("GMBE8P", "USA"), ("GMBP8P", "Europe"), ("GMBJ8P", "Japan")];

/// What the reader accepts, as told to the user.
pub const SUPPORTED: &str = "Supported: RVZ (uncompressed or Zstandard) and ISO/GCM images of Super Monkey Ball (USA/EUR/JPN).";

/// Pack path under which the game executable is served (it lives outside the disc's file table).
pub const MAIN_DOL_PATH: &str = "sys/main.dol";

const GC_MAGIC: [u8; 4] = [0xC2, 0x33, 0x9F, 0x3D];
const WII_MAGIC: [u8; 4] = [0x5D, 0x1C, 0x9E, 0xA3];
/// A GameCube disc holds 1,459,978,240 bytes; nothing larger is a GameCube image.
pub const MAX_DISC_SIZE: u64 = 0x5800_0000;
const BOOT_LEN: usize = 0x440;
/// Real tables are a few hundred kilobytes.
const MAX_FST_SIZE: u32 = 8 << 20;
const MAX_DOL_SIZE: u64 = 32 << 20;
const MAX_PATH_DEPTH: usize = 32;
/// Largest file [`Disc::read_file`] returns in one allocation (the game's own are far smaller; streams are
/// copied in pieces with [`Disc::read_range`]).
pub const MAX_WHOLE_FILE: u32 = 256 << 20;

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn unsupported(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, msg.into())
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Random-access byte source a disc image is read from.
pub trait DiscStream {
    /// Fill `buf` from `offset`; reading past the end is an error.
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()>;
    fn stream_len(&mut self) -> io::Result<u64>;
}

impl<T: DiscStream + ?Sized> DiscStream for Box<T> {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        (**self).read_exact_at(buf, offset)
    }
    fn stream_len(&mut self) -> io::Result<u64> {
        (**self).stream_len()
    }
}

/// An image held in memory (tests, small fixtures).
impl DiscStream for Vec<u8> {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let src = usize::try_from(offset)
            .ok()
            .and_then(|start| self.get(start..)?.get(..buf.len()))
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(src);
        Ok(())
    }
    fn stream_len(&mut self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

/// A file as a [`DiscStream`].
pub struct FileStream(std::fs::File);

impl FileStream {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self(std::fs::File::open(path)?))
    }
}

/// A file that is already open: where the platform hands out a file descriptor instead of a path (a
/// document the user picked on Android), `File::from(descriptor)` goes in here.
impl From<std::fs::File> for FileStream {
    fn from(file: std::fs::File) -> Self {
        Self(file)
    }
}

impl DiscStream for FileStream {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        use std::io::Seek;
        self.0.seek(io::SeekFrom::Start(offset))?;
        self.0.read_exact(buf)
    }

    fn stream_len(&mut self) -> io::Result<u64> {
        Ok(self.0.metadata()?.len())
    }
}

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
            Self::None => "no",
            Self::Purge => "purge",
            Self::Bzip2 => "bzip2",
            Self::Lzma => "LZMA",
            Self::Lzma2 => "LZMA2",
            Self::Zstandard => "Zstandard",
        }
    }

    pub fn supported(self) -> bool {
        matches!(self, Self::None | Self::Zstandard)
    }
}

/// What the first bytes of a file say it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// `compression` is `None` when the header is cut short or names an unknown method.
    Rvz {
        compression: Option<WiaCompression>,
    },
    Wia,
    /// Dolphin GCZ (zlib blocks).
    Gcz,
    Ciso,
    Wbfs,
    /// Plain GameCube ISO/GCM.
    Iso,
    WiiIso,
    Empty,
    Unknown,
}

/// Bytes of the image [`ImageKind::sniff`] wants to see (fewer is fine, the result is just less specific).
pub const SNIFF_LEN: usize = 0x50;

impl ImageKind {
    /// Identify the container from the start of the file without opening it.
    pub fn sniff(header: &[u8]) -> Self {
        let at = |o: usize| header.get(o..o + 4);
        match at(0) {
            None if header.is_empty() => Self::Empty,
            Some(b"RVZ\x01") => {
                // `wia_file_head_t` is 0x48 bytes; `wia_disc_t` follows: disc type, then compression.
                let compression = at(0x4C).and_then(|c| {
                    Some(match be32(c, 0) {
                        0 => WiaCompression::None,
                        1 => WiaCompression::Purge,
                        2 => WiaCompression::Bzip2,
                        3 => WiaCompression::Lzma,
                        4 => WiaCompression::Lzma2,
                        5 => WiaCompression::Zstandard,
                        _ => return None,
                    })
                });
                Self::Rvz { compression }
            }
            Some(b"WIA\x01") => Self::Wia,
            Some([0x01, 0xC0, 0x0B, 0xB1]) => Self::Gcz,
            Some(b"CISO") => Self::Ciso,
            Some(b"WBFS") => Self::Wbfs,
            _ if at(0x1C) == Some(&GC_MAGIC) => Self::Iso,
            _ if at(0x18) == Some(&WII_MAGIC) => Self::WiiIso,
            _ => Self::Unknown,
        }
    }

    /// `Err` with a message for the user when the file cannot be read. No advice about other tools: the
    /// message says what the file is and what is supported.
    pub fn check_supported(self) -> Result<(), String> {
        let what = match self {
            Self::Rvz {
                compression: Some(c),
            } if c.supported() => return Ok(()),
            Self::Iso => return Ok(()),
            Self::Rvz {
                compression: Some(c),
            } => format!("This RVZ image uses {} compression.", c.name()),
            Self::Rvz { compression: None } => {
                "This RVZ image is damaged or uses an unknown compression.".to_string()
            }
            Self::Wia => "This is a WIA image.".to_string(),
            Self::Gcz => "This is a GCZ image.".to_string(),
            Self::Ciso => "This is a CISO image.".to_string(),
            Self::Wbfs => "This is a WBFS image.".to_string(),
            Self::WiiIso => "This is a Wii disc image.".to_string(),
            Self::Empty => "The file is empty.".to_string(),
            Self::Unknown => "This file is not a GameCube disc image.".to_string(),
        };
        Err(format!("{what} {SUPPORTED}"))
    }
}

/// The decoded disc bytes of a supported image.
pub enum ImageStream<S: DiscStream> {
    Plain(S),
    /// Boxed: the reader carries its tables and cache, the plain stream is just the stream.
    Rvz(Box<RvzReader<S>>),
}

impl<S: DiscStream> ImageStream<S> {
    /// Identify the container behind `stream` and open it. Unsupported files come back as
    /// `ErrorKind::Unsupported` with a complete message for the user.
    pub fn open(mut stream: S) -> io::Result<Self> {
        let len = stream.stream_len()?;
        let mut header = vec![0u8; (SNIFF_LEN as u64).min(len) as usize];
        stream.read_exact_at(&mut header, 0)?;
        let kind = ImageKind::sniff(&header);
        kind.check_supported().map_err(unsupported)?;
        Ok(match kind {
            ImageKind::Rvz { .. } => Self::Rvz(Box::new(RvzReader::open(stream)?)),
            _ => Self::Plain(stream),
        })
    }
}

impl<S: DiscStream> DiscStream for ImageStream<S> {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.read_exact_at(buf, offset),
            Self::Rvz(r) => r.read_exact_at(buf, offset),
        }
    }
    fn stream_len(&mut self) -> io::Result<u64> {
        match self {
            Self::Plain(s) => s.stream_len(),
            Self::Rvz(r) => r.stream_len(),
        }
    }
}

/// Where a file's bytes are on the disc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub offset: u64,
    pub len: u32,
}

/// An opened GameCube disc: its id and file table over a stream of disc bytes.
pub struct Disc<S: DiscStream> {
    stream: ImageStream<S>,
    game_id: String,
    files: HashMap<String, FileEntry>,
}

/// Size of a DOL from its header: the end of the furthest text or data section.
fn dol_size(header: &[u8; 0x100]) -> u64 {
    let section = |offsets: usize, sizes: usize, i: usize| {
        u64::from(be32(header, offsets + i * 4)) + u64::from(be32(header, sizes + i * 4))
    };
    let text = (0..7).map(|i| section(0x00, 0x90, i));
    let data = (0..11).map(|i| section(0x1C, 0xAC, i));
    text.chain(data).fold(0x100, u64::max)
}

/// Parse an FST into `path -> entry`. `disc_size` bounds every file.
fn parse_fst(fst: &[u8], disc_size: u64) -> io::Result<HashMap<String, FileEntry>> {
    if fst.len() < 12 {
        return Err(invalid("the disc's file table is too small"));
    }
    let count = be32(fst, 8) as usize;
    if count == 0 || count > fst.len() / 12 {
        return Err(invalid(
            "the disc's file table has an impossible entry count",
        ));
    }
    let names = &fst[count * 12..];
    let name_at = |offset: usize| -> io::Result<String> {
        let tail = names
            .get(offset..)
            .ok_or_else(|| invalid("a file name lies outside the disc's file table"))?;
        let len = tail
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| invalid("an unterminated file name in the disc's file table"))?;
        Ok(String::from_utf8_lossy(&tail[..len]).into_owned())
    };
    let mut files = HashMap::new();
    // Open directories: (index one past the directory's last entry, its path with a trailing slash).
    let mut dirs: Vec<(usize, String)> = Vec::new();
    for i in 1..count {
        while dirs.last().is_some_and(|(end, _)| i >= *end) {
            dirs.pop();
        }
        let e = &fst[i * 12..i * 12 + 12];
        let name = name_at((be32(e, 0) & 0x00FF_FFFF) as usize)?;
        let prefix = dirs.last().map_or("", |(_, p)| p.as_str());
        if e[0] != 0 {
            let end = be32(e, 8) as usize;
            if end <= i || end > count || dirs.last().is_some_and(|(outer, _)| end > *outer) {
                return Err(invalid("a directory in the disc's file table is malformed"));
            }
            if dirs.len() >= MAX_PATH_DEPTH {
                return Err(invalid("the disc's directories are nested too deeply"));
            }
            dirs.push((end, format!("{prefix}{name}/")));
        } else {
            let entry = FileEntry {
                offset: u64::from(be32(e, 4)),
                len: be32(e, 8),
            };
            if entry.offset + u64::from(entry.len) > disc_size {
                return Err(invalid(format!(
                    "{prefix}{name} lies outside the disc image"
                )));
            }
            files.insert(format!("{prefix}{name}"), entry);
        }
    }
    Ok(files)
}

impl<S: DiscStream> Disc<S> {
    /// Open the image behind `stream`: container, boot header, file table and `main.dol`.
    pub fn open(stream: S) -> io::Result<Self> {
        let mut stream = ImageStream::open(stream)?;
        let size = stream.stream_len()?;
        if size < BOOT_LEN as u64 {
            return Err(invalid("the image is too small to be a GameCube disc"));
        }
        if size > MAX_DISC_SIZE {
            return Err(invalid("the image is larger than a GameCube disc"));
        }
        let mut boot = [0u8; BOOT_LEN];
        stream.read_exact_at(&mut boot, 0)?;
        if boot[0x1C..0x20] != GC_MAGIC {
            return Err(unsupported(format!(
                "This file does not hold a GameCube disc. {SUPPORTED}"
            )));
        }
        let game_id: String = boot[..6]
            .iter()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    *b as char
                } else {
                    '?'
                }
            })
            .collect();
        let dol_offset = u64::from(be32(&boot, 0x420));
        let fst_offset = u64::from(be32(&boot, 0x424));
        let fst_size = be32(&boot, 0x428);
        if fst_size > MAX_FST_SIZE || fst_offset + u64::from(fst_size) > size {
            return Err(invalid("the disc's file table lies outside the image"));
        }
        let mut fst = vec![0u8; fst_size as usize];
        stream.read_exact_at(&mut fst, fst_offset)?;
        let mut files = parse_fst(&fst, size)?;

        if dol_offset + 0x100 > size {
            return Err(invalid("the disc's executable lies outside the image"));
        }
        let mut dol_header = [0u8; 0x100];
        stream.read_exact_at(&mut dol_header, dol_offset)?;
        let dol_len = dol_size(&dol_header);
        if dol_len > MAX_DOL_SIZE || dol_offset + dol_len > size {
            return Err(invalid("the disc's executable has an impossible size"));
        }
        files.insert(
            MAIN_DOL_PATH.to_string(),
            FileEntry {
                offset: dol_offset,
                len: dol_len as u32,
            },
        );
        Ok(Self {
            stream,
            game_id,
            files,
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

    /// [`Self::smb1_region`], or the message for the user when this is some other game.
    pub fn require_smb1(&self) -> io::Result<&'static str> {
        self.smb1_region().ok_or_else(|| {
            unsupported(format!(
                "This is a GameCube disc image, but not Super Monkey Ball (disc id {}). {SUPPORTED}",
                self.game_id
            ))
        })
    }

    /// All file paths on the disc (plus [`MAIN_DOL_PATH`]), sorted.
    pub fn list(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.files.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    /// The normalised spelling of a path on the disc (see [`crate::pack::disc_path`]).
    pub fn disc_path(path: &str) -> String {
        disc_path(path)
    }

    pub fn entry(&self, path: &str) -> Option<FileEntry> {
        self.files.get(&Self::disc_path(path)).copied()
    }

    /// Read `buf.len()` bytes of a file starting `at` bytes into it.
    pub fn read_range(&mut self, entry: FileEntry, at: u32, buf: &mut [u8]) -> io::Result<()> {
        if u64::from(at) + buf.len() as u64 > u64::from(entry.len) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.stream.read_exact_at(buf, entry.offset + u64::from(at))
    }

    /// A whole file. `NotFound` when the disc has no such path.
    pub fn read_file(&mut self, path: &str) -> io::Result<Vec<u8>> {
        let entry = self.entry(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("not on disc: {}", Self::disc_path(path)),
            )
        })?;
        if entry.len > MAX_WHOLE_FILE {
            return Err(invalid(format!("{path} is too large to load")));
        }
        let mut out = vec![0u8; entry.len as usize];
        self.read_range(entry, 0, &mut out)?;
        Ok(out)
    }
}

/// A disc image as a [`PackProvider`] (desktop, tools, tests): files are read in place, on demand.
pub struct DiscPack {
    disc: Mutex<Disc<Box<dyn DiscStream + Send>>>,
    names: Vec<String>,
    game_id: String,
}

impl DiscPack {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_stream(Box::new(FileStream::open(path)?))
    }

    pub fn from_stream(stream: Box<dyn DiscStream + Send>) -> io::Result<Self> {
        let disc = Disc::open(stream)?;
        Ok(Self {
            names: disc.list().into_iter().map(String::from).collect(),
            game_id: disc.game_id().to_string(),
            disc: Mutex::new(disc),
        })
    }

    pub fn game_id(&self) -> &str {
        &self.game_id
    }

    pub fn smb1_region(&self) -> Option<&'static str> {
        SMB1_IDS
            .iter()
            .find(|(id, _)| *id == self.game_id)
            .map(|(_, r)| *r)
    }

    /// The game executable (`main.dol`). The engine reads its S16 trig tables from it.
    pub fn main_dol(&self) -> io::Result<Vec<u8>> {
        self.fetch(MAIN_DOL_PATH)
    }

    /// All file paths on the disc, sorted (the executable is listed as [`MAIN_DOL_PATH`]).
    pub fn list(&self) -> Vec<&str> {
        self.names.iter().map(String::as_str).collect()
    }

    pub fn contains(&self, path: &str) -> bool {
        let p = Disc::<Box<dyn DiscStream + Send>>::disc_path(path);
        self.names.binary_search(&p).is_ok()
    }

    /// Size of a file, if the disc has it.
    pub fn file_len(&self, path: &str) -> Option<u32> {
        self.disc.lock().unwrap().entry(path).map(|e| e.len)
    }
}

impl PackProvider for DiscPack {
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
        self.disc.lock().unwrap().read_file(path)
    }

    fn contains(&self, path: &str) -> bool {
        DiscPack::contains(self, path)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small, valid GameCube ISO: `files` are `(path, bytes)`; directories are created as needed (one
    /// level). Returns the image bytes.
    pub(crate) fn build_iso(game_id: &[u8; 6], files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut iso = vec![0u8; 0x2440];
        iso[..6].copy_from_slice(game_id);
        iso[0x1C..0x20].copy_from_slice(&GC_MAGIC);
        // main.dol: one text section of 0x20 bytes right after the header.
        let dol_at = 0x2440usize;
        let mut dol = vec![0u8; 0x120];
        dol[0..4].copy_from_slice(&0x100u32.to_be_bytes());
        dol[0x90..0x94].copy_from_slice(&0x20u32.to_be_bytes());
        for (i, b) in dol[0x100..].iter_mut().enumerate() {
            *b = 0xD0 + i as u8;
        }
        iso.extend(&dol);
        // Group files by directory, in order of first appearance.
        type Listing<'a> = Vec<(&'a str, &'a [u8])>;
        let mut dirs: Vec<(&str, Listing)> = Vec::new();
        for (path, data) in files {
            let (dir, name) = path.split_once('/').unwrap_or(("", path));
            match dirs.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, list)) => list.push((name, data)),
                None => dirs.push((dir, vec![(name, data)])),
            }
        }
        let mut entries: Vec<[u8; 12]> = vec![[0; 12]];
        let mut names = Vec::new();
        let mut data_at = iso.len();
        let mut blobs: Vec<&[u8]> = Vec::new();
        let push_name = |names: &mut Vec<u8>, n: &str| {
            let at = names.len() as u32;
            names.extend(n.as_bytes());
            names.push(0);
            at
        };
        for (dir, list) in &dirs {
            if !dir.is_empty() {
                let at = push_name(&mut names, dir);
                let index = entries.len();
                let mut e = [0u8; 12];
                e[0..4].copy_from_slice(&(0x0100_0000 | at).to_be_bytes());
                e[8..12].copy_from_slice(&((index + 1 + list.len()) as u32).to_be_bytes());
                entries.push(e);
            }
            for (name, data) in list {
                let at = push_name(&mut names, name);
                let mut e = [0u8; 12];
                e[0..4].copy_from_slice(&at.to_be_bytes());
                e[4..8].copy_from_slice(&(data_at as u32).to_be_bytes());
                e[8..12].copy_from_slice(&(data.len() as u32).to_be_bytes());
                entries.push(e);
                data_at += data.len();
                blobs.push(data);
            }
        }
        let count = entries.len() as u32;
        entries[0][0] = 1;
        entries[0][8..12].copy_from_slice(&count.to_be_bytes());
        for b in blobs {
            iso.extend(b);
        }
        let fst_at = iso.len();
        for e in &entries {
            iso.extend(e);
        }
        iso.extend(&names);
        let fst_len = iso.len() - fst_at;
        iso[0x420..0x424].copy_from_slice(&(dol_at as u32).to_be_bytes());
        iso[0x424..0x428].copy_from_slice(&(fst_at as u32).to_be_bytes());
        iso[0x428..0x42C].copy_from_slice(&(fst_len as u32).to_be_bytes());
        iso[0x42C..0x430].copy_from_slice(&(fst_len as u32).to_be_bytes());
        iso
    }

    fn sample() -> Vec<u8> {
        build_iso(
            b"GMBE8P",
            &[
                ("opening.bnr", vec![1, 2, 3]),
                ("test/a.lz", (0..200u8).collect()),
                ("test/b.tpl", vec![9; 40]),
            ],
        )
    }

    #[test]
    fn reads_files_the_executable_and_the_id() {
        let mut disc = Disc::open(sample()).unwrap();
        assert_eq!(disc.game_id(), "GMBE8P");
        assert_eq!(disc.smb1_region(), Some("USA"));
        assert_eq!(
            disc.list(),
            ["opening.bnr", "sys/main.dol", "test/a.lz", "test/b.tpl"]
        );
        assert_eq!(disc.read_file("opening.bnr").unwrap(), [1, 2, 3]);
        assert_eq!(
            disc.read_file("./test/a.lz").unwrap(),
            (0..200u8).collect::<Vec<_>>()
        );
        let dol = disc.read_file(MAIN_DOL_PATH).unwrap();
        assert_eq!((dol.len(), dol[0x100]), (0x120, 0xD0));
        let e = disc.entry("test/a.lz").unwrap();
        let mut part = [0u8; 4];
        disc.read_range(e, 10, &mut part).unwrap();
        assert_eq!(part, [10, 11, 12, 13]);
        assert!(disc.read_range(e, 198, &mut part).is_err());
        let missing = disc.read_file("test/nope").unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn normalises_disc_paths() {
        type D = Disc<Vec<u8>>;
        assert_eq!(
            D::disc_path("/test\\st001/STAGE001.lz"),
            "test/st001/STAGE001.lz"
        );
        assert_eq!(
            D::disc_path("./test/init/common.gma"),
            "test/init/common.gma"
        );
    }

    #[test]
    fn other_games_and_other_files_get_specific_messages() {
        let other = Disc::open(build_iso(b"GALE01", &[])).unwrap();
        let msg = other.require_smb1().unwrap_err().to_string();
        assert!(
            msg.contains("GALE01") && msg.contains("not Super Monkey Ball"),
            "{msg}"
        );

        for (bytes, expect) in [
            (Vec::new(), "empty"),
            (vec![0x5Au8; 4096], "not a GameCube disc image"),
            (b"WIA\x01".to_vec(), "WIA"),
            (b"CISO\0\0\0\0".to_vec(), "CISO"),
            (b"WBFS\0\0\0\0".to_vec(), "WBFS"),
            (vec![0x01, 0xC0, 0x0B, 0xB1], "GCZ"),
        ] {
            let e = Disc::open(bytes).err().expect("refused");
            assert_eq!(e.kind(), io::ErrorKind::Unsupported);
            let msg = e.to_string();
            assert!(
                msg.contains(expect) && msg.contains("Supported: RVZ"),
                "{msg}"
            );
            assert!(!msg.to_lowercase().contains("dolphin") && !msg.contains("onvert"));
        }
        let mut lzma = vec![0u8; SNIFF_LEN];
        lzma[..4].copy_from_slice(b"RVZ\x01");
        lzma[0x4F] = 4;
        let msg = Disc::open(lzma).err().unwrap().to_string();
        assert!(msg.contains("LZMA2"), "{msg}");
    }

    #[test]
    fn sniffs_containers() {
        let mut h = vec![0u8; SNIFF_LEN];
        h[..4].copy_from_slice(b"RVZ\x01");
        h[0x4F] = 5;
        assert_eq!(
            ImageKind::sniff(&h),
            ImageKind::Rvz {
                compression: Some(WiaCompression::Zstandard)
            }
        );
        assert!(ImageKind::sniff(&h).check_supported().is_ok());
        h[0x4F] = 9;
        assert_eq!(ImageKind::sniff(&h), ImageKind::Rvz { compression: None });
        assert_eq!(
            ImageKind::sniff(b"RVZ\x01"),
            ImageKind::Rvz { compression: None }
        );
        assert_eq!(ImageKind::sniff(&sample()[..SNIFF_LEN]), ImageKind::Iso);
        assert!(ImageKind::Iso.check_supported().is_ok());
        assert_eq!(ImageKind::sniff(&[]), ImageKind::Empty);
        assert_eq!(ImageKind::sniff(b"GMBE8P\0\0"), ImageKind::Unknown);
        let mut wii = vec![0u8; SNIFF_LEN];
        wii[0x18..0x1C].copy_from_slice(&WII_MAGIC);
        assert!(
            ImageKind::sniff(&wii)
                .check_supported()
                .unwrap_err()
                .contains("Wii")
        );
    }

    /// Hostile and damaged images: an error, never a panic or an allocation sized by the file's claims.
    #[test]
    fn damaged_images_are_errors() {
        let good = sample();
        // Truncated anywhere.
        for len in [0, 3, 0x1F, 0x20, 0x43F, 0x440, 0x2440, good.len() - 1] {
            assert!(
                Disc::open(good[..len].to_vec()).is_err(),
                "truncated to {len}"
            );
        }
        let set = |at: usize, v: u32| {
            let mut f = good.clone();
            f[at..at + 4].copy_from_slice(&v.to_be_bytes());
            f
        };
        let fst_at = be32(&good, 0x424) as usize;
        for (what, image) in [
            ("fst offset", set(0x424, 0xFFFF_FFF0)),
            ("fst size", set(0x428, 0xFFFF_FFFF)),
            ("fst size zero", set(0x428, 0)),
            ("dol offset", set(0x420, 0xFFFF_FF00)),
            ("dol section size", set(0x2440 + 0x90, 0xFFFF_FFFF)),
            ("entry count", set(fst_at + 8, 0x00FF_FFFF)),
            ("entry count zero", set(fst_at + 8, 0)),
            ("file offset", set(fst_at + 12 + 4, 0xFFFF_0000)),
            ("file length", set(fst_at + 12 + 8, 0xFFFF_FFFF)),
            ("name offset", set(fst_at + 12, 0x00FF_FFFF)),
            ("directory end", set(fst_at + 24 + 8, 0x7FFF_FFFF)),
            ("directory end backwards", set(fst_at + 24 + 8, 1)),
        ] {
            assert!(Disc::open(image).is_err(), "{what}");
        }
        // Every single-bit flip in the header and the file table: open and read everything, no panic.
        let ranges = [0x1Cusize..0x20, 0x420..0x430, fst_at..good.len()];
        for at in ranges.into_iter().flatten() {
            for bit in 0..8 {
                let mut f = good.clone();
                f[at] ^= 1 << bit;
                if let Ok(mut disc) = Disc::open(f) {
                    let names: Vec<String> = disc.list().into_iter().map(String::from).collect();
                    for n in names {
                        let _ = disc.read_file(&n);
                    }
                }
            }
        }
    }

    #[test]
    fn disc_pack_serves_files() {
        let pack = DiscPack::from_stream(Box::new(sample())).unwrap();
        assert_eq!(pack.game_id(), "GMBE8P");
        assert!(pack.contains("./test/b.tpl") && !pack.contains("test/c"));
        assert_eq!(pack.fetch("test/b.tpl").unwrap(), vec![9; 40]);
        assert_eq!(pack.main_dol().unwrap().len(), 0x120);
        assert_eq!(pack.file_len("opening.bnr"), Some(3));
    }
}
