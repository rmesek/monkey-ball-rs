//! The desktop program (Windows, Linux, macOS): one of the platform drivers on top of the game library
//! `mb-app` (see `docs/ARCHITECTURE.md`). [`app`] is the window and its event loop; this file is the command
//! line, the file dialog, and where the game data and the saves are kept.
//!
//! The executable is called `monkey-ball`. The first time it asks for the disc image and copies the files the
//! game uses out of it into one pack file beside the saves, on the game's import screen; from then on it
//! starts from that pack and the image is not needed. Options are listed in [`USAGE`].

use std::path::{Path, PathBuf};
use std::rc::Rc;

pub mod app;

use app::WindowOptions;
use mb_app::audio::Audio;
use mb_app::content::{Content, LatePack, open_disc_image, open_pack_file};
use mb_app::game::Game;
use mb_app::host::{Caps, Host};
use mb_app::import::Importer;
use mb_app::import_screen::{DiscImage, ImageSlot, ImportScreen};
use mb_app::menu::{Autostart, Menu};
use mb_assets::disc::{DiscStream, FileStream};
use mb_game::{Difficulty, FileStorage, MemoryStorage, SlotStorage, Storage};

const USAGE: &str = "usage: monkey-ball [disc image] [options]

The disc image is your own copy of Super Monkey Ball (Nintendo GameCube, 2001): an RVZ or ISO/GCM file,
for example 'Super Monkey Ball (USA).rvz'. It is needed once: the files the game uses (about 145 MB) are
copied into the program's data folder, and later starts need no argument and no image. With no game data
yet, a file dialog asks for the image.

  --pick                   ask for a disc image and import it again
  --fullscreen             start in fullscreen (F11 or Alt+Enter toggles)
  --stage <number>         start practice on that stage directly
  --course <name>          start course play directly (beginner, advanced, expert, ...)
  --mute                   do not open an audio device";

/// The imported game data, in the save directory.
const PACK_FILE: &str = "game.pack";

/// The one developer option, of a build with the `dev` feature (the kill test uses it).
const DEV_USAGE: &str = if cfg!(feature = "dev") {
    "\n  --exit-after <seconds>   (dev) close the window after that long and print frame pacing and audio output"
} else {
    ""
};

struct Args {
    source: Option<PathBuf>,
    stage: Option<u32>,
    pick: bool,
    fullscreen: bool,
    course: Option<Difficulty>,
    mute: bool,
    exit_after: Option<f32>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        source: None,
        stage: None,
        pick: false,
        fullscreen: false,
        course: None,
        mute: false,
        exit_after: None,
    };
    let mut it = args;
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--course" => {
                let v = value("--course")?;
                out.course = Some(
                    Difficulty::from_key(&v.to_lowercase())
                        .ok_or_else(|| format!("unknown course {v:?}"))?,
                );
            }
            "--stage" => {
                let v = value("--stage")?;
                let id = v
                    .parse::<u32>()
                    .map_err(|_| format!("bad stage number {v:?}"))?;
                if !mb_game::stages::is_playable_stage(id) {
                    return Err(format!(
                        "stage {id} is not part of the game (no course uses it)"
                    ));
                }
                out.stage = Some(id);
            }
            "--mute" => out.mute = true,
            "--pick" => out.pick = true,
            "--fullscreen" => out.fullscreen = true,
            "-h" | "--help" => return Err(String::new()),
            "--exit-after" if cfg!(feature = "dev") => {
                let v = value("--exit-after")?;
                out.exit_after = Some(
                    v.parse::<f32>()
                        .ok()
                        .filter(|s| s.is_finite() && *s >= 0.0)
                        .ok_or_else(|| format!("bad --exit-after {v:?}"))?,
                );
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if out.source.is_none() => out.source = Some(PathBuf::from(a)),
            _ => {
                return Err(format!(
                    "unexpected argument {a:?} (one disc image at most)"
                ));
            }
        }
    }
    Ok(out)
}

/// Where this platform keeps everything: the saves (`profile.a`, `profile.b`: progress and settings) and
/// the imported game data ([`PACK_FILE`]). `MB_SAVE_DIR` if set, else the per-user data directory of the
/// operating system: `%APPDATA%\monkey-ball-rs`, `~/Library/Application Support/monkey-ball-rs`, or
/// `$XDG_DATA_HOME/monkey-ball-rs` (`~/.local/share/monkey-ball-rs`). This function is the only place the
/// desktop's location is decided; the game is handed a `Storage` and a `Content` and never asks where.
fn data_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let dir = if let Some(d) = var("MB_SAVE_DIR") {
        Some(PathBuf::from(d))
    } else if cfg!(windows) {
        var("APPDATA").map(|d| PathBuf::from(d).join("monkey-ball-rs"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|d| {
            PathBuf::from(d)
                .join("Library/Application Support")
                .join("monkey-ball-rs")
        })
    } else {
        var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("monkey-ball-rs"))
    };
    if dir.is_none() {
        eprintln!("no data directory found: nothing will be kept between runs");
    }
    dir
}

/// Saves: two checksummed slots per value (`SlotStorage`) written through a temp file and a rename, so
/// killing the program at any moment leaves the previous save or the new one. In memory without a
/// directory.
fn open_storage(dir: Option<&Path>) -> Box<dyn Storage> {
    match dir {
        Some(dir) => {
            // Versions up to 0.2.0 remembered the path of the disc image here. Nothing reads it any more,
            // and nothing is to lie about in this folder that the options screen does not name.
            for old in ["last_disc", "last_disc.a", "last_disc.b"] {
                let _ = std::fs::remove_file(dir.join(old));
            }
            Box::new(SlotStorage::new(FileStorage::new(dir)))
        }
        None => Box::new(MemoryStorage::new()),
    }
}

fn pick_disc() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open your Super Monkey Ball (GameCube) disc image: RVZ, ISO or GCM")
        .add_filter("GameCube disc image", &["rvz", "iso", "gcm"])
        .add_filter("All files", &["*"])
        .pick_file()
}

/// Open the disc image at `path` for an import; this checks that it is the game and has every file.
fn open_image(path: &Path) -> Result<DiscImage, String> {
    let stream: Box<dyn DiscStream + Send> =
        Box::new(FileStream::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
    Importer::open(stream).map_err(|e| format!("{}: {e}", path.display()))
}

/// Ask for a disc image in a file dialog until one can be used (each that cannot says why); `None` when
/// the player cancels.
fn pick_image() -> Option<DiscImage> {
    loop {
        let path = pick_disc()?;
        match open_image(&path) {
            Ok(image) => return Some(image),
            Err(e) => {
                eprintln!("error: {e}");
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("Monkey Ball")
                    .set_description(format!("{e}\n\nPlease pick another disc image."))
                    .show();
            }
        }
    }
}

/// The stored pack, if it is there and complete. One that cannot be read (cut short, another format
/// version) is removed: the image is asked for again.
fn stored_pack(pack: &Path) -> Option<(Content, String, u64)> {
    if !pack.exists() {
        return None;
    }
    match open_pack_file(pack) {
        Ok(found) => Some(found),
        Err(e) => {
            eprintln!("the stored game data cannot be used ({e}); it will be imported again");
            let _ = std::fs::remove_file(pack);
            None
        }
    }
}

/// The image to import now, opened: the one named on the command line, unless the stored pack is of the
/// same disc (it is not copied twice); with `--pick` or without a stored pack, one picked in a dialog.
/// `None`: play the stored pack.
fn image_to_import(args: &Args, stored: Option<&str>) -> Result<Option<DiscImage>, String> {
    if let Some(path) = &args.source {
        let image = open_image(path)?;
        return Ok((stored != Some(image.game_id())).then_some(image));
    }
    if !args.pick && stored.is_some() {
        return Ok(None);
    }
    match pick_image() {
        Some(image) => Ok(Some(image)),
        // `--pick` cancelled: the stored pack stays.
        None if stored.is_some() => Ok(None),
        None => Err("no disc image selected".into()),
    }
}

/// What the options screen says is kept, and where.
fn info(dir: &Path, pack_len: Option<u64>) -> Vec<String> {
    let mut lines: Vec<String> = (pack_len.into_iter())
        .map(|len| format!("Game data: {} MB", len >> 20))
        .collect();
    lines.push(format!("Kept with the saves in {}", dir.display()));
    lines
}

/// The game data and the first state. With a data directory: the stored pack and the title menu, or the
/// import screen first, on the image named on the command line or picked in a dialog. Without a data
/// directory the image is read where it is, for this run. What is kept, and that it can be removed, goes
/// into `host`; where the import screen takes images from, into `images`.
fn start(
    args: &Args,
    dir: Option<&Path>,
    menu: Box<dyn Game>,
    host: &mut Host,
    images: &mut Option<ImageSlot>,
) -> Result<(Content, Box<dyn Game>), String> {
    let Some(dir) = dir else {
        let path = (args.source.clone().or_else(pick_disc)).ok_or("no disc image selected")?;
        let content = open_disc_image(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok((content, menu));
    };
    // The options screen shows what is kept and where, and offers to remove it.
    host.caps.remove_data = true;
    let pack = dir.join(PACK_FILE);
    let stored = stored_pack(&pack);
    let Some(image) = image_to_import(args, stored.as_ref().map(|(_, id, _)| id.as_str()))? else {
        let (content, _, len) = stored.ok_or("no disc image selected")?;
        host.info = info(dir, Some(len));
        return Ok((content, menu));
    };
    // Let go of the old pack before it is replaced.
    drop(stored);
    host.info = info(dir, None);
    let data = Rc::new(LatePack::default());
    let slot = ImageSlot::default();
    slot.offer(image);
    *images = Some(slot.clone());
    let screen = ImportScreen::new(pack, data.clone(), slot, menu);
    Ok((Content::new(data), Box::new(screen)))
}

fn run() -> Result<(), String> {
    // A desktop program ends itself: the title offers "Quit".
    let mut host = Host::new(
        Caps {
            quit: true,
            ..Caps::default()
        },
        Vec::new(),
    );
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}\n");
            }
            eprintln!("{USAGE}{DEV_USAGE}");
            std::process::exit(if e.is_empty() { 0 } else { 2 });
        }
    };
    let dir = data_dir();
    let storage = open_storage(dir.as_deref());

    // The device stream opens here; without a device the game simply runs silent.
    let audio = (!args.mute).then(|| Audio::new(Box::new(mb_app::audio_cpal::CpalBackend::new())));
    let autostart = args
        .course
        .map(Autostart::Course)
        .or(args.stage.map(Autostart::Stage));
    let menu = Menu::new(storage).with_autostart(autostart);
    let menu = match &audio {
        Some(audio) => menu.with_audio(audio.clone()),
        None => menu,
    };
    let mut images = None;
    let (content, initial) = start(
        &args,
        dir.as_deref(),
        Box::new(menu),
        &mut host,
        &mut images,
    )?;
    let options = WindowOptions {
        exit_after: args.exit_after,
        fullscreen: args.fullscreen,
        audio: audio.clone(),
        images,
    };
    let stats = app::run_windowed(host, content, initial, options)?;
    if stats.remove_game_data {
        // Options -> Remove game data: the window is closed and the pack no longer open.
        if let Some(dir) = &dir {
            match std::fs::remove_file(dir.join(PACK_FILE)) {
                Ok(()) => {
                    eprintln!("game data removed; the disc image is asked for at the next start")
                }
                Err(e) => eprintln!("could not remove the game data: {e}"),
            }
        }
    }
    if args.exit_after.is_some() {
        // A timed run reports what it measured.
        println!("{}", stats.summary());
        if let Some(audio) = &audio {
            println!("{}", audio.summary());
        }
    }
    Ok(())
}

pub fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
