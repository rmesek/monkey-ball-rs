//! The desktop program.
//!
//! `mb-app` opens the disc image used last time, or asks for one with a file dialog; `mb-app <disc image>`
//! uses that image. The game starts at the title menu. Developer options are listed in [`USAGE`].

use std::path::{Path, PathBuf};

use glam::Vec3;
use mb_app::app::WindowOptions;
use mb_app::audio::Audio;
use mb_app::content::{open_disc_image, Content, LAST_DISC_KEY};
use mb_app::game::Game;
use mb_app::headless::{autoplay, AutoplayOptions};
use mb_app::menu::{Autostart, Menu};
use mb_app::stage_info::bg_by_name;
use mb_app::viewer::{FlyCamera, StageViewer};
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
  --viewer                 free-fly stage viewer instead of the game
  --screenshot <file>      render one frame of the stage viewer to PNG / PPM and exit
      --cam x,y,z,yaw,pitch   viewer camera (degrees)   --time <seconds>   --bg <bg_jun|...>
  --autoplay <stage>       headless scripted run: menu -> practice -> stage, played by the autopilot
      --screenshot-dir <dir>  write PNGs of the menu, intro, play and goal there
  --size WxH               size for --screenshot / --autoplay (default 1280x720)
  --software               use the software adapter for headless runs
  --mute                   do not open an audio device
  --exit-after <seconds>   close the window after that long and print frame pacing and audio output";

struct Args {
    source: Option<PathBuf>,
    stage: Option<u32>,
    pick: bool,
    fullscreen: bool,
    course: Option<Difficulty>,
    viewer: bool,
    screenshot: Option<PathBuf>,
    size: (u32, u32),
    cam: Option<FlyCamera>,
    time: f32,
    bg: Option<String>,
    software: bool,
    autoplay: Option<u32>,
    screenshot_dir: Option<PathBuf>,
    exit_after: Option<f32>,
    mute: bool,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut out = Args {
        source: None,
        stage: None,
        pick: false,
        fullscreen: false,
        course: None,
        viewer: false,
        screenshot: None,
        size: (1280, 720),
        cam: None,
        time: 0.0,
        bg: None,
        software: false,
        autoplay: None,
        screenshot_dir: None,
        exit_after: None,
        mute: false,
    };
    let mut it = args;
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--screenshot" => out.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--screenshot-dir" => {
                out.screenshot_dir = Some(PathBuf::from(value("--screenshot-dir")?))
            }
            "--size" => out.size = parse_size(&value("--size")?)?,
            "--cam" => out.cam = Some(parse_cam(&value("--cam")?)?),
            "--time" => out.time = parse_number(&value("--time")?, "--time")?,
            "--exit-after" => {
                out.exit_after = Some(parse_number(&value("--exit-after")?, "--exit-after")?)
            }
            "--bg" => out.bg = Some(value("--bg")?),
            "--autoplay" => {
                let v = value("--autoplay")?;
                out.autoplay = Some(v.parse().map_err(|_| format!("bad stage number {v:?}"))?);
            }
            "--course" => {
                let v = value("--course")?;
                out.course = Some(
                    Difficulty::from_key(&v.to_lowercase())
                        .ok_or_else(|| format!("unknown course {v:?}"))?,
                );
            }
            "--software" => out.software = true,
            "--mute" => out.mute = true,
            "--viewer" => out.viewer = true,
            "--pick" => out.pick = true,
            "--fullscreen" => out.fullscreen = true,
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
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
    // Stages no course uses have no background of their own; only the viewer opens them.
    let played = if out.viewer || out.screenshot.is_some() {
        None
    } else {
        out.stage
    };
    for id in [played, out.autoplay].into_iter().flatten() {
        if !mb_game::stages::is_playable_stage(id) {
            return Err(format!(
                "stage {id} is not part of the game (no course uses it); the stage viewer can open it: --viewer {id}"
            ));
        }
    }
    Ok(out)
}

fn parse_number(s: &str, name: &str) -> Result<f32, String> {
    s.parse::<f32>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .ok_or_else(|| format!("bad {name} {s:?}"))
}

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

fn parse_cam(s: &str) -> Result<FlyCamera, String> {
    let v: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("bad --cam {s:?}"))?;
    if v.len() != 5 {
        return Err("--cam needs x,y,z,yaw_deg,pitch_deg".into());
    }
    Ok(FlyCamera {
        pos: Vec3::new(v[0], v[1], v[2]),
        yaw: v[3].to_radians(),
        pitch: v[4].to_radians(),
    })
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

fn run() -> Result<(), String> {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}\n");
            }
            eprintln!("{USAGE}");
            std::process::exit(if e.is_empty() { 0 } else { 2 });
        }
    };
    let headless = args.screenshot.is_some() || args.autoplay.is_some();
    if headless && args.source.is_none() {
        return Err("headless runs need the disc image path".into());
    }
    // Scripted runs never read or write the player's own save directory.
    let mut storage: Box<dyn Storage> = if headless {
        Box::new(MemoryStorage::new())
    } else {
        open_storage()
    };
    let content = resolve_content(&args, storage.as_mut())?;

    if let Some(stage) = args.autoplay {
        let report = autoplay(
            &content,
            &AutoplayOptions {
                stage,
                size: args.size,
                software: args.software,
                screenshot_dir: args.screenshot_dir.clone(),
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
        return if report.goal_tick.is_some() {
            Ok(())
        } else {
            Err("the autopilot did not reach the goal".into())
        };
    }

    let viewer = || -> Result<Box<dyn Game>, String> {
        let bg = match &args.bg {
            Some(name) => {
                Some(*bg_by_name(name).ok_or_else(|| format!("unknown background {name:?}"))?)
            }
            None => None,
        };
        Ok(Box::new(
            StageViewer::new(args.stage.unwrap_or(1), bg, args.cam).with_anim_time(args.time),
        ))
    };
    if let Some(path) = &args.screenshot {
        let cap = mb_app::headless::capture(&content, viewer()?, args.size, args.software)?;
        mb_app::image::write_image(path, cap.width, cap.height, &cap.rgba)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!(
            "wrote {} ({}x{}) using {}\n{} | {} draws",
            path.display(),
            cap.width,
            cap.height,
            cap.adapter,
            cap.status,
            cap.draws
        );
        return Ok(());
    }

    // The device stream opens here; without a device the game simply runs silent.
    let audio = (!args.viewer && !args.mute)
        .then(|| Audio::new(Box::new(mb_app::audio_cpal::CpalBackend::new())));
    let initial: Box<dyn Game> = if args.viewer {
        viewer()?
    } else {
        let autostart = args
            .course
            .map(Autostart::Course)
            .or(args.stage.map(Autostart::Stage));
        let menu = Menu::new(storage).with_autostart(autostart);
        Box::new(match &audio {
            Some(audio) => menu.with_audio(audio.clone()),
            None => menu,
        })
    };
    let options = WindowOptions {
        exit_after: args.exit_after,
        fullscreen: args.fullscreen,
        audio: audio.clone(),
    };
    let stats = mb_app::app::run_windowed(content, initial, options)?;
    if args.exit_after.is_some() {
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
