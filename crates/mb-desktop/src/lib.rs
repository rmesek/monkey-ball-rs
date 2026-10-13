//! The desktop program (Windows, Linux, macOS): one of the platform drivers on top of the game library
//! `mb-app` (see `docs/ARCHITECTURE.md`). [`app`] is the window and its event loop; this file is the command
//! line, the file dialog and where the disc image and the saves are found.
//!
//! The executable is called `mb-app`. It opens the disc image used last time, or asks for one with a file dialog; `mb-app <disc image>`
//! uses that image. The game starts at the title menu. Options are listed in [`USAGE`]; a build with the
//! `dev` feature adds the developer ones in [`DEV_USAGE`].

use std::path::{Path, PathBuf};

pub mod app;

use app::WindowOptions;
use mb_app::audio::Audio;
use mb_app::content::{Content, LAST_DISC_KEY, open_disc_image};
use mb_app::menu::{Autostart, Menu};
use mb_game::{Difficulty, FileStorage, MemoryStorage, SlotStorage, Storage};

const USAGE: &str = "usage: mb-app [disc image] [stage] [options]

The disc image is your own copy of Super Monkey Ball (Nintendo GameCube, 2001): an RVZ (uncompressed or
Zstandard) or ISO/GCM file, for example 'Super Monkey Ball (USA).rvz' (about 475 MB). Disc ids GMBE8P (USA,
the tested one), GMBP8P (Europe) and GMBJ8P (Japan) are accepted. The image is read in place: nothing is
copied, only its path is remembered. With no arguments the image used last time is reopened, or a file
dialog asks for one. A stage number starts practice on that stage directly.

  --pick                   ask for the disc image even if one is remembered
  --fullscreen             start in fullscreen (F11 or Alt+Enter toggles)
  --course <name>          start course play directly (beginner, advanced, expert, ...)
  --mute                   do not open an audio device";

#[cfg(feature = "dev")]
const DEV_USAGE: &str = "
developer options (this build has the `dev` feature):
  --autoplay <stage>       windowless scripted run: menu -> practice -> stage, played by the autopilot;
                           needs the disc image path; succeeds when the goal is reached
      --screenshot-dir <dir>  write PNGs of the menu, intro, play and goal there
      --size WxH              picture size for the run (default 1280x720)
      --software              use the software adapter
  --exit-after <seconds>   close the window after that long and print frame pacing and audio output";
#[cfg(not(feature = "dev"))]
const DEV_USAGE: &str = "";

/// What the developer options ask for; empty without the `dev` feature.
#[derive(Default)]
struct DevArgs {
    #[cfg(feature = "dev")]
    autoplay: Option<u32>,
    #[cfg(feature = "dev")]
    screenshot_dir: Option<PathBuf>,
    #[cfg(feature = "dev")]
    size: Option<(u32, u32)>,
    #[cfg(feature = "dev")]
    software: bool,
    #[cfg(feature = "dev")]
    exit_after: Option<f32>,
}

impl DevArgs {
    /// Take `option` (and its value from `value`) if it is a developer option.
    #[cfg(feature = "dev")]
    fn parse(
        &mut self,
        option: &str,
        value: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Result<bool, String> {
        match option {
            "--autoplay" => {
                let v = value("--autoplay")?;
                self.autoplay = Some(v.parse().map_err(|_| format!("bad stage number {v:?}"))?);
            }
            "--screenshot-dir" => {
                self.screenshot_dir = Some(PathBuf::from(value("--screenshot-dir")?))
            }
            "--size" => self.size = Some(parse_size(&value("--size")?)?),
            "--software" => self.software = true,
            "--exit-after" => {
                let v = value("--exit-after")?;
                self.exit_after = Some(
                    v.parse::<f32>()
                        .ok()
                        .filter(|s| s.is_finite() && *s >= 0.0)
                        .ok_or_else(|| format!("bad --exit-after {v:?}"))?,
                );
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    #[cfg(not(feature = "dev"))]
    fn parse(
        &mut self,
        _option: &str,
        _value: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Result<bool, String> {
        Ok(false)
    }

    /// The stage of a scripted run, which never touches the player's saves or the window.
    fn autoplay(&self) -> Option<u32> {
        #[cfg(feature = "dev")]
        return self.autoplay;
        #[cfg(not(feature = "dev"))]
        None
    }

    fn exit_after(&self) -> Option<f32> {
        #[cfg(feature = "dev")]
        return self.exit_after;
        #[cfg(not(feature = "dev"))]
        None
    }
}

struct Args {
    source: Option<PathBuf>,
    stage: Option<u32>,
    pick: bool,
    fullscreen: bool,
    course: Option<Difficulty>,
    mute: bool,
    dev: DevArgs,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut out = Args {
        source: None,
        stage: None,
        pick: false,
        fullscreen: false,
        course: None,
        mute: false,
        dev: DevArgs::default(),
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
            "--mute" => out.mute = true,
            "--pick" => out.pick = true,
            "--fullscreen" => out.fullscreen = true,
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => {
                if !out.dev.parse(s, &mut value)? {
                    return Err(format!("unknown option {s}"));
                }
            }
            _ => positional.push(a),
        }
    }
    let mut p = positional.into_iter().peekable();
    // A lone number is a stage for the remembered disc.
    if p.peek().is_some_and(|s| s.parse::<u32>().is_err()) {
        out.source = p.next().map(PathBuf::from);
    }
    if let Some(s) = p.next() {
        out.stage = Some(s.parse().map_err(|_| format!("bad stage number {s:?}"))?);
    }
    if p.next().is_some() {
        return Err("too many positional arguments".into());
    }
    for id in [out.stage, out.dev.autoplay()].into_iter().flatten() {
        if !mb_game::stages::is_playable_stage(id) {
            return Err(format!(
                "stage {id} is not part of the game (no course uses it)"
            ));
        }
    }
    Ok(out)
}

#[cfg(feature = "dev")]
fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("bad size {s:?}, expected WxH"))?;
    let w: u32 = w.parse().map_err(|_| format!("bad width in {s:?}"))?;
    let h: u32 = h.parse().map_err(|_| format!("bad height in {s:?}"))?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(format!("size {s:?} out of range"));
    }
    Ok((w, h))
}

/// The per-user save directory, or memory when there is none. Values are kept in two checksummed slots
/// (`SlotStorage`) written through a temp file and a rename, so killing the program at any moment leaves
/// the previous save or the new one.
fn open_storage() -> Box<dyn Storage> {
    match FileStorage::default_dir() {
        Some(dir) => {
            mb_app::host::set_info(vec![format!("Saves: {}", dir.display())]);
            Box::new(SlotStorage::new(FileStorage::new(dir)))
        }
        None => {
            eprintln!("no save directory found; progress will not be kept");
            Box::new(MemoryStorage::new())
        }
    }
}

fn remembered_disc(storage: &dyn Storage) -> Option<PathBuf> {
    let bytes = storage.read(LAST_DISC_KEY).ok()??;
    let path = PathBuf::from(String::from_utf8(bytes).ok()?.trim());
    path.exists().then_some(path)
}

fn pick_disc() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open your Super Monkey Ball (GameCube) disc image: RVZ, ISO or GCM")
        .add_filter("GameCube disc image", &["rvz", "iso", "gcm"])
        .add_filter("All files", &["*"])
        .pick_file()
}

fn open_content(path: &Path) -> Result<Content, String> {
    eprintln!("opening disc image {}", path.display());
    open_disc_image(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Find and open the game content: explicit path, else the remembered image, else a file dialog (which
/// comes back after an image that could not be opened). Remembers what worked.
fn resolve_content(args: &Args, storage: &mut dyn Storage) -> Result<Content, String> {
    if let Some(path) = &args.source {
        let content = open_content(path)?;
        remember(storage, path);
        return Ok(content);
    }
    let mut candidate = if args.pick {
        None
    } else {
        remembered_disc(storage)
    };
    loop {
        let path = match candidate.take().or_else(pick_disc) {
            Some(p) => p,
            None => return Err("no disc image selected".into()),
        };
        match open_content(&path) {
            Ok(content) => {
                remember(storage, &path);
                return Ok(content);
            }
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

fn remember(storage: &mut dyn Storage, path: &Path) {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    // The options screen shows where things are and offers to forget the image.
    let mut info = mb_app::host::info();
    info.push(format!(
        "Game image (read in place): {}",
        absolute.display()
    ));
    mb_app::host::set_info(info);
    mb_app::host::set_caps(mb_app::host::Caps {
        forget_image: true,
        ..mb_app::host::caps()
    });
    // Unchanged: nothing to write.
    let same = storage
        .read(LAST_DISC_KEY)
        .is_ok_and(|v| v.as_deref() == Some(absolute.to_string_lossy().as_bytes()));
    if same {
        return;
    }
    if let Err(e) = storage.write(LAST_DISC_KEY, absolute.to_string_lossy().as_bytes()) {
        eprintln!("could not remember the disc image: {e}");
    }
}

/// The scripted, windowless run of `--autoplay`.
#[cfg(feature = "dev")]
fn autoplay(content: &Content, stage: u32, dev: &DevArgs) -> Result<(), String> {
    use mb_app::headless::{AutoplayOptions, autoplay};
    let report = autoplay(
        content,
        &AutoplayOptions {
            stage,
            size: dev.size.unwrap_or((1280, 720)),
            software: dev.software,
            screenshot_dir: dev.screenshot_dir.clone(),
            max_frames: 60 * 150,
        },
    )?;
    println!(
        "autoplay stage {stage}: {} after {} frames ({}), goal at tick {:?}, {} banana(s), {} screenshot(s)",
        report.end_status,
        report.frames,
        report.adapter,
        report.goal_tick,
        report.bananas,
        report.screenshots.len()
    );
    for s in &report.screenshots {
        println!("  {}", s.display());
    }
    if report.goal_tick.is_some() {
        Ok(())
    } else {
        Err("the autopilot did not reach the goal".into())
    }
}

#[cfg(not(feature = "dev"))]
fn autoplay(_content: &Content, _stage: u32, _dev: &DevArgs) -> Result<(), String> {
    unreachable!("--autoplay is only parsed with the dev feature")
}

fn run() -> Result<(), String> {
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
    let scripted = args.dev.autoplay();
    if scripted.is_some() && args.source.is_none() {
        return Err("--autoplay needs the disc image path".into());
    }
    // A scripted run never reads or writes the player's own save directory.
    let mut storage: Box<dyn Storage> = if scripted.is_some() {
        Box::new(MemoryStorage::new())
    } else {
        open_storage()
    };
    let content = resolve_content(&args, storage.as_mut())?;
    if let Some(stage) = scripted {
        return autoplay(&content, stage, &args.dev);
    }

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
        exit_after: args.dev.exit_after(),
        fullscreen: args.fullscreen,
        audio: audio.clone(),
    };
    let stats = app::run_windowed(content, Box::new(menu), options)?;
    if args.dev.exit_after().is_some() {
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
