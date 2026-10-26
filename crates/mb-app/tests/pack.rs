//! The browser's way of reading the game, without a browser: the disc image is imported into a game pack
//! (`mb_app::import`), the pack is put behind a deferred source (files must be prepared before they are
//! read, fetches take a few frames), and the real menu, loading and gameplay states run on top of it.
//!
//! Needs the user's own disc image (`MB_DISC`, or `Super Monkey Ball*.rvz|iso|gcm` next to the repository);
//! the tests that draw also need a GPU adapter. Without either they skip. The pack lives in memory only.

mod common;

use common::find_disc;

use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use mb_app::content::{
    Content, DeferredPack, LatePack, Prepare, StageFiles, Wanted, all_files, boot_files,
    course_stage_ids,
};
use mb_app::headless::{FRAME, Headless};
use mb_app::host::Command;
use mb_app::import::{Importer, import_to_vec, write_pack_file};
use mb_app::import_screen::{DiscImage, ImageSlot, ImportScreen};
use mb_app::input::Action;
use mb_app::menu::Menu;
use mb_app::stage_scene::{AssetCache, LoadTimings, StageAssets};
use mb_assets::disc::{DiscStream, FileStream};
use mb_assets::gamepack::PackBytes;
use mb_assets::pack::PackProvider;
use mb_engine::Phase;
use mb_game::{MemoryStorage, Profile};

static GPU: Mutex<()> = Mutex::new(());

/// The imported pack of the available disc image (imported once per test run), or `None` without one.
fn pack_bytes() -> Option<&'static [u8]> {
    static PACK: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    PACK.get_or_init(|| {
        let Some(disc) = find_disc() else {
            eprintln!("skipped: no disc image (set MB_DISC)");
            return None;
        };
        let stream = FileStream::open(&disc).expect("open the disc image");
        Some(import_to_vec(stream).expect("import"))
    })
    .as_deref()
}

/// Files and bytes the import copies from the USA disc (144.1 MB of its 475 MB), as of version 0.1.0.
const PACK_FILES_0_1_0: usize = 382;
const PACK_BYTES_0_1_0: u64 = 144_084_942;

/// Deferred content over `provider`; fetches take `delay` frames.
fn deferred(provider: impl PackProvider + 'static, delay: u32) -> (Content, Rc<DeferredPack>) {
    let source = Rc::new(DeferredPack::new(Rc::new(provider), delay));
    (Content::deferred(source.clone()), source)
}

/// Poll `prepare` to its end, as a state would over several frames.
fn prepare(content: &Content, wanted: &[Wanted]) -> Result<u32, String> {
    for polls in 0..100 {
        match content.prepare(wanted) {
            Prepare::Ready => return Ok(polls),
            Prepare::Pending { .. } => {}
            Prepare::Failed(e) => return Err(e),
        }
    }
    Err("prepare did not finish".into())
}

/// Where the platform hands the app an open file instead of a path, the image is read the same way.
#[test]
fn a_disc_image_opens_from_an_open_file_as_from_its_path() {
    let Some(path) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return;
    };
    let by_path = mb_app::content::open_disc_image(&path).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let by_file = mb_app::content::open_disc_stream(Box::new(FileStream::from(file))).unwrap();
    assert!(!by_file.is_deferred());
    for wanted in boot_files() {
        let name = wanted.resolve(&|p| by_path.contains(p)).unwrap();
        assert!(by_file.contains(&name), "{name}");
        assert_eq!(by_file.read(&name).unwrap(), by_path.read(&name).unwrap());
    }
    assert_eq!(by_file.main_dol(), by_path.main_dol());
}

/// The pack as a platform with a file system keeps it: imported into a file, read back from the file.
#[test]
fn a_pack_file_reads_as_the_image_does_and_a_cut_import_leaves_none() {
    let Some(path) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return;
    };
    let dir = std::env::temp_dir().join(format!("mb-pack-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let pack = dir.join("game.pack");
    let image = mb_app::content::open_disc_image(&path).unwrap();

    // From an open file, not a path: what a platform has that is handed a file descriptor.
    let opened = FileStream::from(std::fs::File::open(&path).unwrap());
    let mut calls = 0;
    let len = write_pack_file(Importer::open(opened).unwrap(), &pack, &mut |d, t| {
        assert!(d <= t);
        calls += 1;
    })
    .unwrap();
    assert!(calls > 100, "progress was reported {calls} times");
    assert_eq!(len, std::fs::metadata(&pack).unwrap().len());
    assert_eq!(len, pack_bytes().unwrap().len() as u64);
    assert!(!dir.join("game.pack.tmp").exists());
    {
        let (content, id, size) = mb_app::content::open_pack_file(&pack).unwrap();
        assert_eq!((id.as_str(), size), ("GMBE8P", len));
        assert!(!content.is_deferred());
        for wanted in boot_files() {
            let name = wanted.resolve(&|p| image.contains(p)).unwrap();
            assert_eq!(content.read(&name).unwrap(), image.read(&name).unwrap());
        }
        assert_eq!(content.main_dol(), image.main_dol());
    }

    // An import replaces the pack that is there; one that fails leaves it alone and no temp file.
    let again = Importer::open(FileStream::open(&path).unwrap()).unwrap();
    write_pack_file(again, &pack, &mut |_, _| {}).unwrap();
    let not_a_disc = dir.join("junk.iso");
    std::fs::write(&not_a_disc, vec![0u8; 4096]).unwrap();
    let failed = Importer::open(FileStream::open(&not_a_disc).unwrap())
        .and_then(|i| write_pack_file(i, &pack, &mut |_, _| {}));
    assert!(failed.is_err());
    assert!(!dir.join("game.pack.tmp").exists());
    assert!(mb_app::content::open_pack_file(&pack).is_ok());

    // A pack cut short, at any of a few lengths, is refused, and so is one with a damaged file in it.
    let whole = std::fs::read(&pack).unwrap();
    for keep in [0, 16, 32, whole.len() / 2, whole.len() - 1] {
        std::fs::write(&pack, &whole[..keep]).unwrap();
        assert!(
            mb_app::content::open_pack_file(&pack).is_err(),
            "{keep} bytes"
        );
    }
    let mut damaged = whole;
    damaged[4096] ^= 0xff;
    std::fs::write(&pack, &damaged).unwrap();
    let (content, ..) = mb_app::content::open_pack_file(&pack).unwrap();
    let bad = all_files()
        .iter()
        .filter_map(|w| w.resolve(&|p| content.contains(p)))
        .filter(|name| content.read(name).is_err())
        .count();
    assert_eq!(bad, 1, "exactly the damaged file fails its checksum");
    drop(content);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Draw `h` and, with `MB_SHOT_DIR` set, keep the picture there as `<name>.png`.
fn shot(h: &mut Headless, name: &str) {
    let rgba = h.render().unwrap();
    if let Some(dir) = std::env::var_os("MB_SHOT_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (w, height) = h.size();
        mb_app::image::write_image(&dir.join(format!("{name}.png")), w, height, &rgba).unwrap();
    }
}

/// Frames of `h` until its status passes `done` (the copying thread needs real time, so the frames are
/// a millisecond apart), at most a minute. Returns every status seen on the way.
fn run_while_copying(h: &mut Headless, done: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut seen = Vec::new();
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < 60 {
        h.step(FRAME).unwrap();
        let status = h.status();
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if done(&status) {
            return seen;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("no end after a minute: {:?}", seen.last());
}

/// The first run of a native app: the import screen asks the platform for an image, copies it on a
/// thread of its own while the frames go on, shows a failure with a way back to another image, and
/// starts the game on the finished pack.
#[test]
fn the_import_screen_copies_beside_the_frames_fails_with_a_way_back_and_starts_the_game() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(path) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return;
    };
    let dir = std::env::temp_dir().join(format!("mb-import-screen-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A file where the pack's folder should be: the first import fails once it has started.
    let blocker = dir.join("data");
    std::fs::write(&blocker, b"in the way").unwrap();
    let pack = blocker.join("game.pack");
    let open = || -> DiscImage {
        let stream: Box<dyn DiscStream + Send> = Box::new(FileStream::open(&path).unwrap());
        Importer::open(stream).unwrap()
    };

    let data = Rc::new(LatePack::default());
    let content = Content::new(data.clone());
    let mut h = match Headless::new(&content, (640, 360), false) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped: no GPU adapter ({e})");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
    };
    let slot = ImageSlot::default();
    let menu = Menu::new(Box::new(MemoryStorage::new()));
    let screen = ImportScreen::new(pack.clone(), data, slot.clone(), Box::new(menu));
    h.push(Box::new(screen)).unwrap();
    assert_eq!(h.host.drain(), [Command::PickImage]);
    h.step(FRAME).unwrap();
    assert_eq!(h.status(), "import: waiting for an image");

    slot.offer(open());
    run_while_copying(&mut h, &|s| s.starts_with("import: failed"));
    assert!(h.top().unwrap().notice().is_some());
    shot(&mut h, "import_failed");
    // Back: another image is asked for.
    std::fs::remove_file(&blocker).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.host.drain(), [Command::PickImage]);
    assert_eq!(h.status(), "import: waiting for an image");

    slot.offer(open());
    let seen = run_while_copying(&mut h, &|s| s.starts_with("import: done"));
    let between = seen.iter().filter(|s| s.contains(" of ")).count();
    assert!(between > 10, "the frames went on while it copied: {seen:?}");
    let len = std::fs::metadata(&pack).unwrap().len();
    assert_eq!(h.host.drain(), [Command::GameDataReady { bytes: len }]);
    assert_eq!(len, pack_bytes().unwrap().len() as u64);
    assert!(!blocker.join("game.pack.tmp").exists());
    shot(&mut h, "import_done");
    // The result is shown for a moment, then the game runs on the new pack.
    run_while_copying(&mut h, &|s| s == "menu: title");
    assert!(
        content.contains("test/bmp/bmp_nml.tpl") || content.contains("test/bmp/bmp_nml.tpl.lz")
    );
    h.shutdown();
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_import_copies_exactly_what_the_game_reads() {
    let Some(bytes) = pack_bytes() else {
        return;
    };
    let pack = PackBytes::open(bytes.to_vec()).expect("a complete pack");
    let disc = find_disc().unwrap();
    let importer = Importer::open(FileStream::open(&disc).unwrap()).unwrap();
    assert_eq!(importer.game_id().len(), 6);
    assert_eq!(pack.index().len(), importer.file_count());
    // Header, the files back to back, then the index.
    assert!(bytes.len() as u64 > importer.total_bytes() + 32);
    // Every file of the lists resolves to at most one pack entry, and every pack entry is on a list.
    let listed: Vec<String> = all_files()
        .iter()
        .filter_map(|w| w.resolve(&|p| pack.contains(p)))
        .collect();
    assert_eq!(listed.len(), pack.index().len());
    for name in pack.index().names() {
        assert!(listed.iter().any(|l| l == name), "{name} is not on a list");
    }
    eprintln!(
        "game pack: {} files, {:.1} MB of the image's files ({} bytes)",
        pack.index().len(),
        importer.total_bytes() as f64 / 1e6,
        bytes.len()
    );
    // The storage budget: what is copied from the USA disc. A loader
    // that reads a new file changes these numbers; that is then a deliberate edit here, in `docs/WEB.md`
    // and of `mb_assets::gamepack::VERSION`.
    if importer.game_id() == "GMBE8P" {
        assert_eq!(pack.index().len(), PACK_FILES_0_1_0);
        assert_eq!(importer.total_bytes(), PACK_BYTES_0_1_0);
    }
    // By kind, for the documentation: everything in the pack is something a loader reads.
    let mut kinds = [
        ("stages", 0u64),
        ("backgrounds", 0),
        ("shared models", 0),
        ("HUD and fonts", 0),
        ("music", 0),
        ("sound effects", 0),
        ("executable", 0),
    ];
    for name in pack.index().names() {
        let len = u64::from(pack.index().get(name).unwrap().len);
        let kind = match name {
            n if n.contains("/snd/adp/") => 4,
            n if n.contains("/snd/") => 5,
            n if n.contains("/bg/") => 1,
            n if n.contains("/init/") => 2,
            n if n.contains("/bmp/") => 3,
            n if n.starts_with("sys/") => 6,
            n if n.contains("/st") => 0,
            other => panic!("{other} is in the pack but belongs to no category"),
        };
        kinds[kind].1 += len;
    }
    assert_eq!(
        kinds.iter().map(|k| k.1).sum::<u64>(),
        importer.total_bytes()
    );
    eprintln!("  {kinds:?}");
}

/// Every stage of every course: prepare its files from the pack, load it with the real loader, read its
/// music, release. No file may be read that was not prepared, and nothing may stay behind.
#[test]
fn every_course_stage_prepares_and_loads_from_the_pack() {
    let Some(bytes) = pack_bytes() else {
        return;
    };
    let (content, source) = deferred(PackBytes::open(bytes.to_vec()).unwrap(), 1);
    prepare(&content, &boot_files()).expect("boot files");
    for w in boot_files() {
        let path = w.resolve(&|p| content.contains(p)).expect("on the disc");
        assert!(!content.read(&path).unwrap().is_empty(), "{path}");
    }
    assert!(content.main_dol().is_some());
    content.release(&boot_files());
    assert_eq!(content.prepared_bytes(), 0);

    let ids = course_stage_ids();
    // Unoptimized builds parse slowly: they load a sample (and still prepare and read every stage's
    // files); release builds, which `scripts/check.sh` runs, load every stage.
    let parse_every = if cfg!(debug_assertions) { 6 } else { 1 };
    let mut cache = AssetCache::default();
    let (mut peak, mut total_fetched) = (0usize, 0usize);
    let mut fetches_with_cache = 0;
    for (n, id) in ids.iter().copied().enumerate() {
        let files = StageFiles::for_stage(id);
        let mut wanted = cache.wanted(&files);
        wanted.extend(files.music_wanted());
        let before = source.requests.get();
        prepare(&content, &wanted).unwrap_or_else(|e| panic!("stage {id}: {e}"));
        fetches_with_cache += source.requests.get() - before;
        peak = peak.max(content.prepared_bytes());
        total_fetched += content.prepared_bytes();
        if n % parse_every == 0 {
            let mut timings = LoadTimings::default();
            let assets = StageAssets::load_timed(&content, &files, &mut cache, &mut timings)
                .unwrap_or_else(|e| panic!("stage {id}: {e}"));
            assert_eq!(assets.id, id);
            assert!(assets.stage_gma.is_some() || assets.naomi.is_some());
            assert!(assets.common.nl.is_some(), "goal tape and clock digits");
        } else {
            for w in &wanted {
                if let Some(path) = w.resolve(&|p| content.contains(p)) {
                    content
                        .read(&path)
                        .unwrap_or_else(|e| panic!("{path}: {e}"));
                }
            }
        }
        for path in &files.music {
            assert!(content.read(path).is_ok(), "stage {id}: {path}");
        }
        content.release(&wanted);
        assert_eq!(
            content.prepared_bytes(),
            0,
            "stage {id} left files in memory"
        );
    }
    assert_eq!(content.misses(), Vec::<String>::new());
    assert_eq!(
        fetches_with_cache as usize,
        ids.len(),
        "one fetch per stage"
    );
    eprintln!(
        "{} stages from the pack: largest stage fetch {:.1} MB, average {:.1} MB",
        ids.len(),
        peak as f64 / 1e6,
        total_fetched as f64 / 1e6 / ids.len() as f64
    );
}

fn unlocked() -> Box<MemoryStorage> {
    let mut storage = MemoryStorage::new();
    let mut profile = Profile::default();
    profile.settings.unlock_all = true;
    profile.save(&mut storage).unwrap();
    Box::new(storage)
}

/// A headless game at the title menu over `content`, the way the browser shell starts it: the start-up
/// files are prepared before the menu exists.
fn start(content: &Content) -> Option<Headless> {
    let mut h = match Headless::new(content, (640, 360), false) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped: no GPU adapter ({e})");
            return None;
        }
    };
    prepare(content, &boot_files()).expect("boot files");
    h.push(Box::new(Menu::new(unlocked()))).expect("menu");
    assert!(h.step(FRAME).unwrap());
    assert_eq!(h.status(), "menu: title");
    assert_eq!(content.prepared_bytes(), 0, "the menu released its files");
    Some(h)
}

fn run_until(h: &mut Headless, limit: u32, what: &str, mut done: impl FnMut(&Headless) -> bool) {
    for _ in 0..limit {
        assert!(h.step(FRAME).unwrap(), "the game quit waiting for {what}");
        if done(h) {
            return;
        }
    }
    panic!(
        "{what} did not happen in {limit} frames; at {:?}",
        h.status()
    );
}

fn playing(h: &Headless) -> Option<(u32, u32, bool, u32)> {
    let v = h.top()?.play_view()?;
    Some((
        v.request.stage_id,
        v.request.floor,
        v.request.retry,
        v.request.lives,
    ))
}

/// The loading state is on screen while the files are fetched, the stage then runs with nothing left in
/// memory, a lost life retries on the same scene without fetching or loading anything, and the next floor
/// fetches only its own files.
#[test]
fn stages_load_through_the_loading_screen_and_retry_in_place() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bytes) = pack_bytes() else {
        return;
    };
    let (content, source) = deferred(PackBytes::open(bytes.to_vec()).unwrap(), 3);
    let Some(mut h) = start(&content) else {
        return;
    };
    // Title -> Course Play -> Beginner.
    h.tap(Action::Confirm).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "loading stage 1");
    assert!(h.top().unwrap().play_view().is_none());
    assert_eq!(h.depth(), 2);
    let rgba = h.render().unwrap();
    let (pixels, _) = rgba.as_chunks::<4>();
    assert!(
        pixels.iter().any(|p| *p != pixels[0]),
        "the loading screen draws its bar and text"
    );
    let mut loading_frames = 1;
    run_until(&mut h, 60, "floor 1", |h| {
        loading_frames += (h.status() == "loading stage 1") as u32;
        playing(h).is_some()
    });
    // Three frames of fetching, then one each for parsing, music and building.
    assert!(loading_frames >= 6, "only {loading_frames} loading frames");
    assert_eq!(playing(&h), Some((1, 1, false, 3)));
    assert_eq!(
        content.prepared_bytes(),
        0,
        "raw files are released after the load"
    );
    let fetches_floor_1 = source.requests.get();

    // Forward through the goal of floor 1 into floor 2: one more fetch, of that stage's own files only.
    h.input.set_action(Action::MoveForward, true);
    run_until(&mut h, 3000, "floor 2", |h| {
        playing(h).is_some_and(|p| p.1 == 2)
    });
    assert_eq!(source.requests.get(), fetches_floor_1 + 1);
    // ... and off its edge: a life is lost and the floor starts again at once, on the same scene.
    let fetches = source.requests.get();
    let mut fell = false;
    run_until(&mut h, 3000, "the retry", |h| {
        assert_ne!(h.status(), "loading stage 2", "a retry must not reload");
        let Some(view) = h.top().and_then(|g| g.play_view()) else {
            return false;
        };
        fell |= view.sim.phase == Phase::FallOut;
        fell && view.request.retry && view.sim.phase == Phase::Intro
    });
    assert_eq!(playing(&h), Some((2, 2, true, 2)));
    assert_eq!(source.requests.get(), fetches, "the retry fetched nothing");
    assert_eq!(content.prepared_bytes(), 0);
    let view = h.top().unwrap().play_view().unwrap();
    assert!(view.sim.intro_timer <= 120, "retries get the short intro");
    h.input.set_action(Action::MoveForward, false);

    // Pause -> Quit (a course asks first): back at the title, the scene is gone, nothing is read.
    h.tap(Action::Pause).unwrap();
    h.tap(Action::LookUp).unwrap();
    h.tap(Action::Confirm).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: title");
    assert_eq!(h.depth(), 1);
    assert_eq!(content.misses(), Vec::<String>::new());
    h.shutdown();
}

/// A pack that lost one stage's stagedef and has a damaged byte in another stage's models.
struct Faulty {
    inner: PackBytes,
    missing: &'static str,
}

impl PackProvider for Faulty {
    fn fetch(&self, path: &str) -> std::io::Result<Vec<u8>> {
        if !self.contains(path) {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        self.inner.fetch(path)
    }
    fn contains(&self, path: &str) -> bool {
        mb_assets::pack::disc_path(path) != self.missing && self.inner.contains(path)
    }
}

/// A stage that cannot be loaded returns to the menu with a notice; the session goes on and other stages
/// still load.
#[test]
fn a_stage_that_fails_to_load_returns_to_the_menu_with_a_notice() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bytes) = pack_bytes() else {
        return;
    };
    let mut damaged = bytes.to_vec();
    let at = {
        let pack = PackBytes::open(bytes.to_vec()).unwrap();
        let entry = pack.index().get("test/st003/st003.gma").expect("stage 3");
        entry.offset as usize + entry.len as usize / 2
    };
    damaged[at] ^= 0x40;
    let faulty = Faulty {
        inner: PackBytes::open(damaged).unwrap(),
        missing: "test/st002/STAGE002.lz",
    };
    let (content, _) = deferred(faulty, 1);
    let Some(mut h) = start(&content) else {
        return;
    };
    // Title -> Practice -> Beginner.
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: stage select");

    // Stage 2: its stagedef is not in the pack.
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "loading stage 2");
    run_until(&mut h, 30, "the failure", |h| {
        h.status() != "loading stage 2"
    });
    assert_eq!(h.status(), "menu: stage select");
    assert_eq!(h.depth(), 1);
    let notice = h.top().unwrap().notice().expect("a notice is shown");
    assert!(
        notice.contains("COULD NOT LOAD STAGE 2") && notice.contains("STAGE002.LZ"),
        "{notice}"
    );
    assert_eq!(h.render().unwrap().len(), 640 * 360 * 4);

    // Stage 3: a file comes back damaged from storage.
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    run_until(&mut h, 30, "the failure", |h| {
        h.status() != "loading stage 3"
    });
    assert_eq!(h.status(), "menu: stage select");
    let notice = h.top().unwrap().notice().unwrap();
    assert!(
        notice.contains("STAGE 3") && notice.contains("DAMAGED"),
        "{notice}"
    );

    // Stage 1 is fine and plays; choosing it clears the notice.
    h.tap(Action::LookUp).unwrap();
    h.tap(Action::LookUp).unwrap();
    h.tap(Action::Confirm).unwrap();
    run_until(&mut h, 60, "stage 1", |h| playing(h).is_some());
    assert_eq!(playing(&h).map(|p| p.0), Some(1));
    assert_eq!(content.prepared_bytes(), 0);

    // Backing out of a loading screen is not an error.
    h.tap(Action::Pause).unwrap();
    h.tap(Action::LookUp).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "menu: stage select");
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::LookDown).unwrap();
    h.tap(Action::Confirm).unwrap();
    assert_eq!(h.status(), "loading stage 4");
    h.tap(Action::Cancel).unwrap();
    assert_eq!(h.status(), "menu: stage select");
    assert!(h.top().unwrap().notice().is_none());
    assert_eq!(content.prepared_bytes(), 0);
    h.shutdown();
}

/// The import protocol against a sudden exit: the importer's writes (file pieces in order, then the index,
/// then the header) are replayed onto an empty file and cut off after every possible number of them. What
/// is left is never accepted as a pack until the very last write has landed, so a start after a crash
/// always sees "no game data" (and cleans up), never a half-imported game.
#[test]
fn an_import_cut_off_after_any_write_is_never_taken_for_a_pack() {
    let Some(disc) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return;
    };
    // Record the writes: (offset, bytes).
    let mut importer = Importer::open(FileStream::open(&disc).unwrap()).unwrap();
    let mut writes: Vec<(u64, Vec<u8>)> = Vec::new();
    while let Some((at, bytes)) = importer.step().unwrap() {
        writes.push((at, bytes.to_vec()));
    }
    let done = importer.finish().unwrap();
    writes.push((done.index_offset, done.index.clone()));
    writes.push((0, done.header.to_vec()));
    assert!(writes.len() > 100, "{} writes", writes.len());

    // The file as it grows. Checking a prefix of the writes costs a header parse, so every count is tried.
    let mut file: Vec<u8> = Vec::new();
    let apply = |file: &mut Vec<u8>, (at, bytes): &(u64, Vec<u8>)| {
        let at = *at as usize;
        if file.len() < at + bytes.len() {
            file.resize(at + bytes.len(), 0);
        }
        file[at..at + bytes.len()].copy_from_slice(bytes);
    };
    for (n, write) in writes.iter().enumerate() {
        let header = mb_assets::gamepack::Header::parse(&file, file.len() as u64);
        assert!(
            header.is_err(),
            "accepted after only {n} of {} writes",
            writes.len()
        );
        // A write that itself was cut short (half of its bytes) is no better.
        let half = (write.0, write.1[..write.1.len() / 2].to_vec());
        let mut torn = file.clone();
        apply(&mut torn, &half);
        if n + 1 == writes.len() {
            assert!(
                PackBytes::open(torn).is_err(),
                "accepted with half a header"
            );
        } else {
            assert!(mb_assets::gamepack::Header::parse(&torn, torn.len() as u64).is_err());
        }
        apply(&mut file, write);
    }
    let pack = PackBytes::open(file).expect("complete after the last write");
    assert!(pack.index().len() > 300);
}

/// The particles and the shadow over the pack, the way the browser reads: everything they are drawn with
/// (confetti, star and disc models, the shadow's mask, the sparkle's texture) comes from the shared files
/// that are prepared for every stage, so nothing is read unprepared and nothing new is in the pack.
#[test]
fn particles_and_the_shadow_are_drawn_from_the_pack() {
    use mb_engine::BallEffectKind as Kind;
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bytes) = pack_bytes() else {
        return;
    };
    let (content, _) = deferred(PackBytes::open(bytes.to_vec()).unwrap(), 2);
    let Some(mut h) = start(&content) else {
        return;
    };
    // Title -> Course Play -> Beginner, then forward through the goal of floor 1.
    h.tap(Action::Confirm).unwrap();
    h.tap(Action::Confirm).unwrap();
    run_until(&mut h, 60, "floor 1", |h| playing(h).is_some());
    h.input.set_action(Action::MoveForward, true);
    let (mut streaks, mut burst, mut confetti, mut sparkles) = (false, false, false, false);
    let (mut shadow_on, mut shadow_off) = (false, false);
    let mut peak = 0;
    for _ in 0..3000 {
        assert!(h.step(FRAME).unwrap());
        let Some(view) = h.top().unwrap().play_view() else {
            break;
        };
        if view.request.floor != 1 {
            break;
        }
        h.render().unwrap();
        let stats = h.renderer().stats;
        let view = h.top().unwrap().play_view().unwrap();
        let rt = &view.sim.runtime;
        let count = |kind: Kind| rt.effects.iter().filter(|e| e.kind == kind).count();
        let visible = view.sim.ball.flags & mb_engine::constants::ball_flags::INVISIBLE == 0;
        assert_eq!(view.shadow, visible, "tick {}", view.sim.tick);
        shadow_on |= view.shadow;
        shadow_off |= !view.shadow;
        if !rt.confetti.is_empty() {
            assert!(stats.instances >= rt.confetti.len(), "{stats:?}");
            confetti = true;
        }
        if count(Kind::Levitate) > 0 {
            assert!(stats.quads >= 4 * count(Kind::Levitate), "{stats:?}");
            sparkles = true;
        }
        streaks |= count(Kind::Coli) > 0 && stats.quads > 0;
        burst |= count(Kind::ColiStar) + count(Kind::ColiFlash) > 0 && stats.instance_draws >= 2;
        peak = peak.max(stats.instance_draws + stats.quad_draws);
        assert!(peak <= 12, "{stats:?}");
    }
    h.input.set_action(Action::MoveForward, false);
    assert!(streaks && burst, "streaks {streaks}, landing burst {burst}");
    assert!(
        confetti && sparkles,
        "confetti {confetti}, sparkles {sparkles}"
    );
    assert!(shadow_on && shadow_off);
    assert!(peak >= 6, "peak {peak}");
    assert_eq!(content.misses(), Vec::<String>::new());
    h.shutdown();
}

/// The window and fire flipbooks, the drifting clouds and the rippling floor of stage 92 over the pack,
/// the way the browser reads: every frame model and texture comes from the background and stage files
/// that are prepared for the stage anyway, so nothing is read unprepared, nothing stays in memory and
/// the pack is what it was (`the_import_copies_exactly_what_the_game_reads` holds its size).
#[test]
fn backdrop_flipbooks_clouds_and_the_wave_load_from_the_pack() {
    use mb_app::backdrop::FlipbookKind;
    use mb_app::menu::Autostart;
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bytes) = pack_bytes() else {
        return;
    };
    let (content, _) = deferred(PackBytes::open(bytes.to_vec()).unwrap(), 2);
    let Some(mut h) = start(&content) else {
        return;
    };
    // (stage, window flipbooks, fire flipbooks, cloud layers, waved objects)
    for (stage, windows, fires, clouds, waves) in [
        (21u32, 23usize, 0usize, 0usize, 0usize),
        (81, 0, 8, 0, 0),
        (5, 0, 0, 2, 0),
        (92, 0, 0, 0, 1),
    ] {
        h.shutdown();
        // A new menu reads the start-up files again, as a restarted page would.
        prepare(&content, &boot_files()).expect("boot files");
        let menu = Menu::new(unlocked()).with_autostart(Some(Autostart::Stage(stage)));
        h.push(Box::new(menu)).expect("menu");
        run_until(&mut h, 60, "the stage", |h| playing(h).is_some());
        assert_eq!(playing(&h).map(|p| p.0), Some(stage));
        assert_eq!(content.prepared_bytes(), 0, "stage {stage}");
        h.render().unwrap();
        let created = h.renderer().counters();
        let first: Vec<Option<usize>> = {
            let view = h.top().unwrap().play_view().unwrap();
            let scene = &view.scene.scene;
            let count = |window: bool| {
                let is_window = |b: &&mb_app::backdrop::Flipbook| {
                    (b.kind == FlipbookKind::NightWindow) == window
                };
                scene.flipbooks.iter().filter(is_window).count()
            };
            assert_eq!(
                (count(true), count(false)),
                (windows, fires),
                "stage {stage}"
            );
            // Every frame of every flipbook was found in the prepared background archive.
            for book in &scene.flipbooks {
                assert!(book.frames.iter().all(Option::is_some), "stage {stage}");
            }
            assert_eq!(scene.clouds.len(), clouds, "stage {stage}");
            assert_eq!(scene.wave_objects.len(), waves, "stage {stage}");
            scene.flipbooks.iter().map(|b| b.shown()).collect()
        };
        let offset = |h: &Headless| {
            let view = h.top().unwrap().play_view().unwrap();
            view.scene.clouds().offset(0)
        };
        let cloud_start = offset(&h);
        assert_eq!(cloud_start.is_some(), clouds > 0);
        // Through the fly-in and into play, drawing now and then.
        for frame in 0..420 {
            assert!(h.step(FRAME).unwrap());
            if frame % 60 == 0 {
                h.render().unwrap();
            }
        }
        h.render().unwrap();
        assert_eq!(
            h.renderer().counters(),
            created,
            "stage {stage} created something"
        );
        let view = h.top().unwrap().play_view().unwrap();
        assert!(view.sim.runtime.timer_frames > 100);
        let now: Vec<Option<usize>> = (view.scene.scene.flipbooks.iter())
            .map(|b| b.shown())
            .collect();
        if windows + fires > 0 {
            assert_ne!(now, first, "stage {stage}: the flipbooks moved on");
        }
        if clouds > 0 {
            assert_ne!(offset(&h), cloud_start, "the clouds drifted");
        }
        assert_eq!(content.misses(), Vec::<String>::new(), "stage {stage}");
    }
    h.shutdown();
}

/// Mirror floors over the pack, the way the browser reads (the wave, the first two mirrors, a bonus
/// floor, and floors with mirror models on many anim groups and at two heights): the mirror models are
/// in the stage's own archive, which is prepared for the stage anyway, so nothing is read unprepared,
/// nothing stays in memory and the pack is what it was (`the_import_copies_exactly_what_the_game_reads`
/// holds its files and size). With the Reflections option off in the save, the same stages load
/// without a mirror.
#[test]
fn mirror_floors_load_from_the_pack() {
    use mb_app::menu::Autostart;
    use mb_app::mirror::MirrorKind;
    use mb_app::stage_scene::ItemKind;
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bytes) = pack_bytes() else {
        return;
    };
    let (content, _) = deferred(PackBytes::open(bytes.to_vec()).unwrap(), 2);
    let Some(mut h) = start(&content) else {
        return;
    };
    let save = |reflections: bool| {
        let mut storage = MemoryStorage::new();
        let mut profile = Profile::default();
        profile.settings.unlock_all = true;
        profile.settings.reflections = reflections;
        profile.save(&mut storage).unwrap();
        Box::new(storage)
    };
    // (stage, mode, mirror models among the stage's items); stage 1 has no mirror.
    for (stage, kind, models) in [
        (92u32, Some(MirrorKind::Wavy), 0usize),
        (102, Some(MirrorKind::Flat), 1),
        (103, Some(MirrorKind::Flat), 1),
        (91, Some(MirrorKind::Flat), 1),
        (106, Some(MirrorKind::Flat), 16),
        (110, Some(MirrorKind::Flat), 2),
        (121, Some(MirrorKind::Flat), 24),
        (1, None, 0),
    ] {
        for reflections in [true, false] {
            h.shutdown();
            // A new menu reads the start-up files again, as a restarted page would.
            prepare(&content, &boot_files()).expect("boot files");
            let menu = Menu::new(save(reflections)).with_autostart(Some(Autostart::Stage(stage)));
            h.push(Box::new(menu)).expect("menu");
            run_until(&mut h, 60, "the stage", |h| playing(h).is_some());
            assert_eq!(playing(&h).map(|p| p.0), Some(stage));
            assert_eq!(content.prepared_bytes(), 0, "stage {stage}");
            h.render().unwrap();
            let created = h.renderer().counters();
            let expect = kind.filter(|_| reflections);
            {
                let view = h.top().unwrap().play_view().unwrap();
                let scene = &view.scene.scene;
                assert_eq!(scene.mirror.as_ref().map(|m| m.kind), expect);
                let found = scene.items.iter().filter(|it| it.kind == ItemKind::Mirror);
                let wanted = if expect.is_some() { models } else { 0 };
                assert_eq!(found.count(), wanted, "stage {stage}");
                assert!(scene.missing_models.iter().all(|m| !m.contains("MIR")));
            }
            let set = h.renderer().mirror().copied();
            assert_eq!(set.is_some(), expect.is_some(), "stage {stage}");
            // Through the fly-in and into play, drawing now and then.
            let mut mirrored = 0;
            for frame in 0..420 {
                assert!(h.step(FRAME).unwrap());
                if frame % 30 == 0 {
                    h.render().unwrap();
                    mirrored += (h.renderer().stats.mirror_draws > 0) as u32;
                }
            }
            h.render().unwrap();
            if expect.is_some() {
                assert!(mirrored >= 10, "stage {stage}: {mirrored} of 14 frames");
                assert!(h.renderer().mirror_bytes() > 0);
            } else {
                assert_eq!(mirrored, 0, "stage {stage}");
                assert_eq!(h.renderer().mirror_bytes(), 0);
            }
            assert_eq!(
                h.renderer().counters(),
                created,
                "stage {stage} created something"
            );
            assert_eq!(content.misses(), Vec::<String>::new(), "stage {stage}");
            assert_eq!(content.prepared_bytes(), 0, "stage {stage}");
        }
    }
    h.shutdown();
}
