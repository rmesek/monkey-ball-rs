//! Desktop entry point.
//!
//! `mb-app <disc image | content dir> [stage] [--screenshot out.png|out.ppm] [--size WxH]
//!  [--cam x,y,z,yaw_deg,pitch_deg] [--bg bg_jun] [--software]`

use std::path::PathBuf;

use glam::Vec3;
use mb_app::content::source_from_path;
use mb_app::stage_info::bg_by_name;
use mb_app::viewer::{FlyCamera, StageViewer};

const USAGE: &str =
    "usage: mb-app <disc image | content dir> [stage=1] [--screenshot out.png|out.ppm] \
[--size WxH] [--cam x,y,z,yaw_deg,pitch_deg] [--time seconds] [--bg bg_jun] [--software]";

struct Args {
    source: PathBuf,
    stage: u32,
    screenshot: Option<PathBuf>,
    size: (u32, u32),
    cam: Option<FlyCamera>,
    time: f32,
    bg: Option<String>,
    software: bool,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut out = Args {
        source: PathBuf::new(),
        stage: 1,
        screenshot: None,
        size: (1280, 720),
        cam: None,
        time: 0.0,
        bg: None,
        software: false,
    };
    let mut it = args;
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--screenshot" => out.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--size" => out.size = parse_size(&value("--size")?)?,
            "--cam" => out.cam = Some(parse_cam(&value("--cam")?)?),
            "--time" => {
                out.time = value("--time")?
                    .parse()
                    .map_err(|_| "bad --time".to_string())?
            }
            "--bg" => out.bg = Some(value("--bg")?),
            "--software" => out.software = true,
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => positional.push(a),
        }
    }
    let mut p = positional.into_iter();
    out.source = PathBuf::from(p.next().ok_or("missing disc image / content directory")?);
    if let Some(s) = p.next() {
        out.stage = s.parse().map_err(|_| format!("bad stage number {s:?}"))?;
    }
    if p.next().is_some() {
        return Err("too many positional arguments".into());
    }
    Ok(out)
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

fn run() -> Result<(), String> {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}");
            }
            eprintln!("{USAGE}");
            std::process::exit(if e.is_empty() { 0 } else { 2 });
        }
    };
    let bg = match &args.bg {
        Some(name) => {
            Some(*bg_by_name(name).ok_or_else(|| format!("unknown background {name:?}"))?)
        }
        None => None,
    };
    let source = source_from_path(&args.source);
    eprintln!("opening {}", source.describe());
    let content = source
        .open()
        .map_err(|e| format!("{}: {e}", source.describe()))?;
    let viewer = Box::new(StageViewer::new(args.stage, bg, args.cam).with_anim_time(args.time));

    if let Some(path) = &args.screenshot {
        let cap = mb_app::headless::capture(&content, viewer, args.size, args.software)?;
        mb_app::image::write_image(path, cap.width, cap.height, &cap.rgba)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!(
            "wrote {} ({}x{}) using {}
{} | {} draws",
            path.display(),
            cap.width,
            cap.height,
            cap.adapter,
            cap.status,
            cap.draws
        );
        Ok(())
    } else {
        mb_app::app::run_windowed(content, viewer)
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
