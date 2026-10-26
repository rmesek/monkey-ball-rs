//! Where game files come from, and which files the game needs.
//!
//! Two kinds of source sit behind [`Content`]:
//! * **direct** (desktop, tests): a game pack in a file, or a disc image read in place; every read is
//!   synchronous.
//! * **deferred** (browser): files live in on-device storage that can only be read asynchronously. A state
//!   that needs files first *prepares* them ([`Content::prepare`], polled once per frame while a loading
//!   screen is shown); the synchronous loaders then read from memory, and the files are released again
//!   ([`Content::release`]) once they are parsed and on the GPU.
//!
//! The lists of files are the functions at the bottom ([`boot_files`], [`StageFiles`], [`all_files`]). They
//! are the only place paths are spelled: the loaders read through [`StageFiles`], the prepare step asks for
//! the same list, and the importer copies [`all_files`]. A read of a file that the source has but that was
//! not prepared is a loud error (and is recorded, see [`Content::misses`]), never a silent "not found".

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Mutex, PoisonError};

use mb_assets::disc::{DiscStream, MAIN_DOL_PATH};
use mb_assets::pack::{PackProvider, disc_path};
use mb_game::Difficulty;
use mb_game::course::stage_list;

use crate::stage_info::{BgInfo, bg_by_name, bg_for_stage_or_nearest};

/// State of an asynchronous fetch.
pub enum Fetch {
    /// Nothing was requested (or the result was already handed over).
    Idle,
    Pending {
        done: u64,
        total: u64,
    },
    /// The requested files, handed over once.
    Ready(Vec<(String, Vec<u8>)>),
    Failed(String),
}

/// Storage that can only be read asynchronously (the browser's imported game data).
pub trait DeferredSource {
    /// Whether the game data has `path` (a disc path); answered from its index, without I/O.
    fn contains(&self, path: &str) -> bool;
    /// Start fetching `paths` (all of which `contains`). Replaces a request that is still running.
    fn request(&self, paths: &[String]);
    /// Progress of the current request.
    fn poll(&self) -> Fetch;
}

/// What [`Content::prepare`] reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Prepare {
    /// Every requested file the source has is in memory.
    Ready,
    /// Bytes fetched so far and in total.
    Pending {
        done: u64,
        total: u64,
    },
    Failed(String),
}

struct Deferred {
    source: Rc<dyn DeferredSource>,
    /// Prepared files by disc path.
    ready: RefCell<HashMap<String, Vec<u8>>>,
    /// The list being fetched.
    requested: RefCell<Option<Vec<String>>>,
    misses: RefCell<Vec<String>>,
}

#[derive(Clone)]
enum Backing {
    Direct(Rc<dyn PackProvider>),
    Deferred(Rc<Deferred>),
}

/// Opened content: raw file access plus the `.lz` conveniences every loader needs.
#[derive(Clone)]
pub struct Content {
    backing: Backing,
}

/// Open a disc image (RVZ none/Zstandard, ISO/GCM) of Super Monkey Ball for direct reading.
pub fn open_disc_image(path: impl AsRef<Path>) -> io::Result<Content> {
    open_disc_stream(Box::new(mb_assets::disc::FileStream::open(path)?))
}

/// [`open_disc_image`] for an image that is not reached by a path: a file the platform opened for the app
/// (`FileStream::from(file)`), or any other source of its bytes.
pub fn open_disc_stream(stream: Box<dyn DiscStream + Send>) -> io::Result<Content> {
    let disc = mb_assets::disc::DiscPack::from_stream(stream)?;
    if disc.smb1_region().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "This is a GameCube disc image, but not Super Monkey Ball (disc id {}). {}",
                disc.game_id(),
                mb_assets::disc::SUPPORTED
            ),
        ));
    }
    Ok(Content::new(Rc::new(disc)))
}

/// Open an imported game pack kept in a file ([`crate::import::write_pack_file`]) for direct reading.
/// Returns the content, the pack's disc id and its size in bytes. An error means the file is not a
/// complete pack this build can read: remove it and import again.
#[cfg(not(target_arch = "wasm32"))]
pub fn open_pack_file(path: impl AsRef<Path>) -> io::Result<(Content, String, u64)> {
    let pack = mb_assets::gamepack::PackFile::open(path)?;
    let (id, len) = (pack.game_id().to_string(), pack.len());
    Ok((Content::new(Rc::new(pack)), id, len))
}

/// Game data that arrives while the game runs: empty until [`LatePack::fill`]. A native platform without
/// a pack yet starts the driver on `Content::new(late.clone())`; the import screen fills it once the
/// pack is complete.
#[derive(Default)]
pub struct LatePack(Mutex<Option<Box<dyn PackProvider>>>);

impl LatePack {
    pub fn fill(&self, pack: Box<dyn PackProvider>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(pack);
    }
}

impl PackProvider for LatePack {
    fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
        match &*self.0.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(pack) => pack.fetch(path),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no game data yet: {path}"),
            )),
        }
    }

    fn contains(&self, path: &str) -> bool {
        let pack = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        pack.as_ref().is_some_and(|p| p.contains(path))
    }
}

impl Content {
    /// Content read synchronously from `pack`.
    pub fn new(pack: Rc<dyn PackProvider>) -> Self {
        Self {
            backing: Backing::Direct(pack),
        }
    }

    /// Content whose files must be prepared before they can be read.
    pub fn deferred(source: Rc<dyn DeferredSource>) -> Self {
        Self {
            backing: Backing::Deferred(Rc::new(Deferred {
                source,
                ready: RefCell::new(HashMap::new()),
                requested: RefCell::new(None),
                misses: RefCell::new(Vec::new()),
            })),
        }
    }

    /// True when files have to be prepared (and a loading screen is worth showing).
    pub fn is_deferred(&self) -> bool {
        matches!(self.backing, Backing::Deferred(_))
    }

    /// Whether the source has `path` at all.
    pub fn contains(&self, path: &str) -> bool {
        match &self.backing {
            Backing::Direct(p) => p.contains(path),
            Backing::Deferred(d) => d.source.contains(&disc_path(path)),
        }
    }

    /// Hand over files that are already in memory (the browser shell fetches the start-up files before the
    /// game exists).
    pub fn insert_prepared(&self, files: Vec<(String, Vec<u8>)>) {
        if let Backing::Deferred(d) = &self.backing {
            let mut ready = d.ready.borrow_mut();
            for (path, bytes) in files {
                ready.insert(disc_path(&path), bytes);
            }
        }
    }

    /// Make `wanted` readable. Call it every frame with the same list until it stops returning
    /// [`Prepare::Pending`]; files the source does not have are skipped (the loader decides whether that
    /// matters). Direct content is always ready.
    pub fn prepare(&self, wanted: &[Wanted]) -> Prepare {
        let Backing::Deferred(d) = &self.backing else {
            return Prepare::Ready;
        };
        let missing: Vec<String> = {
            let ready = d.ready.borrow();
            wanted
                .iter()
                .filter_map(|w| w.resolve(&|p| d.source.contains(p)))
                .filter(|p| !ready.contains_key(p))
                .collect()
        };
        if missing.is_empty() {
            *d.requested.borrow_mut() = None;
            return Prepare::Ready;
        }
        let mut requested = d.requested.borrow_mut();
        if requested.as_ref() != Some(&missing) {
            d.source.request(&missing);
            *requested = Some(missing);
            return Prepare::Pending { done: 0, total: 0 };
        }
        match d.source.poll() {
            Fetch::Idle => {
                // The source lost the request (it was replaced); ask again next frame.
                *requested = None;
                Prepare::Pending { done: 0, total: 0 }
            }
            Fetch::Pending { done, total } => Prepare::Pending { done, total },
            Fetch::Failed(e) => {
                *requested = None;
                Prepare::Failed(e)
            }
            Fetch::Ready(files) => {
                *requested = None;
                let mut ready = d.ready.borrow_mut();
                for (path, bytes) in files {
                    ready.insert(disc_path(&path), bytes);
                }
                if missing.iter().all(|p| ready.contains_key(p)) {
                    Prepare::Ready
                } else {
                    Prepare::Failed("the game data did not return every requested file".into())
                }
            }
        }
    }

    /// Drop prepared files from memory (after they were parsed). No effect on direct content.
    pub fn release(&self, wanted: &[Wanted]) {
        if let Backing::Deferred(d) = &self.backing {
            let mut ready = d.ready.borrow_mut();
            for w in wanted {
                for p in w.candidates() {
                    ready.remove(&p);
                }
            }
        }
    }

    /// Drop every prepared file.
    pub fn release_all(&self) {
        if let Backing::Deferred(d) = &self.backing {
            d.ready.borrow_mut().clear();
        }
    }

    /// Bytes of prepared files currently held in memory (0 for direct content).
    pub fn prepared_bytes(&self) -> usize {
        match &self.backing {
            Backing::Direct(_) => 0,
            Backing::Deferred(d) => d.ready.borrow().values().map(Vec::len).sum(),
        }
    }

    /// Files that were read without having been prepared although the source has them: each one is a bug
    /// in the file lists below. Tests assert this stays empty.
    pub fn misses(&self) -> Vec<String> {
        match &self.backing {
            Backing::Direct(_) => Vec::new(),
            Backing::Deferred(d) => d.misses.borrow().clone(),
        }
    }

    /// The game executable, when the source has it.
    pub fn main_dol(&self) -> Option<Vec<u8>> {
        self.read(MAIN_DOL_PATH).ok()
    }

    /// Install the game's own S16 sine / arctangent tables into `mb-engine` (once per process; later calls
    /// are no-ops). Returns false when the executable is unavailable or has no tables: the engine then
    /// keeps its computed approximation, which plays the same but is not bit-identical to the original.
    pub fn install_trig_tables(&self) -> bool {
        use std::sync::OnceLock;
        static DONE: OnceLock<bool> = OnceLock::new();
        *DONE.get_or_init(|| {
            self.main_dol()
                .and_then(|dol| mb_engine::Tables::from_dol(&dol))
                .is_some_and(|t| mb_engine::install_tables(t).is_ok())
        })
    }

    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        match &self.backing {
            Backing::Direct(p) => p.fetch(path),
            Backing::Deferred(d) => {
                let key = disc_path(path);
                if let Some(bytes) = d.ready.borrow().get(&key) {
                    return Ok(bytes.clone());
                }
                if d.source.contains(&key) {
                    d.misses.borrow_mut().push(key.clone());
                    return Err(io::Error::other(format!(
                        "{key} was not prepared before it was read"
                    )));
                }
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("not in the game data: {key}"),
                ))
            }
        }
    }

    /// [`Content::read`] for a file that is read exactly once (a music stream): prepared content hands over
    /// its copy instead of cloning it, so the file is in memory once, not twice.
    pub fn read_once(&self, path: &str) -> io::Result<Vec<u8>> {
        if let Backing::Deferred(d) = &self.backing
            && let Some(bytes) = d.ready.borrow_mut().remove(&disc_path(path))
        {
            return Ok(bytes);
        }
        self.read(path)
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

// ---------------------------------------------------------------------------------------------------------
// Which files the game reads.

/// One file a loader reads, as a disc path.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Wanted {
    pub path: String,
    /// The loader reads `path`, or `path.lz` when the disc has no plain file
    /// ([`Content::read_maybe_lz`]).
    pub maybe_lz: bool,
}

impl Wanted {
    pub fn file(path: impl AsRef<str>) -> Self {
        Self {
            path: disc_path(path.as_ref()),
            maybe_lz: false,
        }
    }

    pub fn maybe_lz(path: impl AsRef<str>) -> Self {
        Self {
            path: disc_path(path.as_ref()),
            maybe_lz: true,
        }
    }

    /// Every path the loader may ask for.
    pub fn candidates(&self) -> Vec<String> {
        let mut v = vec![self.path.clone()];
        if self.maybe_lz {
            v.push(format!("{}.lz", self.path));
        }
        v
    }

    /// The path the loader will end up reading, given what exists.
    pub fn resolve(&self, exists: &dyn Fn(&str) -> bool) -> Option<String> {
        self.candidates().into_iter().find(|p| exists(p))
    }
}

const BASE: &str = "test";

/// Files the title menu needs before its first frame: HUD and font sheets, the sound effect banks and the
/// executable (trig tables, sound table).
pub fn boot_files() -> Vec<Wanted> {
    let mut v: Vec<Wanted> = crate::text::UI_SHEETS
        .iter()
        .map(|name| Wanted::maybe_lz(ui_sheet_path(name)))
        .collect();
    v.extend(crate::audio::sfx_paths().iter().map(Wanted::file));
    v.push(Wanted::file(MAIN_DOL_PATH));
    v
}

/// Disc path of a UI texture sheet (`bmp_nml`, `bmp_com`).
pub fn ui_sheet_path(name: &str) -> String {
    format!("{BASE}/bmp/{name}.tpl")
}

/// The background a stage is drawn with: the one the course table names, else the nearest known one.
pub fn stage_bg(stage_id: u32, bg_name: &str) -> BgInfo {
    bg_by_name(bg_name)
        .copied()
        .unwrap_or(*bg_for_stage_or_nearest(stage_id))
}

/// Every file one stage reads, by role. The stage loader reads through this and the prepare step fetches
/// [`StageFiles::wanted`], so the two cannot drift apart.
#[derive(Clone, Debug, PartialEq)]
pub struct StageFiles {
    pub stage_id: u32,
    pub stagedef: String,
    pub gma: String,
    pub tpl: String,
    /// NaomiLib geometry and textures (only stages in `NAOMI_STAGE_IDS` read them).
    pub naomi_obj: String,
    pub naomi_tpl: String,
    pub naomi: bool,
    pub bg: BgInfo,
    pub bg_gma: String,
    pub bg_tpl: String,
    /// Music streams, intro first (empty when the background has no music).
    pub music: Vec<String>,
}

/// Files shared by every stage (ball, bananas, goal bag; goal tape and clock digits).
pub const COMMON_GMA: &str = "test/init/common.gma";
pub const COMMON_TPL: &str = "test/init/common.tpl";
pub const COMMON_NL_OBJ: &str = "test/init/common_p.lz";
pub const COMMON_NL_TPL: &str = "test/init/common.lz";

/// What every stage load reads besides its own files.
pub fn common_files() -> Vec<Wanted> {
    vec![
        Wanted::maybe_lz(COMMON_GMA),
        Wanted::maybe_lz(COMMON_TPL),
        Wanted::file(COMMON_NL_OBJ),
        Wanted::file(COMMON_NL_TPL),
    ]
}

impl StageFiles {
    /// `music_bg` is the background name the course table gives the stage (it selects the music; empty
    /// for none); `bg` is what is drawn.
    pub fn new(stage_id: u32, bg: BgInfo, music_bg: &str) -> Self {
        let id = format!("{stage_id:03}");
        let dir = format!("{BASE}/st{id}");
        Self {
            stage_id,
            stagedef: format!("{dir}/STAGE{id}.lz"),
            gma: format!("{dir}/st{id}.gma"),
            tpl: format!("{dir}/st{id}.tpl"),
            naomi_obj: format!("{dir}/st{id}_p.lz"),
            naomi_tpl: format!("{dir}/st{id}.lz"),
            naomi: crate::stage_scene::is_naomi_stage(stage_id),
            bg,
            bg_gma: format!("{BASE}/bg/{}.gma", bg.file_name),
            bg_tpl: format!("{BASE}/bg/{}.tpl", bg.file_name),
            music: mb_audio::music::stage_music(music_bg)
                .map(|m| m.paths())
                .unwrap_or_default(),
        }
    }

    /// The files of a course stage, as the course flow plays it.
    pub fn for_stage(stage_id: u32) -> Self {
        let name = mb_game::stages::bg_name(stage_id).unwrap_or_default();
        Self::new(stage_id, stage_bg(stage_id, name), name)
    }

    /// The stage's own files (not the background, the shared files or the music).
    pub fn stage_wanted(&self) -> Vec<Wanted> {
        let mut v = vec![
            Wanted::file(&self.stagedef),
            Wanted::maybe_lz(&self.gma),
            Wanted::maybe_lz(&self.tpl),
        ];
        if self.naomi {
            v.push(Wanted::file(&self.naomi_obj));
            v.push(Wanted::file(&self.naomi_tpl));
        }
        v
    }

    pub fn bg_wanted(&self) -> Vec<Wanted> {
        vec![
            Wanted::maybe_lz(&self.bg_gma),
            Wanted::maybe_lz(&self.bg_tpl),
        ]
    }

    pub fn music_wanted(&self) -> Vec<Wanted> {
        self.music.iter().map(Wanted::file).collect()
    }

    /// Everything a load of this stage may read.
    pub fn wanted(&self) -> Vec<Wanted> {
        let mut v = self.stage_wanted();
        v.extend(self.bg_wanted());
        v.extend(common_files());
        v.extend(self.music_wanted());
        v
    }
}

/// Every stage that can be reached from the menus, in course order (each id once).
pub fn course_stage_ids() -> Vec<u32> {
    let mut ids = Vec::new();
    for d in Difficulty::ALL {
        for entry in stage_list(d) {
            if !ids.contains(&entry.id) {
                ids.push(entry.id);
            }
        }
    }
    ids
}

/// Everything the game can read: what the importer copies out of the disc image. Sorted, each file once.
pub fn all_files() -> Vec<Wanted> {
    let mut v = boot_files();
    v.extend(common_files());
    for id in course_stage_ids() {
        v.extend(StageFiles::for_stage(id).wanted());
    }
    v.sort();
    v.dedup();
    v
}

/// [`all_files`] resolved against a disc: the paths to copy. `Err` names a file the game cannot do without.
pub fn import_list(exists: &dyn Fn(&str) -> bool) -> Result<Vec<String>, String> {
    let need = |w: &Wanted| -> Result<String, String> {
        w.resolve(exists)
            .ok_or_else(|| format!("this disc image has no {}", w.path))
    };
    for w in boot_files() {
        need(&w)?;
    }
    let mut out: Vec<String> = all_files()
        .iter()
        .filter_map(|w| w.resolve(exists))
        .collect();
    // A stage needs its stagedef and some geometry; anything else missing only costs decoration.
    for id in course_stage_ids() {
        let files = StageFiles::for_stage(id);
        need(&Wanted::file(&files.stagedef))?;
        let has_geometry = Wanted::maybe_lz(&files.gma).resolve(exists).is_some()
            || (files.naomi && exists(&files.naomi_obj));
        if !has_geometry {
            return Err(format!("this disc image has no models for stage {id}"));
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// A deferred source over a synchronous pack, for tests (behind the `dev` feature): it behaves like the
/// browser's storage (files must be prepared; a fetch takes `delay` polls) without needing a browser.
#[cfg(any(test, feature = "dev"))]
pub struct DeferredPack {
    pack: Rc<dyn PackProvider>,
    delay: u32,
    state: RefCell<Option<(Vec<String>, u32)>>,
    /// How many fetches were started.
    pub requests: std::cell::Cell<u32>,
}

#[cfg(any(test, feature = "dev"))]
impl DeferredPack {
    pub fn new(pack: Rc<dyn PackProvider>, delay: u32) -> Self {
        Self {
            pack,
            delay,
            state: RefCell::new(None),
            requests: std::cell::Cell::new(0),
        }
    }
}

#[cfg(any(test, feature = "dev"))]
impl DeferredSource for DeferredPack {
    fn contains(&self, path: &str) -> bool {
        self.pack.contains(path)
    }

    fn request(&self, paths: &[String]) {
        self.requests.set(self.requests.get() + 1);
        *self.state.borrow_mut() = Some((paths.to_vec(), self.delay));
    }

    fn poll(&self) -> Fetch {
        let mut state = self.state.borrow_mut();
        match state.as_mut() {
            None => Fetch::Idle,
            Some((paths, left)) if *left > 0 => {
                *left -= 1;
                let total = paths.len() as u64;
                Fetch::Pending {
                    done: total.saturating_sub(u64::from(*left)).min(total),
                    total,
                }
            }
            Some(_) => {
                let (paths, _) = state.take().unwrap();
                let mut files = Vec::new();
                for p in paths {
                    match self.pack.fetch(&p) {
                        Ok(bytes) => files.push((p, bytes)),
                        Err(e) => return Fetch::Failed(e.to_string()),
                    }
                }
                Fetch::Ready(files)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pack of three small files.
    struct Files(HashMap<String, Vec<u8>>);

    impl PackProvider for Files {
        fn fetch(&self, path: &str) -> io::Result<Vec<u8>> {
            let found = self.0.get(&disc_path(path)).cloned();
            found.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.to_string()))
        }

        fn contains(&self, path: &str) -> bool {
            self.0.contains_key(&disc_path(path))
        }
    }

    fn pack() -> Rc<dyn PackProvider> {
        let files = [
            ("test/a.gma", vec![1]),
            ("test/b.tpl.lz", vec![2, 2]),
            ("test/c.lz", vec![3, 3, 3]),
        ];
        Rc::new(Files(
            files.into_iter().map(|(p, b)| (p.to_string(), b)).collect(),
        ))
    }

    #[test]
    fn wanted_resolves_the_plain_file_first() {
        let exists = |p: &str| p == "test/a.gma" || p == "test/a.gma.lz" || p == "test/b.tpl.lz";
        assert_eq!(
            Wanted::maybe_lz("./test/a.gma").resolve(&exists),
            Some("test/a.gma".into())
        );
        assert_eq!(
            Wanted::maybe_lz("test/b.tpl").resolve(&exists),
            Some("test/b.tpl.lz".into())
        );
        assert_eq!(Wanted::file("test/b.tpl").resolve(&exists), None);
    }

    #[test]
    fn deferred_content_needs_prepare_and_reports_unprepared_reads() {
        let source = Rc::new(DeferredPack::new(pack(), 2));
        let content = Content::deferred(source.clone());
        assert!(content.is_deferred() && content.contains("/test/a.gma"));
        // Reading before preparing: an error that is not "not found", and it is recorded.
        let e = content.read("test/a.gma").unwrap_err();
        assert_ne!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(content.misses(), ["test/a.gma"]);
        // A file the source does not have is plain "not found".
        assert_eq!(
            content.read("test/zzz").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );

        let wanted = [
            Wanted::maybe_lz("test/a.gma"),
            Wanted::maybe_lz("test/b.tpl"),
            Wanted::file("test/missing.bin"),
        ];
        let mut polls = 0;
        loop {
            match content.prepare(&wanted) {
                Prepare::Ready => break,
                Prepare::Pending { .. } => polls += 1,
                Prepare::Failed(e) => panic!("{e}"),
            }
            assert!(polls < 10);
        }
        assert_eq!(polls, 3, "one frame to request, two to wait");
        assert_eq!(source.requests.get(), 1);
        assert_eq!(content.read("test/a.gma").unwrap(), [1]);
        assert_eq!(content.prepared_bytes(), 3);
        // Already there: ready at once, nothing fetched again.
        assert_eq!(content.prepare(&wanted), Prepare::Ready);
        assert_eq!(source.requests.get(), 1);
        // A file read once is handed over, not copied.
        assert_eq!(content.read_once("test/b.tpl.lz").unwrap(), [2, 2]);
        assert_eq!(content.prepared_bytes(), 1);
        content.release(&wanted);
        assert_eq!(content.prepared_bytes(), 0);
        assert!(content.read("test/a.gma").is_err());
    }

    #[test]
    fn direct_content_is_always_ready() {
        let content = Content::new(pack());
        assert!(!content.is_deferred());
        assert_eq!(
            content.prepare(&[Wanted::file("test/c.lz")]),
            Prepare::Ready
        );
        assert_eq!(content.read("./test/c.lz").unwrap(), [3, 3, 3]);
        assert_eq!(content.read_once("test/c.lz").unwrap(), [3, 3, 3]);
        assert_eq!(content.read("test/c.lz").unwrap(), [3, 3, 3]);
        assert!(content.misses().is_empty());
    }

    #[test]
    fn file_lists_cover_boot_stages_and_music() {
        let boot: Vec<String> = boot_files().into_iter().map(|w| w.path).collect();
        assert!(boot.contains(&"test/bmp/bmp_nml.tpl".to_string()));
        assert!(boot.contains(&"test/snd/mkb/comn.samp".to_string()));
        assert!(boot.contains(&MAIN_DOL_PATH.to_string()));

        let s1 = StageFiles::for_stage(1);
        assert_eq!(s1.stagedef, "test/st001/STAGE001.lz");
        assert_eq!(s1.bg_gma, "test/bg/bg_jun.gma");
        assert_eq!(s1.music, ["test/snd/adp/st1.adp"]);
        assert!(!s1.naomi && s1.wanted().len() == 3 + 2 + 4 + 1);
        let s10 = StageFiles::for_stage(10);
        assert!(s10.naomi);
        assert!(
            s10.wanted()
                .contains(&Wanted::file("test/st010/st010_p.lz"))
        );

        let ids = course_stage_ids();
        assert!(ids.len() > 100 && ids.contains(&1) && ids.contains(&92));
        let all = all_files();
        for id in ids {
            for w in StageFiles::for_stage(id).wanted() {
                assert!(all.contains(&w), "{w:?} of stage {id}");
            }
        }
        // A disc without the menu's sheets or a stage's stagedef is refused by name.
        let nothing = |_: &str| false;
        assert!(import_list(&nothing).unwrap_err().contains("bmp_nml"));
        let no_stage_7 = |p: &str| !p.contains("STAGE007");
        assert!(import_list(&no_stage_7).unwrap_err().contains("STAGE007"));
        let everything = |_: &str| true;
        let list = import_list(&everything).unwrap();
        // Where both exist the plain file is the one copied.
        assert!(list.contains(&"test/init/common.gma".to_string()));
        assert!(!list.contains(&"test/init/common.gma.lz".to_string()));
    }
}
