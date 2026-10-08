//! Pure-Rust reader for GameCube RVZ disc images: presents the original ISO bytes of an `.rvz` file as a
//! random-access stream, decoding groups on demand.
//!
//! Why this exists next to `nod`: nod's WIA/RVZ reader calls `std::time::Instant::now()`, which panics on
//! `wasm32-unknown-unknown`, and its Zstandard support is the C library. In the browser the RVZ container is
//! therefore decoded here (Zstandard through `ruzstd`) and nod only sees a plain ISO stream, whose code path
//! needs neither. Native builds keep using nod for everything; the tests compare the two byte for byte.
//!
//! Supported: RVZ of a GameCube disc, compression "none" or Zstandard, including RVZ-packed junk data.
//! Not supported: Wii discs (partition data and hash exceptions), the older WIA variant, bzip2/LZMA/LZMA2/purge.
//! The SHA-1 hashes stored in the headers are not checked; sizes and offsets are validated instead.
//!
//! Format reference (CC0): Dolphin's `docs/WiaAndRvz.md`; the lagged Fibonacci generator for junk data follows
//! Dolphin's `LaggedFibonacciGenerator.cpp` as described there.

use crate::disc::DiscStream;
use std::io::{self, Read};
use std::sync::Arc;

const SECTOR: u64 = 0x8000;
const FILE_HEAD_LEN: usize = 0x48;
const DISC_LEN: usize = 0xDC;
const DISC_HEAD_LEN: usize = 0x80;
const RAW_DATA_LEN: usize = 0x18;
const GROUP_LEN: usize = 0x0C;
/// Highest `version_compatible` this reader understands (RVZ 1.0).
const RVZ_VERSION: u32 = 0x0100_0000;
const TOP_BIT: u32 = 0x8000_0000;
/// Decoded groups kept per reader, by total size. nod caches what it reads as well; this only has to make
/// neighbouring small reads inside one group cheap.
const CACHE_BYTES: usize = 4 << 20;

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be64(b: &[u8], at: usize) -> u64 {
    (u64::from(be32(b, at)) << 32) | u64::from(be32(b, at + 4))
}

/// One contiguous stretch of disc data (`wia_raw_data_t`), in disc byte offsets.
#[derive(Clone, Copy, Debug)]
struct RawData {
    /// Rounded down to a sector, as the format specifies.
    start: u64,
    end: u64,
    group_index: u32,
    num_groups: u32,
}

/// `rvz_group_t`: where one chunk of disc data is stored in the file.
#[derive(Clone, Copy, Debug)]
struct Group {
    offset: u64,
    size: u32,
    compressed: bool,
    packed: bool,
}

struct Meta {
    iso_size: u64,
    chunk_size: u64,
    zstd: bool,
    disc_head: [u8; DISC_HEAD_LEN],
    raw: Vec<RawData>,
    groups: Vec<Group>,
}

/// The ISO contents of an RVZ file. Cloning is cheap (tables are shared; each clone has its own small cache).
#[derive(Clone)]
pub struct RvzReader {
    inner: Box<dyn DiscStream>,
    meta: Arc<Meta>,
    /// Most recently used first: (group index, decoded bytes).
    cache: Vec<(u32, Arc<[u8]>)>,
}

fn read_vec(inner: &mut dyn DiscStream, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0u8; len];
    inner.read_exact_at(&mut v, offset)?;
    Ok(v)
}

fn zstd_decode(data: &[u8], size_hint: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(size_hint);
    let mut src = data;
    while !src.is_empty() {
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&mut src)
            .map_err(|e| invalid(format!("Zstandard frame: {e}")))?;
        decoder
            .read_to_end(&mut out)
            .map_err(|e| invalid(format!("Zstandard data: {e}")))?;
    }
    Ok(out)
}

/// Read a table that is stored with the image's compression method and must decode to exactly `len` bytes.
fn read_table(
    inner: &mut dyn DiscStream,
    what: &str,
    offset: u64,
    stored: u32,
    len: usize,
    zstd: bool,
    file_len: u64,
) -> io::Result<Vec<u8>> {
    if offset
        .checked_add(u64::from(stored))
        .is_none_or(|end| end > file_len)
    {
        return Err(invalid(format!("RVZ {what} table lies outside the file")));
    }
    let data = read_vec(inner, offset, stored as usize)?;
    let table = if zstd { zstd_decode(&data, len)? } else { data };
    if table.len() != len {
        return Err(invalid(format!(
            "RVZ {what} table has {} bytes, expected {len}",
            table.len()
        )));
    }
    Ok(table)
}

impl RvzReader {
    /// Parse the headers and tables of the RVZ file behind `inner`.
    pub fn open(mut inner: Box<dyn DiscStream>) -> io::Result<Self> {
        let file_len = inner.stream_len()?;
        if file_len < (FILE_HEAD_LEN + 0x10) as u64 {
            return Err(invalid("file too small for an RVZ image"));
        }
        let head = read_vec(inner.as_mut(), 0, FILE_HEAD_LEN)?;
        if &head[..4] != b"RVZ\x01" {
            return Err(invalid("not an RVZ image"));
        }
        let version_compatible = be32(&head, 0x08);
        if version_compatible > RVZ_VERSION {
            return Err(invalid(format!(
                "RVZ format version {version_compatible:#010x} is newer than this reader"
            )));
        }
        let disc_len = be32(&head, 0x0C) as usize;
        let iso_size = be64(&head, 0x24);
        // Older writers may store a shorter struct; missing fields read as zero.
        let mut disc = read_vec(inner.as_mut(), FILE_HEAD_LEN as u64, disc_len.min(DISC_LEN))?;
        disc.resize(DISC_LEN, 0);

        if be32(&disc, 0x00) != 1 || be32(&disc, 0x90) != 0 {
            return Err(invalid(
                "this RVZ holds a Wii disc; only GameCube images are supported",
            ));
        }
        let zstd = match be32(&disc, 0x04) {
            0 => false,
            5 => true,
            other => {
                return Err(invalid(format!(
                    "RVZ compression method {other} is not supported (only none and Zstandard)"
                )))
            }
        };
        let chunk_size = u64::from(be32(&disc, 0x0C));
        let small_ok = chunk_size >= SECTOR && chunk_size.is_power_of_two();
        let large_ok = chunk_size >= (2 << 20) && chunk_size % (2 << 20) == 0;
        if !(small_ok || large_ok) || chunk_size > (64 << 20) {
            return Err(invalid(format!("invalid RVZ chunk size {chunk_size:#x}")));
        }
        let mut disc_head = [0u8; DISC_HEAD_LEN];
        disc_head.copy_from_slice(&disc[0x10..0x10 + DISC_HEAD_LEN]);

        let num_raw = be32(&disc, 0xB4) as usize;
        let num_groups = be32(&disc, 0xC4) as usize;
        if num_raw == 0 || num_raw > 1 << 16 || num_groups > 1 << 24 {
            return Err(invalid("implausible RVZ table sizes"));
        }
        let raw_table = read_table(
            inner.as_mut(),
            "raw data",
            be64(&disc, 0xB8),
            be32(&disc, 0xC0),
            num_raw * RAW_DATA_LEN,
            zstd,
            file_len,
        )?;
        let group_table = read_table(
            inner.as_mut(),
            "group",
            be64(&disc, 0xC8),
            be32(&disc, 0xD0),
            num_groups * GROUP_LEN,
            zstd,
            file_len,
        )?;

        let mut raw = Vec::with_capacity(num_raw);
        for (i, e) in raw_table.as_chunks::<RAW_DATA_LEN>().0.iter().enumerate() {
            let offset = be64(e, 0);
            let start = offset / SECTOR * SECTOR;
            let end = offset
                .checked_add(be64(e, 8))
                .ok_or_else(|| invalid("RVZ raw data size overflows"))?;
            let (group_index, count) = (be32(e, 16), be32(e, 20));
            let needed = (end - start).div_ceil(chunk_size);
            if u64::from(count) < needed || u64::from(group_index) + needed > num_groups as u64 {
                return Err(invalid(format!(
                    "RVZ raw data {i} refers to missing groups"
                )));
            }
            raw.push(RawData {
                start,
                end,
                group_index,
                num_groups: count,
            });
        }
        let mut groups = Vec::with_capacity(num_groups);
        for (i, e) in group_table.as_chunks::<GROUP_LEN>().0.iter().enumerate() {
            let offset = u64::from(be32(e, 0)) * 4;
            let size_and_flag = be32(e, 4);
            let size = size_and_flag & !TOP_BIT;
            if size != 0 && offset + u64::from(size) > file_len {
                return Err(invalid(format!("RVZ group {i} lies outside the file")));
            }
            groups.push(Group {
                offset,
                size,
                compressed: size_and_flag & TOP_BIT != 0,
                packed: be32(e, 8) != 0,
            });
        }
        Ok(Self {
            inner,
            meta: Arc::new(Meta {
                iso_size,
                chunk_size,
                zstd,
                disc_head,
                raw,
                groups,
            }),
            cache: Vec::new(),
        })
    }

    /// Size of the original disc image in bytes.
    pub fn iso_size(&self) -> u64 {
        self.meta.iso_size
    }

    /// Decode the group that starts at disc offset `start` and holds `size` bytes.
    fn decode_group(&mut self, index: u32, start: u64, size: usize) -> io::Result<Arc<[u8]>> {
        if let Some(at) = self.cache.iter().position(|(i, _)| *i == index) {
            let hit = self.cache.remove(at);
            self.cache.insert(0, hit.clone());
            return Ok(hit.1);
        }
        let meta = self.meta.clone();
        let group = meta.groups[index as usize];
        let mut out = vec![0u8; size];
        // A stored size of zero means the whole group is zeros.
        if group.size != 0 {
            let stored = read_vec(self.inner.as_mut(), group.offset, group.size as usize)?;
            let data = match (group.compressed, meta.zstd) {
                (true, true) => zstd_decode(&stored, size)?,
                (true, false) => {
                    return Err(invalid(format!(
                        "RVZ group {index} is compressed in an uncompressed image"
                    )))
                }
                (false, _) => stored,
            };
            if group.packed {
                unpack(&data, &mut out, start)
                    .map_err(|e| invalid(format!("RVZ group {index}: {e}")))?;
            } else if data.len() == size {
                out.copy_from_slice(&data);
            } else {
                return Err(invalid(format!(
                    "RVZ group {index} has {} bytes, expected {size}",
                    data.len()
                )));
            }
        }
        // The first 0x80 bytes of the disc are stored in the header, not in the group data.
        if start == 0 {
            let n = DISC_HEAD_LEN.min(out.len());
            out[..n].copy_from_slice(&meta.disc_head[..n]);
        }
        let decoded: Arc<[u8]> = out.into();
        let keep = (CACHE_BYTES / meta.chunk_size as usize).max(2);
        self.cache.truncate(keep - 1);
        self.cache.insert(0, (index, decoded.clone()));
        Ok(decoded)
    }
}

impl DiscStream for RvzReader {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let meta = self.meta.clone();
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|end| *end <= meta.iso_size)
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        let mut pos = offset;
        while pos < end {
            let out = &mut buf[(pos - offset) as usize..];
            let Some(rd) = meta.raw.iter().find(|r| r.start <= pos && pos < r.end) else {
                // Not covered by any raw data entry: zeros up to the next entry (or the end of the read).
                let next = meta
                    .raw
                    .iter()
                    .map(|r| r.start)
                    .filter(|s| *s > pos)
                    .min()
                    .unwrap_or(end)
                    .min(end);
                out[..(next - pos) as usize].fill(0);
                pos = next;
                continue;
            };
            let rel = (pos - rd.start) / meta.chunk_size;
            debug_assert!(rel < u64::from(rd.num_groups));
            let group_start = rd.start + rel * meta.chunk_size;
            let group_size = (rd.end - group_start).min(meta.chunk_size) as usize;
            let data = self.decode_group(rd.group_index + rel as u32, group_start, group_size)?;
            let within = (pos - group_start) as usize;
            let n = (group_size - within).min((end - pos) as usize);
            out[..n].copy_from_slice(&data[within..within + n]);
            pos += n as u64;
        }
        Ok(())
    }

    fn stream_len(&mut self) -> io::Result<u64> {
        Ok(self.meta.iso_size)
    }
}

/// Expand RVZ-packed group data: a sequence of `u32 size` records, each followed either by `size` literal
/// bytes or, when the top bit is set, by a 68-byte seed from which `size` bytes of junk are regenerated.
/// `disc_offset` is where the group starts on the disc (the generator's phase depends on it).
fn unpack(mut data: &[u8], out: &mut [u8], disc_offset: u64) -> Result<(), String> {
    let mut done = 0usize;
    while data.len() >= 4 {
        let word = be32(data, 0);
        data = &data[4..];
        let size = (word & !TOP_BIT) as usize;
        if size > out.len() - done {
            return Err(format!("packed record of {size} bytes overruns the group"));
        }
        let target = &mut out[done..done + size];
        if word & TOP_BIT != 0 {
            let seed = data.get(..SEED_BYTES).ok_or("truncated junk seed")?;
            data = &data[SEED_BYTES..];
            let mut lfg = Lfg::from_seed(seed);
            lfg.skip(((disc_offset + done as u64) % SECTOR) as usize);
            lfg.fill(target);
        } else {
            let literal = data.get(..size).ok_or("truncated literal data")?;
            data = &data[size..];
            target.copy_from_slice(literal);
        }
        done += size;
    }
    if done != out.len() {
        return Err(format!("unpacked {done} bytes, expected {}", out.len()));
    }
    Ok(())
}

const LFG_K: usize = 521;
const LFG_J: usize = 32;
const LFG_BYTES: usize = LFG_K * 4;
const SEED_WORDS: usize = 17;
const SEED_BYTES: usize = SEED_WORDS * 4;

/// The lagged Fibonacci generator GameCube/Wii mastering used for padding ("junk") between files.
struct Lfg {
    state: [u32; LFG_K],
    /// Byte position within the current state block.
    position: usize,
}

impl Lfg {
    /// `seed`: 17 big-endian words as stored in RVZ.
    fn from_seed(seed: &[u8]) -> Self {
        let mut state = [0u32; LFG_K];
        for (word, bytes) in state.iter_mut().zip(seed.as_chunks::<4>().0) {
            *word = be32(bytes, 0);
        }
        for i in SEED_WORDS..LFG_K {
            state[i] = (state[i - 17] << 23) ^ (state[i - 16] >> 9) ^ state[i - 1];
        }
        // The console outputs bits 18.. instead of 16.. for the third byte; fold that in once here.
        for x in state.iter_mut() {
            *x = (*x & 0xFF00_FFFF) | ((*x >> 2) & 0x00FF_0000);
        }
        let mut lfg = Self { state, position: 0 };
        for _ in 0..4 {
            lfg.forward();
        }
        lfg
    }

    fn forward(&mut self) {
        for i in 0..LFG_J {
            self.state[i] ^= self.state[i + LFG_K - LFG_J];
        }
        for i in LFG_J..LFG_K {
            self.state[i] ^= self.state[i - LFG_J];
        }
    }

    fn skip(&mut self, n: usize) {
        self.position += n;
        while self.position >= LFG_BYTES {
            self.forward();
            self.position -= LFG_BYTES;
        }
    }

    fn fill(&mut self, out: &mut [u8]) {
        for byte in out {
            if self.position >= LFG_BYTES {
                self.forward();
                self.position -= LFG_BYTES;
            }
            // Words are emitted big-endian.
            let shift = 24 - 8 * (self.position % 4);
            *byte = (self.state[self.position / 4] >> shift) as u8;
            self.position += 1;
        }
    }
}

/// Compare this reader with nod on the RVZ file at `path`: the decoded disc is read through both in 1 MiB
/// blocks and every `stride`-th block is compared (1 = the whole disc). Returns the number of bytes compared.
/// Host only: this is the check that the browser's decoder yields exactly the disc nod yields.
#[cfg(not(target_arch = "wasm32"))]
pub fn verify_against_nod(path: &std::path::Path, stride: u64) -> io::Result<u64> {
    use std::io::Seek;
    const BLOCK: u64 = 1 << 20;
    let mut reference = nod::read::DiscReader::new(path, &nod::read::DiscOptions::default())
        .map_err(|e| io::Error::other(e.to_string()))?;
    let mut reader = RvzReader::open(Box::new(crate::disc::FileStream::open(path)?))?;
    let size = reader.iso_size();
    if reference.disc_size() != size {
        return Err(invalid(format!(
            "disc size differs: nod {} / built-in {size}",
            reference.disc_size()
        )));
    }
    let (mut a, mut b) = (vec![0u8; BLOCK as usize], vec![0u8; BLOCK as usize]);
    let mut compared = 0;
    for offset in (0..size)
        .step_by(BLOCK as usize)
        .step_by(stride.max(1) as usize)
    {
        let n = BLOCK.min(size - offset) as usize;
        reference.seek(io::SeekFrom::Start(offset))?;
        reference.read_exact(&mut a[..n])?;
        reader.read_exact_at(&mut b[..n], offset)?;
        if let Some(at) = a[..n].iter().zip(&b[..n]).position(|(x, y)| x != y) {
            return Err(invalid(format!(
                "built-in RVZ reader differs from nod at disc offset {:#x}",
                offset + at as u64
            )));
        }
        compared += n as u64;
    }
    Ok(compared)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assemble a small RVZ image in memory. `groups`: stored bytes, compressed flag, packed size.
    fn build_rvz(
        iso_size: u64,
        chunk_size: u32,
        zstd: bool,
        raw: &[(u64, u64, u32, u32)],
        groups: &[(Vec<u8>, bool, u32)],
    ) -> Vec<u8> {
        let enc = |d: &[u8]| {
            if zstd {
                ruzstd::encoding::compress_to_vec(d, ruzstd::encoding::CompressionLevel::Fastest)
            } else {
                d.to_vec()
            }
        };
        let mut file = vec![0u8; FILE_HEAD_LEN + DISC_LEN];
        let mut group_table = Vec::new();
        for (data, compressed, packed) in groups {
            while !file.len().is_multiple_of(4) {
                file.push(0);
            }
            let flag = if *compressed { TOP_BIT } else { 0 };
            group_table.extend(((file.len() / 4) as u32).to_be_bytes());
            group_table.extend((data.len() as u32 | flag).to_be_bytes());
            group_table.extend(packed.to_be_bytes());
            file.extend(data);
        }
        let mut raw_table = Vec::new();
        for (offset, size, group_index, count) in raw {
            raw_table.extend(offset.to_be_bytes());
            raw_table.extend(size.to_be_bytes());
            raw_table.extend(group_index.to_be_bytes());
            raw_table.extend(count.to_be_bytes());
        }
        let (raw_stored, group_stored) = (enc(&raw_table), enc(&group_table));
        let raw_at = file.len() as u64;
        file.extend(&raw_stored);
        let group_at = file.len() as u64;
        file.extend(&group_stored);

        file[..4].copy_from_slice(b"RVZ\x01");
        file[0x04..0x08].copy_from_slice(&RVZ_VERSION.to_be_bytes());
        file[0x08..0x0C].copy_from_slice(&0x0003_0000u32.to_be_bytes());
        file[0x0C..0x10].copy_from_slice(&(DISC_LEN as u32).to_be_bytes());
        file[0x24..0x2C].copy_from_slice(&iso_size.to_be_bytes());
        let d = FILE_HEAD_LEN;
        file[d..d + 4].copy_from_slice(&1u32.to_be_bytes());
        file[d + 4..d + 8].copy_from_slice(&(if zstd { 5u32 } else { 0 }).to_be_bytes());
        file[d + 0x0C..d + 0x10].copy_from_slice(&chunk_size.to_be_bytes());
        for (i, b) in file[d + 0x10..d + 0x90].iter_mut().enumerate() {
            *b = 0xA0 ^ i as u8;
        }
        file[d + 0xB4..d + 0xB8].copy_from_slice(&(raw.len() as u32).to_be_bytes());
        file[d + 0xB8..d + 0xC0].copy_from_slice(&raw_at.to_be_bytes());
        file[d + 0xC0..d + 0xC4].copy_from_slice(&(raw_stored.len() as u32).to_be_bytes());
        file[d + 0xC4..d + 0xC8].copy_from_slice(&(groups.len() as u32).to_be_bytes());
        file[d + 0xC8..d + 0xD0].copy_from_slice(&group_at.to_be_bytes());
        file[d + 0xD0..d + 0xD4].copy_from_slice(&(group_stored.len() as u32).to_be_bytes());
        file
    }

    fn pattern(len: usize, salt: u8) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(7) ^ salt).collect()
    }

    fn read_all(file: Vec<u8>) -> io::Result<Vec<u8>> {
        let mut r = RvzReader::open(Box::new(Arc::<[u8]>::from(file)))?;
        let mut out = vec![0xEEu8; r.iso_size() as usize];
        r.read_exact_at(&mut out, 0)?;
        Ok(out)
    }

    #[test]
    fn reads_plain_zero_and_compressed_groups() {
        let chunk = 0x8000usize;
        let (a, c) = (pattern(chunk, 1), pattern(0x1234, 3));
        for zstd in [false, true] {
            let enc = |d: &[u8]| {
                ruzstd::encoding::compress_to_vec(d, ruzstd::encoding::CompressionLevel::Fastest)
            };
            let groups = vec![
                if zstd {
                    (enc(&a), true, 0)
                } else {
                    (a.clone(), false, 0)
                },
                // All zeros: nothing stored.
                (Vec::new(), false, 0),
                // A group may be stored raw even in a compressed image; the last one is short.
                (c.clone(), false, 0),
            ];
            let iso_size = (2 * chunk + c.len()) as u64 + 0x100;
            let raw = [(0x80, (2 * chunk + c.len()) as u64 - 0x80, 0, 3)];
            let iso = read_all(build_rvz(iso_size, chunk as u32, zstd, &raw, &groups)).unwrap();
            // The first 0x80 bytes come from the header copy, the rest of group 0 from its data.
            let head: Vec<u8> = (0..0x80).map(|i| 0xA0 ^ i as u8).collect();
            assert_eq!(&iso[..0x80], &head[..]);
            assert_eq!(&iso[0x80..chunk], &a[0x80..]);
            assert!(iso[chunk..2 * chunk].iter().all(|b| *b == 0));
            assert_eq!(&iso[2 * chunk..2 * chunk + c.len()], &c[..]);
            // Past the last raw data entry the image reads as zeros.
            assert!(iso[2 * chunk + c.len()..].iter().all(|b| *b == 0));
        }
    }

    #[test]
    fn reads_across_groups_and_rejects_reads_past_the_end() {
        let chunk = 0x8000usize;
        let (a, b) = (pattern(chunk, 9), pattern(chunk, 20));
        let raw = [(0x80, 2 * chunk as u64 - 0x80, 0, 2)];
        let groups = vec![(a.clone(), false, 0), (b.clone(), false, 0)];
        let file = build_rvz(2 * chunk as u64, chunk as u32, false, &raw, &groups);
        let mut r = RvzReader::open(Box::new(Arc::<[u8]>::from(file))).unwrap();
        let mut buf = [0u8; 64];
        r.read_exact_at(&mut buf, chunk as u64 - 32).unwrap();
        assert_eq!(&buf[..32], &a[chunk - 32..]);
        assert_eq!(&buf[32..], &b[..32]);
        // A clone reads independently.
        let mut r2 = r.clone();
        r2.read_exact_at(&mut buf, chunk as u64).unwrap();
        assert_eq!(&buf[..], &b[..64]);
        assert!(r.read_exact_at(&mut buf, 2 * chunk as u64 - 10).is_err());
        assert_eq!(r.stream_len().unwrap(), 2 * chunk as u64);
    }

    /// Junk as the console's mastering tool produced it, written out independently of `Lfg`: the seed
    /// expansion, the "shift by 18" output quirk and the lagged XOR are spelled out per byte.
    fn reference_junk(seed: &[u32; 17], skip: usize, len: usize) -> Vec<u8> {
        let mut s = vec![0u32; LFG_K];
        s[..17].copy_from_slice(seed);
        for i in 17..LFG_K {
            s[i] = (s[i - 17] << 23) ^ (s[i - 16] >> 9) ^ s[i - 1];
        }
        let step = |s: &mut Vec<u32>| {
            for i in 0..LFG_K {
                let j = if i < 32 { i + LFG_K - 32 } else { i - 32 };
                s[i] ^= s[j];
            }
        };
        // Four warm-up rounds; applying the output transform before or after them is equivalent only if it
        // is linear over XOR, which it is (bit selection), so do it after to differ from `Lfg::from_seed`.
        for _ in 0..4 {
            step(&mut s);
        }
        let mut out = Vec::new();
        let mut produced = 0usize;
        while out.len() < len {
            for x in s.iter() {
                let bytes = [(x >> 24) as u8, (x >> 18) as u8, (x >> 8) as u8, *x as u8];
                for b in bytes {
                    if produced >= skip && out.len() < len {
                        out.push(b);
                    }
                    produced += 1;
                }
            }
            step(&mut s);
        }
        out
    }

    #[test]
    fn regenerates_packed_junk() {
        let chunk = 0x10000usize;
        let seed: [u32; 17] = std::array::from_fn(|i| 0x9E37_79B9u32.wrapping_mul(i as u32 + 1));
        let seed_bytes: Vec<u8> = seed.iter().flat_map(|w| w.to_be_bytes()).collect();
        let literal = pattern(0x300, 5);
        // Group 1 (disc offset 0x10000): 0x300 literal bytes, then junk for the rest of the group. The junk
        // starts 0x300 bytes into a sector, so the generator must skip that much.
        let junk_len = chunk - literal.len();
        let mut packed = Vec::new();
        packed.extend((literal.len() as u32).to_be_bytes());
        packed.extend(&literal);
        packed.extend((junk_len as u32 | TOP_BIT).to_be_bytes());
        packed.extend(&seed_bytes);
        let packed_len = packed.len() as u32;
        for zstd in [false, true] {
            let stored = if zstd {
                ruzstd::encoding::compress_to_vec(
                    &packed[..],
                    ruzstd::encoding::CompressionLevel::Fastest,
                )
            } else {
                packed.clone()
            };
            let groups = vec![(pattern(chunk, 2), false, 0), (stored, zstd, packed_len)];
            let raw = [(0x80, 2 * chunk as u64 - 0x80, 0, 2)];
            let iso = read_all(build_rvz(
                2 * chunk as u64,
                chunk as u32,
                zstd,
                &raw,
                &groups,
            ))
            .unwrap();
            assert_eq!(&iso[chunk..chunk + 0x300], &literal[..]);
            let expected = reference_junk(&seed, 0x300, junk_len);
            assert!(
                iso[chunk + 0x300..] == expected[..],
                "junk differs (zstd {zstd})"
            );
        }
    }

    #[test]
    fn rejects_what_it_cannot_read() {
        let chunk = 0x8000u32;
        let raw = [(0x80, 0x7F80, 0, 1)];
        let groups = vec![(pattern(0x8000, 1), false, 0)];
        let good = build_rvz(0x8000, chunk, false, &raw, &groups);
        assert!(read_all(good.clone()).is_ok());

        let err = |mutate: &dyn Fn(&mut Vec<u8>)| {
            let mut f = good.clone();
            mutate(&mut f);
            read_all(f).unwrap_err().to_string()
        };
        let d = FILE_HEAD_LEN;
        assert!(err(&|f| f[0] = b'W').contains("not an RVZ"));
        assert!(err(&|f| f[d + 3] = 2).contains("Wii"));
        assert!(err(&|f| f[d + 7] = 4).contains("method 4"));
        assert!(err(&|f| f[d + 0x0F] = 1).contains("chunk size"));
        assert!(err(&|f| f[0x08] = 2).contains("newer"));
        // Group table pointing past the end of the file.
        assert!(err(&|f| f[d + 0xC8] = 0x7F).contains("outside the file"));
        // A group whose stored size does not match the chunk.
        let short = build_rvz(0x8000, chunk, false, &raw, &[(pattern(100, 1), false, 0)]);
        assert!(read_all(short)
            .unwrap_err()
            .to_string()
            .contains("expected 32768"));
        // More data than groups.
        let missing = build_rvz(0x10000, chunk, false, &[(0x80, 0xFF80, 0, 2)], &groups);
        assert!(read_all(missing)
            .unwrap_err()
            .to_string()
            .contains("missing groups"));
        assert!(read_all(vec![0u8; 16]).is_err());
    }
}
