//! The import screen of a native app: the files the game uses are copied out of the disc image the player
//! chose into the pack file on a thread of its own ([`write_pack_file`]), with the progress on screen; what
//! goes wrong is shown with a way back to choosing another image; once the pack is complete it is opened,
//! the result is shown for a moment, and the game starts.
//!
//! Choosing the image is the platform's (a file dialog, Android's document picker). The screen asks for
//! one with [`Command::PickImage`]; the platform opens the file ([`Importer::open`], which also checks that
//! it is the game) and hands it over through the [`ImageSlot`]. An image that cannot be opened therefore
//! never reaches this screen: the platform says why in its own way, since without an image there are no
//! fonts to say it with. The screen's fonts are read from the image itself.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use glam::Vec3;
use mb_assets::disc::DiscStream;
use mb_assets::gamepack::PackFile;
use mb_assets::pack::{PackProvider, disc_path};
use mb_render::Camera;

use crate::content::{Content, LatePack, Wanted, ui_sheet_path};
use crate::game::{Game, GameCtx, Transition};
use crate::host::Command;
use crate::import::{Importer, write_pack_file};
use crate::input::Action;
use crate::menu::wrap;
use crate::text::{Align, Color, FontId, Painter, UI_SHEETS, UiAssets, rgb};
use crate::ui::Rect;
use crate::widgets::{ListLayout, ListState, Row, draw_list, fit_scale, fit_scale_lines};

/// A disc image the platform opened for the import.
pub type DiscImage = Importer<Box<dyn DiscStream + Send>>;

/// Where the platform puts the image the player chose, for the import screen to take at its next frame.
#[derive(Clone, Default)]
pub struct ImageSlot(Rc<RefCell<Option<DiscImage>>>);

impl ImageSlot {
    pub fn offer(&self, image: DiscImage) {
        *self.0.borrow_mut() = Some(image);
    }

    fn take(&self) -> Option<DiscImage> {
        self.0.borrow_mut().take()
    }
}

/// How long the result stays on screen before the game starts (a tap or a key skips it).
const RESULT_SECONDS: f64 = 1.5;

const BACKGROUND: [f64; 4] = [0.02, 0.07, 0.24, 1.0];
const BACKDROP: Color = [0.02, 0.07, 0.24, 1.0];
const TITLE_COLOR: Color = rgb(0xffc800);
const INFO_COLOR: Color = rgb(0xd8e6ff);
const ERROR_COLOR: Color = rgb(0xff8c60);
/// An error is wrapped to lines of about this many characters, at most this many of them.
const ERROR_WIDTH: usize = 34;
const ERROR_LINES: usize = 6;

enum Report {
    Progress { done: u64, total: u64 },
    Finished(io::Result<u64>),
}

enum Phase {
    /// No image: the platform was asked for one.
    Waiting,
    Copying {
        done: u64,
        total: u64,
        reports: Receiver<Report>,
    },
    Failed(String),
    /// The pack is complete and open; the game starts at `until`.
    Done {
        bytes: u64,
        until: Option<f64>,
    },
}

pub struct ImportScreen {
    pack: PathBuf,
    data: Rc<LatePack>,
    slot: ImageSlot,
    next: Option<Box<dyn Game>>,
    phase: Phase,
    files: usize,
    ui: Option<UiAssets>,
    /// Texture count before the screen's own textures were uploaded (to free them when it is left).
    texture_base: usize,
    list: ListState,
}

/// The files of the screen's fonts, read from the image before it is handed to the copying thread.
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

impl ImportScreen {
    /// Import into the pack file `pack` from what the platform puts into `slot`, fill `data` (the game
    /// data the driver runs on) with the result, and continue with `next` (the title menu).
    pub fn new(pack: PathBuf, data: Rc<LatePack>, slot: ImageSlot, next: Box<dyn Game>) -> Self {
        Self {
            pack,
            data,
            slot,
            next: Some(next),
            phase: Phase::Waiting,
            files: 0,
            ui: None,
            texture_base: 0,
            list: ListState::default(),
        }
    }

    /// Start copying `image` on a thread of its own, after its fonts were read for this screen.
    fn start(&mut self, ctx: &mut GameCtx, mut image: DiscImage) {
        if self.ui.is_none() {
            let mut fonts = HashMap::new();
            for name in UI_SHEETS {
                let exists = |p: &str| image.paths().any(|q| q == p);
                if let Some(path) = Wanted::maybe_lz(ui_sheet_path(name)).resolve(&exists)
                    && let Ok(bytes) = image.read_file(&path)
                {
                    fonts.insert(path, bytes);
                }
            }
            let fonts = Content::new(Rc::new(Files(fonts)));
            // Without them the screen is blank but the import still runs.
            self.ui = UiAssets::load(ctx.renderer, &fonts).ok();
        }
        self.files = image.file_count();
        let (send, reports) = channel();
        let pack = self.pack.clone();
        std::thread::spawn(move || {
            let mut progress = |done, total| {
                let _ = send.send(Report::Progress { done, total });
            };
            let result = write_pack_file(image, &pack, &mut progress);
            let _ = send.send(Report::Finished(result));
        });
        self.phase = Phase::Copying {
            done: 0,
            total: 0,
            reports,
        };
    }

    /// Take what the copying thread reported since the last frame.
    fn poll(&mut self, ctx: &mut GameCtx) {
        let Phase::Copying {
            done,
            total,
            reports,
        } = &mut self.phase
        else {
            return;
        };
        let finished = loop {
            match reports.try_recv() {
                Ok(Report::Progress { done: d, total: t }) => (*done, *total) = (d, t),
                Ok(Report::Finished(result)) => break result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    break Err(io::Error::other("the import stopped"));
                }
            }
        };
        let opened = finished.and_then(|bytes| Ok((PackFile::open(&self.pack)?, bytes)));
        self.phase = match opened {
            Ok((pack, bytes)) => {
                self.data.fill(Box::new(pack));
                ctx.host.post(Command::GameDataReady { bytes });
                Phase::Done { bytes, until: None }
            }
            Err(e) => {
                self.list = ListState::default();
                Phase::Failed(e.to_string())
            }
        };
    }

    fn rows(&self, ctx: &GameCtx) -> Vec<&'static str> {
        let mut rows = vec!["CHOOSE ANOTHER IMAGE"];
        if ctx.host.caps.quit {
            rows.push("QUIT");
        }
        rows
    }

    fn draw(&self, ctx: &mut GameCtx, list: Option<(&ListLayout, &[&str])>) {
        let Some(ui) = &self.ui else {
            return;
        };
        let safe = ctx.layout.safe_rect();
        let mut p = Painter {
            r: ctx.renderer,
            ui,
        };
        let full = Rect {
            x: 0.0,
            y: 0.0,
            w: ctx.size.0 as f32,
            h: ctx.size.1 as f32,
        };
        p.rect(full, BACKDROP);
        let cx = safe.x + safe.w / 2.0;
        let (title, lines, color): (&str, Vec<String>, Color) = match &self.phase {
            Phase::Waiting => ("CHOOSE A DISC IMAGE", Vec::new(), INFO_COLOR),
            Phase::Copying { done, total, .. } => {
                let percent = done * 100 / (*total).max(1);
                let line = format!("{percent} %");
                ("IMPORTING THE GAME DATA", vec![line], INFO_COLOR)
            }
            Phase::Failed(e) => {
                let lines = wrap(&e.to_uppercase(), ERROR_WIDTH, ERROR_LINES);
                ("THE IMPORT DID NOT FINISH", lines, ERROR_COLOR)
            }
            Phase::Done { bytes, .. } => {
                let line = format!("{} FILES, {} MB", self.files, bytes >> 20);
                ("THE GAME DATA IS READY", vec![line], INFO_COLOR)
            }
        };
        let s = fit_scale(&p, FontId::Asc32, title, safe.w * 0.85, safe.h * 0.07);
        let top = if list.is_some() { 0.12 } else { 0.38 };
        p.text(
            FontId::Asc32,
            title,
            cx,
            safe.y + safe.h * top,
            s,
            s,
            TITLE_COLOR,
            Align::Center,
        );
        let line_h = safe.h * 0.055;
        let s = fit_scale_lines(
            &p,
            FontId::Asc20,
            lines.iter().map(String::as_str),
            safe.w * 0.85,
            line_h * 0.8,
        );
        for (i, line) in lines.iter().enumerate() {
            let y = safe.y + safe.h * (top + 0.1) + i as f32 * line_h;
            p.text(FontId::Asc20, line, cx, y, s, s, color, Align::Center);
        }
        if let Phase::Copying { done, total, .. } = &self.phase {
            let bar = Rect {
                x: safe.x + safe.w * 0.2,
                y: safe.y + safe.h * 0.57,
                w: safe.w * 0.6,
                h: (safe.h * 0.018).max(6.0),
            };
            p.rect(bar, [1.0, 1.0, 1.0, 0.18]);
            let share = *done as f32 / (*total).max(1) as f32;
            let filled = Rect {
                w: bar.w * share.clamp(0.0, 1.0),
                ..bar
            };
            p.rect(filled, rgb(0xffc800));
        }
        if let Some((layout, rows)) = list {
            let row = |i: usize| Row {
                label: rows[i],
                detail: None,
                enabled: true,
            };
            draw_list(&mut p, layout, &self.list, &row, false);
        }
    }
}

impl Game for ImportScreen {
    fn name(&self) -> &'static str {
        "import"
    }

    fn on_enter(&mut self, ctx: &mut GameCtx) -> Result<(), String> {
        self.texture_base = ctx.renderer.texture_count();
        ctx.renderer.clear_color = BACKGROUND;
        if self.slot.0.borrow().is_none() {
            ctx.host.post(Command::PickImage);
        }
        Ok(())
    }

    fn on_exit(&mut self, ctx: &mut GameCtx) {
        if self.ui.take().is_some() {
            ctx.renderer.truncate_textures(self.texture_base);
        }
    }

    fn update(&mut self, ctx: &mut GameCtx) -> Transition {
        ctx.input.request_virtual_controls(false);
        ctx.renderer.clear_color = BACKGROUND;
        if matches!(self.phase, Phase::Waiting)
            && let Some(image) = self.slot.take()
        {
            self.start(ctx, image);
        }
        self.poll(ctx);
        let mut list = None;
        match &mut self.phase {
            Phase::Waiting | Phase::Copying { .. } => {}
            Phase::Done { until, .. } => {
                let until = *until.get_or_insert(ctx.time + RESULT_SECONDS);
                let skip = ctx.input.pressed(Action::Confirm) || ctx.input.pointer_released();
                if (ctx.time >= until || skip)
                    && let Some(next) = self.next.take()
                {
                    return Transition::Replace(next);
                }
            }
            Phase::Failed(_) => {
                let rows = self.rows(ctx);
                let layout = ListLayout::column(&ctx.layout, rows.len(), 0.5);
                let chosen = self.list.update(ctx.input, &layout, &|_| true, ctx.dt);
                match chosen {
                    Some(0) => {
                        ctx.host.post(Command::PickImage);
                        self.phase = Phase::Waiting;
                    }
                    Some(_) => return Transition::Quit,
                    None if ctx.input.pressed(Action::Cancel) => {
                        ctx.host.post(Command::PickImage);
                        self.phase = Phase::Waiting;
                    }
                    None => list = Some((layout, rows)),
                }
            }
        }
        let list = list.as_ref().map(|(l, r)| (l, r.as_slice()));
        self.draw(ctx, list);
        Transition::None
    }

    fn camera(&self) -> Camera {
        Camera::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y)
    }

    fn status(&self) -> String {
        match &self.phase {
            Phase::Waiting => "import: waiting for an image".into(),
            Phase::Copying { done, total, .. } => format!("import: {done} of {total} bytes"),
            Phase::Failed(e) => format!("import: failed: {e}"),
            Phase::Done { bytes, .. } => format!("import: done, {bytes} bytes"),
        }
    }

    fn notice(&self) -> Option<String> {
        match &self.phase {
            Phase::Failed(e) => Some(e.clone()),
            _ => None,
        }
    }
}
