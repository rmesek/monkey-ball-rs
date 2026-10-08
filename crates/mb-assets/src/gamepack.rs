//! The on-device game pack: the files the game uses, copied once out of the user's disc image into one
//! file (the browser keeps it in the Origin Private File System; see `docs/WEB.md`).
//!
//! Layout, little-endian:
//!
//! ```text
//! 0   "MBPK"
//! 4   u32 format version
//! 8   disc id (6 bytes), 2 zero bytes
//! 16  u64 index offset
//! 24  u32 index length
//! 28  u32 CRC-32 of the index
//! 32  file data, back to back
//! ..  index: u32 count, then per file: u16 name length, name, u64 offset, u32 length, u32 CRC-32
//! ```
//!
//! The header is written last, so a pack whose import was interrupted has no valid header. Every file
//! carries a CRC-32 that is checked when it is read back: storage that lost or changed bytes is an error
//! with the file's name, not a broken stage.

use crate::pack::disc_path;
use std::collections::HashMap;
use std::io;

pub const MAGIC: [u8; 4] = *b"MBPK";
/// Bump when the layout or the set of files the game needs changes: older packs are then re-imported.
pub const VERSION: u32 = 1;
pub const HEADER_LEN: usize = 32;
const MAX_INDEX_LEN: u32 = 4 << 20;
const MAX_NAME_LEN: usize = 255;

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// Running CRC-32 (IEEE, as in zip and PNG).
#[derive(Clone, Copy)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self(!0)
    }
}

impl Crc32 {
    pub fn update(&mut self, data: &[u8]) {
        for b in data {
            self.0 = CRC_TABLE[((self.0 ^ u32::from(*b)) & 0xFF) as usize] ^ (self.0 >> 8);
        }
    }

    pub fn finish(self) -> u32 {
        !self.0
    }

    pub fn of(data: &[u8]) -> u32 {
        let mut c = Self::default();
        c.update(data);
        c.finish()
    }
}

/// Where one file sits in the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub offset: u64,
    pub len: u32,
    pub crc: u32,
}

/// Lays a pack out while its data is streamed to wherever it is stored. The caller writes each piece
/// [`PackWriter::data`] is given at the offset it returns, then the index and header from
/// [`PackWriter::finish`].
pub struct PackWriter {
    game_id: [u8; 6],
    entries: Vec<(String, Entry)>,
    pos: u64,
    current: Option<(String, u64, u32, Crc32, u32)>,
}

/// What [`PackWriter::finish`] hands back: write `index` at `index_offset`, then `header` at 0.
pub struct Finished {
    pub header: [u8; HEADER_LEN],
    pub index: Vec<u8>,
    pub index_offset: u64,
    /// Size of the complete pack file.
    pub total_len: u64,
}

impl PackWriter {
    pub fn new(game_id: &str) -> Self {
        let mut id = [0u8; 6];
        for (o, b) in id.iter_mut().zip(game_id.bytes()) {
            *o = b;
        }
        Self {
            game_id: id,
            entries: Vec::new(),
            pos: HEADER_LEN as u64,
            current: None,
        }
    }

    /// Start the file `name` (a disc path) of `len` bytes.
    pub fn begin(&mut self, name: &str, len: u32) -> io::Result<()> {
        let name = disc_path(name);
        if self.current.is_some() {
            return Err(invalid("pack: previous file not finished"));
        }
        if name.is_empty() || name.len() > MAX_NAME_LEN {
            return Err(invalid("pack: bad file name"));
        }
        self.current = Some((name, self.pos, len, Crc32::default(), 0));
        Ok(())
    }

    /// Account for the next piece of the current file; returns the pack offset to write it at.
    pub fn data(&mut self, bytes: &[u8]) -> io::Result<u64> {
        let (_, start, len, crc, done) = self
            .current
            .as_mut()
            .ok_or_else(|| invalid("pack: no file started"))?;
        if bytes.len() as u64 + u64::from(*done) > u64::from(*len) {
            return Err(invalid("pack: more data than the file's length"));
        }
        let at = *start + u64::from(*done);
        crc.update(bytes);
        *done += bytes.len() as u32;
        Ok(at)
    }

    pub fn end(&mut self) -> io::Result<()> {
        let (name, offset, len, crc, done) = self
            .current
            .take()
            .ok_or_else(|| invalid("pack: no file started"))?;
        if done != len {
            return Err(invalid(format!("pack: {name} is incomplete")));
        }
        self.pos = offset + u64::from(len);
        self.entries.push((
            name,
            Entry {
                offset,
                len,
                crc: crc.finish(),
            },
        ));
        Ok(())
    }

    /// Bytes laid out so far (header and finished files).
    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn finish(self) -> io::Result<Finished> {
        if self.current.is_some() {
            return Err(invalid("pack: last file not finished"));
        }
        let mut index = Vec::new();
        index.extend((self.entries.len() as u32).to_le_bytes());
        for (name, e) in &self.entries {
            index.extend((name.len() as u16).to_le_bytes());
            index.extend(name.as_bytes());
            index.extend(e.offset.to_le_bytes());
            index.extend(e.len.to_le_bytes());
            index.extend(e.crc.to_le_bytes());
        }
        if index.len() > MAX_INDEX_LEN as usize {
            return Err(invalid("pack: index too large"));
        }
        let mut header = [0u8; HEADER_LEN];
        header[..4].copy_from_slice(&MAGIC);
        header[4..8].copy_from_slice(&VERSION.to_le_bytes());
        header[8..14].copy_from_slice(&self.game_id);
        header[16..24].copy_from_slice(&self.pos.to_le_bytes());
        header[24..28].copy_from_slice(&(index.len() as u32).to_le_bytes());
        header[28..32].copy_from_slice(&Crc32::of(&index).to_le_bytes());
        Ok(Finished {
            header,
            total_len: self.pos + index.len() as u64,
            index_offset: self.pos,
            index,
        })
    }
}

/// The fixed-size start of a pack file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub game_id: [u8; 6],
    pub index_offset: u64,
    pub index_len: u32,
    index_crc: u32,
}

impl Header {
    /// Check the first [`HEADER_LEN`] bytes of a pack of `file_len` bytes.
    pub fn parse(bytes: &[u8], file_len: u64) -> io::Result<Self> {
        let b: &[u8; HEADER_LEN] = bytes
            .get(..HEADER_LEN)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| invalid("game data is incomplete (no header)"))?;
        if b[..4] != MAGIC {
            return Err(invalid("game data is incomplete or damaged (bad header)"));
        }
        let version = u32::from_le_bytes(b[4..8].try_into().unwrap());
        if version != VERSION {
            return Err(invalid(format!(
                "game data has format version {version}, this build reads {VERSION}"
            )));
        }
        let index_offset = u64::from_le_bytes(b[16..24].try_into().unwrap());
        let index_len = u32::from_le_bytes(b[24..28].try_into().unwrap());
        if index_len > MAX_INDEX_LEN
            || index_offset < HEADER_LEN as u64
            || index_offset.checked_add(u64::from(index_len)) != Some(file_len)
        {
            return Err(invalid("game data is incomplete (its size does not match)"));
        }
        Ok(Self {
            game_id: b[8..14].try_into().unwrap(),
            index_offset,
            index_len,
            index_crc: u32::from_le_bytes(b[28..32].try_into().unwrap()),
        })
    }

    pub fn game_id(&self) -> String {
        String::from_utf8_lossy(&self.game_id).into_owned()
    }
}

/// The file table of a pack.
#[derive(Debug, Default)]
pub struct PackIndex {
    entries: HashMap<String, Entry>,
}

impl PackIndex {
    /// Parse the index bytes `header` points at.
    pub fn parse(header: &Header, index: &[u8]) -> io::Result<Self> {
        if index.len() != header.index_len as usize || Crc32::of(index) != header.index_crc {
            return Err(invalid("game data is damaged (file table)"));
        }
        let bad = || invalid("game data is damaged (file table entry)");
        let mut at = 0usize;
        let mut take = |n: usize| -> io::Result<&[u8]> {
            let s = index.get(at..at + n).ok_or_else(bad)?;
            at += n;
            Ok(s)
        };
        let count = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
        // Each entry is at least 19 bytes, so a count from the file cannot make this reserve too much.
        if count > index.len() / 19 {
            return Err(bad());
        }
        let mut entries = HashMap::with_capacity(count);
        for _ in 0..count {
            let name_len = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
            let name = std::str::from_utf8(take(name_len)?)
                .map_err(|_| bad())?
                .to_string();
            let offset = u64::from_le_bytes(take(8)?.try_into().unwrap());
            let len = u32::from_le_bytes(take(4)?.try_into().unwrap());
            let crc = u32::from_le_bytes(take(4)?.try_into().unwrap());
            if offset < HEADER_LEN as u64 || offset + u64::from(len) > header.index_offset {
                return Err(bad());
            }
            entries.insert(name, Entry { offset, len, crc });
        }
        Ok(Self { entries })
    }

    pub fn get(&self, path: &str) -> Option<Entry> {
        self.entries.get(&disc_path(path)).copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Check bytes read back for `path` against the entry's length and checksum.
    pub fn verify(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let e = self
            .get(path)
            .ok_or_else(|| invalid(format!("{path} is not part of the game data")))?;
        if bytes.len() != e.len as usize || Crc32::of(bytes) != e.crc {
            return Err(invalid(format!(
                "{} was damaged in storage",
                disc_path(path)
            )));
        }
        Ok(())
    }
}

/// Build a whole pack in memory (tests and tools; the importer streams instead).
pub fn build(game_id: &str, files: &[(&str, &[u8])]) -> io::Result<Vec<u8>> {
    let mut w = PackWriter::new(game_id);
    let mut out = vec![0u8; HEADER_LEN];
    for (name, data) in files {
        w.begin(name, data.len() as u32)?;
        let at = w.data(data)? as usize;
        debug_assert_eq!(at, out.len());
        out.extend_from_slice(data);
        w.end()?;
    }
    let done = w.finish()?;
    out.extend(&done.index);
    out[..HEADER_LEN].copy_from_slice(&done.header);
    Ok(out)
}

/// A complete pack held in memory, as a [`crate::pack::PackProvider`] (tests).
pub struct PackBytes {
    bytes: Vec<u8>,
    index: PackIndex,
}

impl PackBytes {
    pub fn open(bytes: Vec<u8>) -> io::Result<Self> {
        let header = Header::parse(&bytes, bytes.len() as u64)?;
        let index = PackIndex::parse(&header, &bytes[header.index_offset as usize..])?;
        Ok(Self { bytes, index })
    }

    pub fn index(&self) -> &PackIndex {
        &self.index
    }
}

impl crate::pack::PackProvider for PackBytes {
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
        let e = self.index.get(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing pack entry: {}", disc_path(path)),
            )
        })?;
        let data = &self.bytes[e.offset as usize..e.offset as usize + e.len as usize];
        self.index.verify(path, data)?;
        Ok(data.to_vec())
    }

    fn contains(&self, path: &str) -> bool {
        self.index.get(path).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::PackProvider;

    fn sample() -> Vec<u8> {
        build(
            "GMBE8P",
            &[
                ("test/a.lz", &[1, 2, 3, 4, 5]),
                ("./test/b.tpl", &[9; 300]),
                ("sys/main.dol", &[]),
            ],
        )
        .unwrap()
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(Crc32::of(b"123456789"), 0xCBF4_3926);
        assert_eq!(Crc32::of(b""), 0);
    }

    #[test]
    fn round_trips_and_checks_what_it_reads() {
        let bytes = sample();
        let header = Header::parse(&bytes, bytes.len() as u64).unwrap();
        assert_eq!(header.game_id(), "GMBE8P");
        let pack = PackBytes::open(bytes.clone()).unwrap();
        assert_eq!(pack.index().len(), 3);
        assert_eq!(pack.fetch("test/a.lz").unwrap(), [1, 2, 3, 4, 5]);
        assert_eq!(pack.fetch("test/b.tpl").unwrap(), vec![9; 300]);
        assert!(pack.fetch("sys/main.dol").unwrap().is_empty());
        assert_eq!(
            pack.fetch("test/c").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        // One flipped data byte: that file reports damage by name, the others still read.
        let mut damaged = bytes;
        damaged[HEADER_LEN + 2] ^= 0x10;
        let pack = PackBytes::open(damaged).unwrap();
        let e = pack.fetch("test/a.lz").unwrap_err().to_string();
        assert!(e.contains("test/a.lz") && e.contains("damaged"), "{e}");
        assert!(pack.fetch("test/b.tpl").is_ok());
    }

    #[test]
    fn writer_refuses_misuse() {
        let mut w = PackWriter::new("GMBE8P");
        assert!(w.data(&[1]).is_err());
        w.begin("a", 2).unwrap();
        assert!(w.begin("b", 1).is_err());
        assert_eq!(w.data(&[1]).unwrap(), HEADER_LEN as u64);
        assert!(w.data(&[1, 2]).is_err());
        assert!(w.end().is_err(), "one byte short");
    }

    /// Interrupted, truncated and damaged packs: an error, never a panic or a large allocation.
    #[test]
    fn incomplete_and_damaged_packs_are_errors() {
        let good = sample();
        for len in 0..good.len() {
            assert!(
                PackBytes::open(good[..len].to_vec()).is_err(),
                "cut to {len}"
            );
        }
        // The header is written last: without it nothing is accepted.
        let mut no_header = good.clone();
        no_header[..HEADER_LEN].fill(0);
        assert!(PackBytes::open(no_header).is_err());
        let header = Header::parse(&good, good.len() as u64).unwrap();
        let index_at = header.index_offset as usize;
        for at in (0..HEADER_LEN).chain(index_at..good.len()) {
            for bit in 0..8 {
                let mut f = good.clone();
                f[at] ^= 1 << bit;
                let (result, peak) = crate::test_alloc::peak_during(|| PackBytes::open(f));
                assert!(peak < 1 << 20, "byte {at} bit {bit}: allocated {peak}");
                // The disc id and reserved bytes are not covered by a checksum; everything else is.
                if !(8..16).contains(&at) {
                    assert!(result.is_err(), "byte {at} bit {bit} accepted");
                }
            }
        }
        let mut other_version = good.clone();
        other_version[4] = 9;
        let e = PackBytes::open(other_version).err().unwrap().to_string();
        assert!(e.contains("version 9"), "{e}");
    }
}
