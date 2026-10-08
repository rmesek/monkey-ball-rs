//! Importing a disc image: copy the files the game uses ([`crate::content::import_list`]) out of the user's
//! image into one game pack (`mb_assets::gamepack`), a piece at a time.
//!
//! The importer owns no storage and no thread: the caller (the browser's import worker, a test) asks for
//! the next piece with [`Importer::step`], writes it where the pack lives and reports progress in between.
//! Nothing larger than one piece is ever held, whatever the size of the image or of a file in it.

use std::io;

use mb_assets::disc::{Disc, DiscStream, FileEntry};
use mb_assets::gamepack::{Finished, PackWriter};

use crate::content::import_list;

/// Most bytes copied per [`Importer::step`].
pub const PIECE: usize = 1 << 20;

pub struct Importer<S: DiscStream> {
    disc: Disc<S>,
    region: &'static str,
    files: Vec<(String, FileEntry)>,
    writer: PackWriter,
    /// Index of the file being copied and how much of it is done.
    current: usize,
    within: u32,
    started: bool,
    done: u64,
    total: u64,
    buf: Vec<u8>,
}

impl<S: DiscStream> Importer<S> {
    /// Open the image behind `stream` and work out what to copy. Errors carry a message for the user:
    /// what the file is when it is not a supported image of the game, or which file the image lacks.
    pub fn open(stream: S) -> io::Result<Self> {
        let disc = Disc::open(stream)?;
        let region = disc.require_smb1()?;
        let list = import_list(&|p| disc.entry(p).is_some())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let files: Vec<(String, FileEntry)> = list
            .into_iter()
            .filter_map(|p| disc.entry(&p).map(|e| (p, e)))
            .collect();
        let total = files.iter().map(|(_, e)| u64::from(e.len)).sum();
        Ok(Self {
            writer: PackWriter::new(disc.game_id()),
            disc,
            region,
            files,
            current: 0,
            within: 0,
            started: false,
            done: 0,
            total,
            buf: Vec::new(),
        })
    }

    pub fn game_id(&self) -> &str {
        self.disc.game_id()
    }

    pub fn region(&self) -> &'static str {
        self.region
    }

    /// The disc paths that are copied (what the game can read afterwards).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|(p, _)| p.as_str())
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Bytes of file data to copy, and how many are copied so far.
    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    pub fn done_bytes(&self) -> u64 {
        self.done
    }

    /// Copy the next piece. `Some((offset, bytes))`: write `bytes` at `offset` of the pack. `None`: every
    /// file is copied; call [`Importer::finish`].
    pub fn step(&mut self) -> io::Result<Option<(u64, &[u8])>> {
        loop {
            let Some((name, entry)) = self.files.get(self.current) else {
                return Ok(None);
            };
            if !self.started {
                self.writer.begin(name, entry.len)?;
                self.started = true;
            }
            if self.within == entry.len {
                self.writer.end()?;
                self.current += 1;
                self.within = 0;
                self.started = false;
                continue;
            }
            let n = PIECE.min((entry.len - self.within) as usize);
            self.buf.resize(n, 0);
            self.disc
                .read_range(*entry, self.within, &mut self.buf)
                .map_err(|e| io::Error::new(e.kind(), format!("{name}: {e}")))?;
            let at = self.writer.data(&self.buf)?;
            self.within += n as u32;
            self.done += n as u64;
            return Ok(Some((at, &self.buf)));
        }
    }

    /// The index and header that complete the pack (write the index, then the header).
    pub fn finish(self) -> io::Result<Finished> {
        if self.current < self.files.len() {
            return Err(io::Error::other("the import is not complete"));
        }
        self.writer.finish()
    }

    /// Read one file straight from the image (used where the pack cannot be stored and the image itself
    /// serves the session).
    pub fn read_file(&mut self, path: &str) -> io::Result<Vec<u8>> {
        self.disc.read_file(path)
    }
}

/// Import a whole image into memory (tests and tools).
pub fn import_to_vec<S: DiscStream>(stream: S) -> io::Result<Vec<u8>> {
    let mut importer = Importer::open(stream)?;
    let mut out = vec![0u8; mb_assets::gamepack::HEADER_LEN];
    while let Some((at, bytes)) = importer.step()? {
        let at = at as usize;
        if out.len() < at + bytes.len() {
            out.resize(at + bytes.len(), 0);
        }
        out[at..at + bytes.len()].copy_from_slice(bytes);
    }
    let done = importer.finish()?;
    out.truncate(done.index_offset as usize);
    out.extend(&done.index);
    out[..done.header.len()].copy_from_slice(&done.header);
    Ok(out)
}
