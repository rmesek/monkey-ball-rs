//! The desktop program (Windows, Linux, macOS): one of the platform drivers on top of the game library
//! `mb-app` (see `docs/ARCHITECTURE.md`). [`app`] is the window and its event loop; this file is the command
//! line, the file dialog, and where the game data and the saves are kept.
//!
//! The executable is called `monkey-ball`. The first time it asks for the disc image and copies the files the
//! game uses out of it into one pack file beside the saves; from then on it starts from that pack and the
//! image is not needed. Options are listed in [`USAGE`].

use std::path::{Path, PathBuf};

pub mod app;

use app::WindowOptions;
use mb_app::audio::Audio;
use mb_app::content::{Content, open_disc_image, open_pack_file};
use mb_app::import::import_to_file;
use mb_app::menu::{Autostart, Menu};
use mb_assets::disc::FileStream;
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

/// Copy the files the game uses out of `image` into the pack file. Atomic: see `import_to_file`.
fn import(image: &Path, pack: &Path) -> Result<(), String> {
    eprintln!("importing {}", image.display());
    let stream = FileStream::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let mut shown = 0;
    import_to_file(stream, pack, &mut |done, total| {
        let tenth = done * 10 / total.max(1);
        if tenth > shown {
            shown = tenth;
            eprintln!("  {} %", tenth * 10);
        }
    })
    .map_err(|e| format!("{}: {e}", image.display()))?;
    Ok(())
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

/// Find the game content. With a data directory: the stored pack; else the image named on the command
/// line or picked in a dialog is imported first (the dialog comes back after an image that could not be
/// used). An image named again whose pack is already there is not copied twice. Without a data
/// directory the image is read where it is, for this run.
fn resolve_content(args: &Args, dir: Option<&Path>) -> Result<Content, String> {
    let Some(dir) = dir else {
        let path = (args.source.clone().or_else(pick_disc)).ok_or("no disc image selected")?;
        return open_disc_image(&path).map_err(|e| format!("{}: {e}", path.display()));
    };
    let pack = dir.join(PACK_FILE);
    let mut stored = stored_pack(&pack);
    if let Some(image) = &args.source {
        let id = mb_assets::disc::DiscPack::open(image)
            .map(|d| d.game_id().to_string())
            .map_err(|e| format!("{}: {e}", image.display()))?;
        if stored.as_ref().is_none_or(|(_, have, _)| *have != id) {
            // Let go of the old pack before it is replaced.
            stored = None;
            import(image, &pack)?;
        }
    } else if args.pick {
        stored = None;
    }
    let (content, _, len) = loop {
        if let Some(found) = stored.take().or_else(|| stored_pack(&pack)) {
            break found;
        }
        let image = pick_disc().ok_or("no disc image selected")?;
        if let Err(e) = import(&image, &pack) {
            eprintln!("error: {e}");
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Monkey Ball")
                .set_description(format!("{e}\n\nPlease pick another disc image."))
                .show();
        }
    };
    // The options screen shows what is kept and where, and offers to remove it.
    mb_app::host::set_info(vec![
        format!("Game data: {} MB", len >> 20),
        format!("Kept with the saves in {}", dir.display()),
    ]);
    mb_app::host::set_caps(mb_app::host::Caps {
        remove_data: true,
        ..mb_app::host::caps()
    });
    Ok(content)
}

fn run() -> Result<(), String> {
    // A desktop program ends itself: the title offers "Quit".
    mb_app::host::set_caps(mb_app::host::Caps {
        quit: true,
        ..mb_app::host::caps()
    });
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
    let content = resolve_content(&args, dir.as_deref())?;

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
    let options = WindowOptions {
        exit_after: args.exit_after,
        fullscreen: args.fullscreen,
        audio: audio.clone(),
    };
    let stats = app::run_windowed(content, Box::new(menu), options)?;
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
