//! Browser entry point (wasm32). Everything here is gated on `target_arch = "wasm32"` so the host build of the
//! workspace is unaffected. The same module runs in two places (see `docs/WEB.md`):
//!
//! * In the **import worker** (`web/worker.js`): [`web::ImportJob`] reads the user's disc image through
//!   synchronous `Blob` slices, and either copies the files the game uses into one pack file in the Origin
//!   Private File System, or (where that storage is unavailable) serves single files for the session.
//!   The image itself is never held in memory: reads are a group or a megabyte at a time.
//! * On the **page** (`web/main.js`): [`web::GameData`] is the imported pack (a `Blob` plus its index) or
//!   the worker's live file service; [`web::start`] fetches the start-up files, builds the first game state
//!   and hands the winit event loop to the browser. While the game runs, a state that needs files prepares
//!   them (`mb_app::content`): they are fetched asynchronously into memory, parsed, and released.
//!
//! Nothing here talks to the network.

/// Web Audio backend of the game's audio system (`mb_app::audio`).
#[cfg(target_arch = "wasm32")]
mod audio;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;
    use std::io;
    use std::rc::Rc;

    use js_sys::{Array, Function, Uint8Array};
    use mb_app::content::{Content, DeferredSource, Fetch, boot_files};
    use mb_app::host;
    use mb_app::import::Importer;
    use mb_app::input::{Action, Axis, InputState};
    use mb_app::tilt::TiltSteering;
    use mb_app::ui::Insets;
    use mb_assets::disc::{DiscStream, ImageKind, SNIFF_LEN};
    use mb_assets::gamepack::{Crc32, HEADER_LEN, Header, PackIndex};
    use mb_driver::Platform;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use winit::platform::web::WindowAttributesExtWebSys;
    use winit::window::{Window, WindowAttributes};

    const CANVAS_ID: &str = "mb-canvas";
    /// Render at most this many device pixels per CSS pixel: phones report 3 and more, which costs fill rate
    /// for no visible gain at this art's resolution.
    const MAX_PIXEL_RATIO: f64 = 2.0;

    thread_local! {
        /// Frames presented since start; `web/main.js` derives the `?fps=1` readout from it.
        static FRAMES: Cell<u32> = const { Cell::new(0) };
        /// Adapter description once the GPU is up ("... (BrowserWebGpu, ...)" or "... (Gl, ...)").
        static GPU: RefCell<String> = const { RefCell::new(String::new()) };
        /// Frame timings, recorded only while the page asked for them (`?fps=1`, [`perf_enable`]).
        static PERF: RefCell<Option<mb_app::perf::PerfStats>> = const { RefCell::new(None) };
        /// A gamepad is connected (the page tells us, see `set_gamepad_present`).
        static PAD_PRESENT: Cell<bool> = const { Cell::new(false) };
        /// Render scale asked for by the page (`?scale=`) and whether it may drop on its own.
        static RENDER_OPTIONS: Cell<(f32, bool)> = const { Cell::new((1.0, true)) };
        /// Render scale in use, relative to the capped device resolution (for the overlay).
        static QUALITY: Cell<f32> = const { Cell::new(1.0) };
        /// `Game::nav_depth` of the state on top: not zero while "back" has somewhere to go in the game.
        static NAV_DEPTH: Cell<u32> = const { Cell::new(0) };
        /// The page saw a "back" gesture (history `popstate`); delivered as Cancel at the next frame.
        static BACK_PENDING: Cell<bool> = const { Cell::new(false) };
        /// The top state's status line, refreshed only after [`game_status`] was called.
        static STATUS: RefCell<(bool, String)> = const { RefCell::new((false, String::new())) };
        /// Device tilt steering, fed by [`device_orientation`].
        static TILT: RefCell<TiltSteering> = RefCell::new(TiltSteering::default());
        /// Prepared game files held in memory right now, in bytes (for `mbStats`).
        static PREPARED: RefCell<Option<Content>> = const { RefCell::new(None) };
    }

    fn js_err(e: impl std::fmt::Display) -> JsValue {
        JsValue::from_str(&e.to_string())
    }

    fn describe(e: &JsValue) -> String {
        e.as_string()
            .or_else(|| {
                e.dyn_ref::<js_sys::Error>()
                    .map(|e| format!("{}: {}", e.name(), e.message()))
            })
            .unwrap_or_else(|| format!("{e:?}"))
    }

    // -----------------------------------------------------------------------------------------------------
    // Small exports for the page.

    /// The page reports gamepads coming and going (`gamepadconnected` / `gamepaddisconnected`), so the
    /// per-frame poll, which allocates on the JS side, only runs while there is a pad to read.
    #[wasm_bindgen]
    pub fn set_gamepad_present(present: bool) {
        PAD_PRESENT.with(|p| p.set(present));
    }

    /// Rendering options, to be set before [`start`]. `scale`: render resolution relative to the (capped)
    /// device resolution, 0.25..=1. `auto`: let the game lower it when frames stay over budget.
    #[wasm_bindgen]
    pub fn set_render_options(scale: f32, auto: bool) {
        let scale = if scale.is_finite() { scale } else { 1.0 };
        RENDER_OPTIONS.with(|o| o.set((scale.clamp(0.25, 1.0), auto)));
    }

    /// Render scale currently in use (1 = the capped device resolution).
    #[wasm_bindgen]
    pub fn render_quality() -> f32 {
        QUALITY.with(Cell::get)
    }

    /// Where the time of the last stage load went, as JSON (`mb_app::stage_scene::LoadTimings`); empty before
    /// the first stage.
    #[wasm_bindgen]
    pub fn perf_last_load() -> String {
        mb_app::perf::last_load()
            .map(|t| t.to_json())
            .unwrap_or_default()
    }

    /// Start (or stop) recording frame timings for [`perf_json`]. Off by default: recording is cheap, but
    /// nothing should run that nobody reads.
    #[wasm_bindgen]
    pub fn perf_enable(on: bool) {
        PERF.with(|p| *p.borrow_mut() = on.then(mb_app::perf::PerfStats::new));
    }

    /// Forget the recorded frames (start a fresh measurement window).
    #[wasm_bindgen]
    pub fn perf_reset() {
        PERF.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.reset();
            }
        });
    }

    /// Summary of the recorded window as JSON (see `mb_app::perf::Summary`); empty while recording is off.
    #[wasm_bindgen]
    pub fn perf_json() -> String {
        PERF.with(|p| {
            p.borrow()
                .as_ref()
                .map(|p| p.summary().to_json())
                .unwrap_or_default()
        })
    }

    /// The recorded window, oldest first, four numbers per frame: interval, update and render time in
    /// milliseconds, simulation ticks.
    #[wasm_bindgen]
    pub fn perf_frames() -> Vec<f32> {
        PERF.with(|p| {
            p.borrow()
                .as_ref()
                .map(|p| {
                    p.samples()
                        .flat_map(|s| [s.interval_ms, s.update_ms, s.render_ms, s.ticks as f32])
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// What the frames of [`perf_frames`] drew, oldest first, seven numbers per frame: model draw calls,
    /// pipeline and bind switches, bytes written to GPU buffers, the mirror floor's draw calls, particle
    /// instances, particle quads, and 1 when the ball's shadow was drawn. Built when asked for, like the
    /// rows; empty while recording is off.
    #[wasm_bindgen]
    pub fn perf_counts() -> Vec<u32> {
        PERF.with(|p| {
            p.borrow()
                .as_ref()
                .map(|p| {
                    p.samples()
                        .flat_map(|s| {
                            [
                                s.draws,
                                s.state_changes,
                                s.upload_bytes,
                                s.mirror_draws,
                                s.instances,
                                s.quads,
                                u32::from(s.shadow),
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// Frames recorded since the last [`perf_reset`]. The window keeps the newest 1024 of them, so a
    /// reader that wants every frame of a longer stretch takes the new rows each time this has grown.
    #[wasm_bindgen]
    pub fn perf_total_frames() -> f64 {
        PERF.with(|p| p.borrow().as_ref().map_or(0, |p| p.total_frames)) as f64
    }

    #[wasm_bindgen(start)]
    pub fn init() {
        // Panics go to the console with a stack and to the page's error box instead of a silent freeze.
        std::panic::set_hook(Box::new(|info| {
            console_error_panic_hook::hook(info);
            call_page(
                "mbOnError",
                &[JsValue::from_str(&format!("internal error: {info}"))],
            );
        }));
    }

    /// Call `globalThis[name](..args)` if the page (or worker) defines it.
    fn call_page(name: &str, args: &[JsValue]) {
        let global = js_sys::global();
        if let Ok(f) = js_sys::Reflect::get(&global, &JsValue::from_str(name))
            && let Some(f) = f.dyn_ref::<Function>()
        {
            let list = Array::new();
            for a in args {
                list.push(a);
            }
            let _ = f.apply(&JsValue::NULL, &list);
        }
    }

    /// Frames presented so far (wraps at 2^32).
    #[wasm_bindgen]
    pub fn frame_count() -> u32 {
        FRAMES.with(Cell::get)
    }

    /// Description of the graphics adapter in use; empty until the first frame is set up.
    #[wasm_bindgen]
    pub fn gpu_info() -> String {
        GPU.with(|g| g.borrow().clone())
    }

    /// Size of the engine's linear memory in bytes (it only ever grows).
    #[wasm_bindgen]
    pub fn wasm_memory_bytes() -> f64 {
        core::arch::wasm32::memory_size(0) as f64 * 65536.0
    }

    /// Bytes of game files held in memory right now (fetched for a load and not yet released).
    #[wasm_bindgen]
    pub fn prepared_bytes() -> f64 {
        PREPARED.with(|c| c.borrow().as_ref().map_or(0, Content::prepared_bytes)) as f64
    }

    /// Files that were read without having been prepared (always empty unless the file lists have a bug).
    #[wasm_bindgen]
    pub fn content_misses() -> Vec<String> {
        PREPARED.with(|c| c.borrow().as_ref().map(Content::misses).unwrap_or_default())
    }

    /// What the page can do for the game: remove the imported data, go fullscreen, report device tilt.
    #[wasm_bindgen]
    pub fn set_caps(remove_data: bool, fullscreen: bool, tilt: bool) {
        host::set_caps(host::Caps {
            remove_data,
            fullscreen,
            tilt,
            // A page is left through the browser, not from a menu entry.
            quit: false,
        });
    }

    /// The page entered or left fullscreen.
    #[wasm_bindgen]
    pub fn set_fullscreen_state(on: bool) {
        host::set_fullscreen(on);
    }

    /// Not zero while the game has somewhere to go "back" to (a sub-menu, a stage): the page keeps one
    /// history entry then, so the browser's back gesture reaches [`nav_back`] instead of leaving the page.
    #[wasm_bindgen]
    pub fn nav_depth() -> u32 {
        NAV_DEPTH.with(Cell::get)
    }

    /// What the game is showing ("menu: title", "loading stage 3", "stage 3 ... | Playing | tick 120"), as
    /// of the frame after the previous call. For automated checks; nothing is built unless this is called.
    #[wasm_bindgen]
    pub fn game_status() -> String {
        STATUS.with(|s| {
            let mut s = s.borrow_mut();
            s.0 = true;
            s.1.clone()
        })
    }

    /// The browser's back gesture: one step back in the game (the Cancel action).
    #[wasm_bindgen]
    pub fn nav_back() {
        BACK_PENDING.with(|b| b.set(true));
    }

    /// One `deviceorientation` sample (degrees) and the screen's rotation. The single entry point for tilt.
    #[wasm_bindgen]
    pub fn device_orientation(beta: f32, gamma: f32, screen_angle: i32) {
        TILT.with(|t| t.borrow_mut().sample(beta, gamma, screen_angle));
    }

    /// Format version of the game pack this build writes and reads. The page stores it with an import and
    /// asks for the image again when it differs.
    #[wasm_bindgen]
    pub fn pack_version() -> u32 {
        mb_assets::gamepack::VERSION
    }

    /// Number of bytes from the start of the file [`check_header`] wants.
    #[wasm_bindgen]
    pub fn header_len() -> usize {
        SNIFF_LEN
    }

    /// Look at the first bytes of a file before anything else is done with it: fails with a message for
    /// the user when the file is not a supported disc image.
    #[wasm_bindgen]
    pub fn check_header(header: &[u8]) -> Result<(), JsValue> {
        ImageKind::sniff(header)
            .check_supported()
            .map_err(|e| JsValue::from_str(&e))
    }

    // -----------------------------------------------------------------------------------------------------
    // Import worker side.

    /// The user's file, read through a synchronous JS function `read(offset, length) -> Uint8Array`
    /// (`FileReaderSync` over `Blob.slice` in the worker).
    struct JsStream {
        read: Function,
        len: u64,
    }

    impl DiscStream for JsStream {
        fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
            if offset
                .checked_add(buf.len() as u64)
                .is_none_or(|end| end > self.len)
            {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            if buf.is_empty() {
                return Ok(());
            }
            let bytes = self
                .read
                .call2(
                    &JsValue::NULL,
                    &JsValue::from_f64(offset as f64),
                    &JsValue::from_f64(buf.len() as f64),
                )
                .map_err(|e| {
                    io::Error::other(format!("the file could not be read ({})", describe(&e)))
                })?;
            let bytes: Uint8Array = bytes
                .dyn_into()
                .map_err(|_| io::Error::other("the file reader returned no data"))?;
            if bytes.length() as usize != buf.len() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            bytes.copy_to(buf);
            Ok(())
        }

        fn stream_len(&mut self) -> io::Result<u64> {
            Ok(self.len)
        }
    }

    /// Messages for the user: what the reader says is already complete for unsupported files; anything else
    /// is a damaged or truncated image.
    fn image_error(e: io::Error) -> JsValue {
        JsValue::from_str(&match e.kind() {
            io::ErrorKind::Unsupported => e.to_string(),
            io::ErrorKind::UnexpectedEof => format!(
                "This disc image is incomplete (the file ends too early). {}",
                mb_assets::disc::SUPPORTED
            ),
            _ => format!("This disc image is damaged or incomplete: {e}."),
        })
    }

    /// One import of a disc image, driven by the worker a piece at a time.
    #[wasm_bindgen]
    pub struct ImportJob {
        importer: Option<Importer<JsStream>>,
    }

    #[wasm_bindgen]
    impl ImportJob {
        /// Open the image: `read(offset, length)` returns that range of the file, `size` is its length.
        /// Fails with a message for the user when the file is not a supported image of the game.
        pub fn open(read: Function, size: f64) -> Result<ImportJob, JsValue> {
            if !(1.0..=9.0e15).contains(&size) {
                return Err(JsValue::from_str(&format!(
                    "The file is empty. {}",
                    mb_assets::disc::SUPPORTED
                )));
            }
            let stream = JsStream {
                read,
                len: size as u64,
            };
            let importer = Importer::open(stream).map_err(image_error)?;
            Ok(ImportJob {
                importer: Some(importer),
            })
        }

        fn get(&self) -> Result<&Importer<JsStream>, JsValue> {
            self.importer
                .as_ref()
                .ok_or_else(|| JsValue::from_str("the import is finished"))
        }

        #[wasm_bindgen(js_name = gameId)]
        pub fn game_id(&self) -> Result<String, JsValue> {
            Ok(self.get()?.game_id().to_string())
        }

        pub fn region(&self) -> Result<String, JsValue> {
            Ok(self.get()?.region().to_string())
        }

        /// Bytes of file data the import copies in total, and so far.
        #[wasm_bindgen(js_name = totalBytes)]
        pub fn total_bytes(&self) -> Result<f64, JsValue> {
            Ok(self.get()?.total_bytes() as f64)
        }

        #[wasm_bindgen(js_name = doneBytes)]
        pub fn done_bytes(&self) -> Result<f64, JsValue> {
            Ok(self.get()?.done_bytes() as f64)
        }

        /// The disc paths the game can read after the import.
        pub fn paths(&self) -> Result<Vec<String>, JsValue> {
            Ok(self.get()?.paths().map(String::from).collect())
        }

        /// Copy the next piece (at most a megabyte): `write(bytes, offset)` stores `bytes` at `offset` of
        /// the pack. Returns false when every file is copied.
        pub fn step(&mut self, write: &Function) -> Result<bool, JsValue> {
            let importer = self
                .importer
                .as_mut()
                .ok_or_else(|| JsValue::from_str("the import is finished"))?;
            match importer.step().map_err(image_error)? {
                Some((at, bytes)) => {
                    write_piece(write, bytes, at)?;
                    Ok(true)
                }
                None => Ok(false),
            }
        }

        /// Write the index and the header, which complete the pack. Returns the pack's size in bytes.
        pub fn finish(&mut self, write: &Function) -> Result<f64, JsValue> {
            let importer = self
                .importer
                .take()
                .ok_or_else(|| JsValue::from_str("the import is finished"))?;
            let done = importer.finish().map_err(js_err)?;
            write_piece(write, &done.index, done.index_offset)?;
            write_piece(write, &done.header, 0)?;
            Ok(done.total_len as f64)
        }

        /// One file straight from the image (the session is served from the image when the pack cannot be
        /// stored).
        #[wasm_bindgen(js_name = readFile)]
        pub fn read_file(&mut self, path: &str) -> Result<Uint8Array, JsValue> {
            let importer = self
                .importer
                .as_mut()
                .ok_or_else(|| JsValue::from_str("the import is finished"))?;
            let bytes = importer.read_file(path).map_err(image_error)?;
            Ok(Uint8Array::from(bytes.as_slice()))
        }
    }

    fn write_piece(write: &Function, bytes: &[u8], at: u64) -> Result<(), JsValue> {
        // A copy on the JS side (a megabyte at most), so no view into wasm memory outlives this call.
        let piece = Uint8Array::from(bytes);
        write.call2(&JsValue::NULL, &piece, &JsValue::from_f64(at as f64))?;
        Ok(())
    }

    /// Read a stored pack back and check all of it: header, index and the checksum of every file.
    /// `read(offset, length)` is synchronous like the importer's; `size` is the stored file's length.
    #[wasm_bindgen]
    pub fn verify_pack(read: Function, size: f64) -> Result<(), JsValue> {
        let mut stream = JsStream {
            read,
            len: size.max(0.0) as u64,
        };
        let mut head = [0u8; HEADER_LEN];
        stream.read_exact_at(&mut head, 0).map_err(js_err)?;
        let header = Header::parse(&head, stream.len).map_err(js_err)?;
        let mut index = vec![0u8; header.index_len as usize];
        stream
            .read_exact_at(&mut index, header.index_offset)
            .map_err(js_err)?;
        let index = PackIndex::parse(&header, &index).map_err(js_err)?;
        let mut buf = vec![0u8; 1 << 20];
        let names: Vec<String> = index.names().map(String::from).collect();
        for name in names {
            let Some(entry) = index.get(&name) else {
                continue;
            };
            let mut crc = Crc32::default();
            let mut at = 0u64;
            while at < u64::from(entry.len) {
                let n = (u64::from(entry.len) - at).min(buf.len() as u64) as usize;
                stream
                    .read_exact_at(&mut buf[..n], entry.offset + at)
                    .map_err(js_err)?;
                crc.update(&buf[..n]);
                at += n as u64;
            }
            if crc.finish() != entry.crc {
                return Err(JsValue::from_str(&format!(
                    "{name} was not stored correctly"
                )));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------------------------
    // Page side: the game data and the game.

    enum Backing {
        /// The imported pack: a `Blob` (the file in the Origin Private File System) and its index.
        Pack {
            blob: web_sys::Blob,
            index: PackIndex,
        },
        /// No stored pack: `read(paths) -> Promise<Uint8Array[]>` asks the worker, which reads the picked
        /// image; `names` is what it can serve.
        Live {
            read: Function,
            names: HashSet<String>,
        },
    }

    /// The game's files as the page has them.
    #[wasm_bindgen]
    pub struct GameData {
        backing: Rc<Backing>,
        game_id: String,
    }

    async fn read_blob(blob: &web_sys::Blob, offset: u64, len: u64) -> Result<Vec<u8>, String> {
        let slice = blob
            .slice_with_f64_and_f64(offset as f64, (offset + len) as f64)
            .map_err(|e| describe(&e))?;
        let buffer = JsFuture::from(slice.array_buffer())
            .await
            .map_err(|e| describe(&e))?;
        let bytes = Uint8Array::new(&buffer);
        if u64::from(bytes.length()) != len {
            return Err("the stored game data is shorter than expected".into());
        }
        Ok(bytes.to_vec())
    }

    #[wasm_bindgen]
    impl GameData {
        /// Open an imported pack. Fails when it is incomplete, damaged or of another format version (the
        /// page then removes it and asks for the image again).
        #[wasm_bindgen(js_name = openPack)]
        pub async fn open_pack(blob: web_sys::Blob) -> Result<GameData, JsValue> {
            let size = blob.size() as u64;
            if size < HEADER_LEN as u64 {
                return Err(JsValue::from_str("game data is incomplete"));
            }
            let head = read_blob(&blob, 0, HEADER_LEN as u64)
                .await
                .map_err(js_err)?;
            let header = Header::parse(&head, size).map_err(js_err)?;
            let index = read_blob(&blob, header.index_offset, u64::from(header.index_len))
                .await
                .map_err(js_err)?;
            let index = PackIndex::parse(&header, &index).map_err(js_err)?;
            Ok(GameData {
                game_id: header.game_id(),
                backing: Rc::new(Backing::Pack { blob, index }),
            })
        }

        /// Game data served by the import worker from the picked image for this session.
        pub fn live(read: Function, names: Vec<String>, game_id: String) -> GameData {
            GameData {
                game_id,
                backing: Rc::new(Backing::Live {
                    read,
                    names: names.into_iter().collect(),
                }),
            }
        }

        #[wasm_bindgen(js_name = gameId)]
        pub fn game_id(&self) -> String {
            self.game_id.clone()
        }

        #[wasm_bindgen(js_name = fileCount)]
        pub fn file_count(&self) -> usize {
            match &*self.backing {
                Backing::Pack { index, .. } => index.len(),
                Backing::Live { names, .. } => names.len(),
            }
        }
    }

    impl Backing {
        fn contains(&self, path: &str) -> bool {
            match self {
                Backing::Pack { index, .. } => index.get(path).is_some(),
                Backing::Live { names, .. } => names.contains(path),
            }
        }

        /// Total size of `paths`, where it is known up front.
        fn size_of(&self, paths: &[String]) -> u64 {
            match self {
                Backing::Pack { index, .. } => paths
                    .iter()
                    .filter_map(|p| index.get(p))
                    .map(|e| u64::from(e.len))
                    .sum(),
                Backing::Live { .. } => 0,
            }
        }

        /// Fetch `paths`, reporting bytes done through `progress`.
        async fn fetch(
            &self,
            paths: &[String],
            progress: &dyn Fn(u64),
        ) -> Result<Vec<(String, Vec<u8>)>, String> {
            let mut out = Vec::with_capacity(paths.len());
            match self {
                Backing::Pack { blob, index } => {
                    let mut done = 0;
                    for path in paths {
                        let entry = index
                            .get(path)
                            .ok_or_else(|| format!("{path} is not in the game data"))?;
                        let bytes = read_blob(blob, entry.offset, u64::from(entry.len))
                            .await
                            .map_err(|e| format!("{path}: {e}"))?;
                        index.verify(path, &bytes).map_err(|e| e.to_string())?;
                        done += bytes.len() as u64;
                        progress(done);
                        out.push((path.clone(), bytes));
                    }
                }
                Backing::Live { read, .. } => {
                    let list: Array = paths.iter().map(|p| JsValue::from_str(p)).collect();
                    let promise = read
                        .call1(&JsValue::NULL, &list)
                        .map_err(|e| describe(&e))?;
                    let files = JsFuture::from(js_sys::Promise::resolve(&promise))
                        .await
                        .map_err(|e| describe(&e))?;
                    let files: Array = files
                        .dyn_into()
                        .map_err(|_| "the file service returned no list".to_string())?;
                    if files.length() as usize != paths.len() {
                        return Err("the file service returned the wrong number of files".into());
                    }
                    for (path, file) in paths.iter().zip(files.iter()) {
                        let bytes: Uint8Array =
                            file.dyn_into().map_err(|_| format!("{path}: no data"))?;
                        out.push((path.clone(), bytes.to_vec()));
                    }
                }
            }
            Ok(out)
        }
    }

    enum FetchState {
        Idle,
        Pending { done: u64, total: u64 },
        Ready(Vec<(String, Vec<u8>)>),
        Failed(String),
    }

    /// [`DeferredSource`] over [`Backing`]: a request runs as a future on the page's event loop.
    struct WebSource {
        backing: Rc<Backing>,
        /// The current request's number and state; a finished fetch only lands if it is still current.
        state: Rc<RefCell<(u32, FetchState)>>,
    }

    impl DeferredSource for WebSource {
        fn contains(&self, path: &str) -> bool {
            self.backing.contains(path)
        }

        fn request(&self, paths: &[String]) {
            let generation = {
                let mut state = self.state.borrow_mut();
                state.0 = state.0.wrapping_add(1);
                state.1 = FetchState::Pending {
                    done: 0,
                    total: self.backing.size_of(paths),
                };
                state.0
            };
            let (backing, state, paths) =
                (self.backing.clone(), self.state.clone(), paths.to_vec());
            wasm_bindgen_futures::spawn_local(async move {
                let progress = |done: u64| {
                    let mut s = state.borrow_mut();
                    if let (true, FetchState::Pending { done: d, .. }) =
                        (s.0 == generation, &mut s.1)
                    {
                        *d = done;
                    }
                };
                let result = backing.fetch(&paths, &progress).await;
                let mut s = state.borrow_mut();
                if s.0 == generation {
                    s.1 = match result {
                        Ok(files) => FetchState::Ready(files),
                        Err(e) => FetchState::Failed(e),
                    };
                }
            });
        }

        fn poll(&self) -> Fetch {
            let mut state = self.state.borrow_mut();
            match std::mem::replace(&mut state.1, FetchState::Idle) {
                FetchState::Idle => Fetch::Idle,
                FetchState::Pending { done, total } => {
                    state.1 = FetchState::Pending { done, total };
                    Fetch::Pending { done, total }
                }
                FetchState::Ready(files) => Fetch::Ready(files),
                FetchState::Failed(e) => Fetch::Failed(e),
            }
        }
    }

    /// Fetch the start-up files and start the game on the page's canvas. Resolves once the browser owns
    /// the event loop; later fatal errors are reported through `window.mbOnError`.
    /// `force_webgl`: skip WebGPU even where it is available (the page's `?gl=1`).
    #[wasm_bindgen]
    pub async fn start(data: GameData, force_webgl: bool) -> Result<(), JsValue> {
        let backing = data.backing;
        let wanted: Vec<String> = boot_files()
            .iter()
            .filter_map(|w| w.resolve(&|p| backing.contains(p)))
            .collect();
        let files = backing
            .fetch(&wanted, &|_| {})
            .await
            .map_err(|e| JsValue::from_str(&format!("The game data could not be read: {e}")))?;
        let content = Content::deferred(Rc::new(WebSource {
            backing,
            state: Rc::new(RefCell::new((0, FetchState::Idle))),
        }));
        content.insert_prepared(files);
        PREPARED.with(|c| *c.borrow_mut() = Some(content.clone()));
        let event_loop = winit::event_loop::EventLoop::new()
            .map_err(|e| JsValue::from_str(&format!("event loop: {e}")))?;
        mb_driver::spawn(
            event_loop,
            WebPlatform::new(force_webgl),
            content,
            mb_app::initial_game_with_audio(
                Box::new(mb_app::mb_game::SlotStorage::new(WebStorage::new())),
                crate::audio::create(),
            ),
        );
        Ok(())
    }

    // -----------------------------------------------------------------------------------------------------
    // Saves.

    /// Where the page keeps saves, chosen once at start-up by `web/main.js`: exactly one place, never two.
    #[derive(Clone, Copy, PartialEq, Eq, Default)]
    enum SaveMode {
        /// Files next to the game pack in the Origin Private File System. The page loads them before the
        /// game starts ([`preload_save`]) and writes them when the game saves (`window.mbSaveWrite`).
        Opfs,
        /// `localStorage` (hex text), where the private file system is unavailable.
        Local,
        /// Neither works (a page opened from disk in some browsers, blocked site data): saves live until
        /// the page closes, and every save says so.
        #[default]
        Memory,
    }

    thread_local! {
        static SAVE_MODE: Cell<SaveMode> = const { Cell::new(SaveMode::Memory) };
        /// Saves read from the private file system before start.
        static SAVE_PRELOAD: RefCell<Vec<(String, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
        /// A file write the page could not complete, to be reported by the next save.
        static SAVE_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    /// Tell the engine where saves go: `"opfs"`, `"local"` or anything else for memory only.
    #[wasm_bindgen]
    pub fn set_save_mode(mode: &str) {
        SAVE_MODE.with(|m| {
            m.set(match mode {
                "opfs" => SaveMode::Opfs,
                "local" => SaveMode::Local,
                _ => SaveMode::Memory,
            })
        });
    }

    /// Hand over one save file found in the private file system (before [`start`]).
    #[wasm_bindgen]
    pub fn preload_save(key: String, bytes: Vec<u8>) {
        SAVE_PRELOAD.with(|p| p.borrow_mut().push((key, bytes)));
    }

    /// The page could not write a save file; the next save reports it in the menu.
    #[wasm_bindgen]
    pub fn save_failed(message: String) {
        SAVE_ERROR.with(|e| *e.borrow_mut() = Some(message));
    }

    /// What the options screen says about data kept on this device (set by the page, which knows).
    #[wasm_bindgen]
    pub fn set_storage_info(lines: Vec<String>) {
        host::set_info(lines);
    }

    /// The browser side of the save store. Values arrive already crash-safe (`SlotStorage` wraps this: two
    /// checksummed slots per value), so a write that is cut short costs nothing but itself.
    struct WebStorage {
        mode: SaveMode,
        /// `Opfs`: every value (the files are only read at start). `Memory`: the session's values.
        values: std::collections::HashMap<String, Vec<u8>>,
    }

    impl WebStorage {
        const PREFIX: &'static str = "monkey-ball-rs.";

        fn new() -> Self {
            let mode = SAVE_MODE.with(Cell::get);
            let preload = SAVE_PRELOAD.with(|p| std::mem::take(&mut *p.borrow_mut()));
            Self {
                mode,
                values: if mode == SaveMode::Opfs {
                    preload.into_iter().collect()
                } else {
                    Default::default()
                },
            }
        }

        fn local() -> io::Result<web_sys::Storage> {
            web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .ok_or_else(|| io::Error::other(NO_STORAGE))
        }
    }

    const NO_STORAGE: &str =
        "this browser gives the page no storage; progress lasts until the page closes";

    impl mb_app::mb_game::Storage for WebStorage {
        fn read(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
            if self.mode != SaveMode::Local {
                return Ok(self.values.get(key).cloned());
            }
            let hex = match Self::local()?.get_item(&format!("{}{key}", Self::PREFIX)) {
                Ok(Some(hex)) => hex,
                Ok(None) => return Ok(None),
                Err(e) => return Err(io::Error::other(describe(&e))),
            };
            if hex.len() % 2 != 0 || !hex.is_ascii() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bad saved value",
                ));
            }
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .map(Some)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad saved value"))
        }

        fn write(&mut self, key: &str, data: &[u8]) -> io::Result<()> {
            match self.mode {
                SaveMode::Opfs => {
                    self.values.insert(key.to_string(), data.to_vec());
                    call_page(
                        "mbSaveWrite",
                        &[JsValue::from_str(key), Uint8Array::from(data).into()],
                    );
                    // The file write is asynchronous: a failure of an earlier one is reported now.
                    match SAVE_ERROR.with(|e| e.borrow_mut().take()) {
                        Some(e) => Err(io::Error::other(e)),
                        None => Ok(()),
                    }
                }
                SaveMode::Local => {
                    let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
                    Self::local()?
                        .set_item(&format!("{}{key}", Self::PREFIX), &hex)
                        .map_err(|e| io::Error::other(format!("browser storage: {}", describe(&e))))
                }
                SaveMode::Memory => {
                    self.values.insert(key.to_string(), data.to_vec());
                    Err(io::Error::other(NO_STORAGE))
                }
            }
        }

        fn remove(&mut self, key: &str) -> io::Result<()> {
            self.values.remove(key);
            match self.mode {
                SaveMode::Opfs => {
                    call_page("mbSaveRemove", &[JsValue::from_str(key)]);
                    Ok(())
                }
                SaveMode::Local => Self::local()?
                    .remove_item(&format!("{}{key}", Self::PREFIX))
                    .map_err(|e| io::Error::other(describe(&e))),
                SaveMode::Memory => Ok(()),
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // The platform the shared driver runs on.

    /// The left stick.
    const PAD_AXES: [Axis; 2] = [Axis::MoveX, Axis::MoveY];
    /// Standard mapping: 0 = A, 1 = B, 9 = Start.
    const PAD_BUTTONS: [(u32, Action); 3] = [
        (0, Action::Confirm),
        (1, Action::Cancel),
        (9, Action::Pause),
    ];
    const PAD_DEADZONE: f32 = 0.15;

    struct WebPlatform {
        /// What the gamepad last wrote. Axes and buttons are only written when they change, so an idle or absent
        /// pad never overwrites keyboard/touch input, and a pad that disconnects is released exactly once.
        pad_axes: [f32; 2],
        pad_buttons: [bool; 3],
        insets: Insets,
        frames: u32,
        force_webgl: bool,
        /// A pad was being read last frame (so its release is still to be delivered).
        pad_active: bool,
        /// Render scale on top of the pixel-ratio cap (`?scale=`, lowered by the automatic fallback).
        quality: f32,
        auto_quality: bool,
        quality_changed: bool,
        /// Surface pixels per CSS pixel (what `render_scale` arrived at), for the layout and the insets.
        css_ratio: f32,
        /// Frames of play looked at since the last decision, and how many of them were over budget.
        watched: u32,
        slow: u32,
        /// The Cancel injected for a "back" gesture is still held (released at the next frame).
        back_held: bool,
    }

    /// A frame this long (ms) missed 60 Hz by a clear margin.
    const SLOW_FRAME_MS: f32 = 21.0;
    /// Frames of play per quality decision (about two seconds), and the share of slow ones that triggers it.
    const QUALITY_WINDOW: u32 = 120;
    const QUALITY_SLOW_SHARE: f32 = 0.6;
    /// Each step renders about a third fewer pixels; below the floor the picture is not worth the speed.
    const QUALITY_STEP: f32 = 0.8;
    const QUALITY_FLOOR: f32 = 0.5;

    fn pixel_ratio() -> f64 {
        web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .filter(|r| r.is_finite() && *r > 0.0)
            .unwrap_or(1.0)
    }

    impl WebPlatform {
        fn new(force_webgl: bool) -> Self {
            let (quality, auto_quality) = RENDER_OPTIONS.with(Cell::get);
            QUALITY.with(|q| q.set(quality));
            Self {
                force_webgl,
                pad_active: false,
                quality,
                auto_quality,
                quality_changed: false,
                css_ratio: pixel_ratio().min(MAX_PIXEL_RATIO) as f32 * quality,
                watched: 0,
                slow: 0,
                pad_axes: [0.0; 2],
                pad_buttons: [false; 3],
                insets: Insets::default(),
                frames: 0,
                back_held: false,
            }
        }

        /// `--sai-*` custom properties are defined in index.html from `env(safe-area-inset-*)`.
        fn read_insets(scale: f32) -> Option<Insets> {
            let win = web_sys::window()?;
            let root = win.document()?.document_element()?;
            let style = win.get_computed_style(&root).ok()??;
            let px = |name: &str| -> f32 {
                style
                    .get_property_value(name)
                    .ok()
                    .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
                    .unwrap_or(0.0)
                    * scale
            };
            Some(Insets {
                left: px("--sai-left"),
                top: px("--sai-top"),
                right: px("--sai-right"),
                bottom: px("--sai-bottom"),
            })
        }

        fn insets_tick(&mut self) -> Insets {
            if self.frames.is_multiple_of(120)
                && let Some(i) = Self::read_insets(self.css_ratio)
            {
                self.insets = i;
            }
            self.insets
        }

        /// Current state of the first connected gamepad, or everything released when there is none.
        fn read_pad() -> ([f32; 2], [bool; 3]) {
            let mut axes = [0.0f32; 2];
            let mut buttons = [false; 3];
            let pad = web_sys::window()
                .and_then(|w| w.navigator().get_gamepads().ok())
                .and_then(|pads| {
                    pads.iter()
                        .filter_map(|p| p.dyn_into::<web_sys::Gamepad>().ok())
                        .find(|p| p.connected())
                });
            let Some(pad) = pad else {
                return (axes, buttons);
            };
            let pad_axes = pad.axes();
            for (i, out) in axes.iter_mut().enumerate() {
                let v = pad_axes.get(i as u32).as_f64().unwrap_or(0.0) as f32;
                // Stick up is negative on the Gamepad API; the engine's Y axes are positive forward/up.
                let v = if i % 2 == 1 { -v } else { v };
                *out = if v.abs() < PAD_DEADZONE { 0.0 } else { v };
            }
            let pad_buttons = pad.buttons();
            for (out, (index, _)) in buttons.iter_mut().zip(PAD_BUTTONS) {
                *out = pad_buttons
                    .get(index)
                    .dyn_into::<web_sys::GamepadButton>()
                    .map(|b| b.pressed())
                    .unwrap_or(false);
            }
            (axes, buttons)
        }

        /// What game states asked of the page since the last frame (`mb_app::host`). The page's side is
        /// `window.mbHost(command, argument)` in `web/main.js`.
        fn run_host_commands() {
            for command in host::drain() {
                match command {
                    host::Command::RemoveGameData => {
                        call_page("mbHost", &[JsValue::from_str("remove-data")]);
                    }
                    host::Command::SetFullscreen(on) => {
                        call_page("mbHost", &[JsValue::from_str("fullscreen"), on.into()]);
                    }
                    host::Command::Tilt { enabled, range_deg } => {
                        TILT.with(|t| t.borrow_mut().set_enabled(enabled, range_deg));
                        // The page listens to the sensor only while tilt steers.
                        call_page("mbHost", &[JsValue::from_str("tilt"), enabled.into()]);
                    }
                    host::Command::CalibrateTilt => TILT.with(|t| t.borrow_mut().calibrate()),
                }
            }
        }
    }

    impl Platform for WebPlatform {
        fn window_attributes(&mut self, attrs: WindowAttributes) -> WindowAttributes {
            let canvas = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(CANVAS_ID))
                .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
            // No explicit inner size: winit then follows the canvas' CSS size (full window, see index.html).
            attrs
                .with_canvas(canvas)
                .with_prevent_default(true)
                .with_focusable(true)
        }

        fn insets(&mut self, _window: &Window) -> Insets {
            self.insets_tick()
        }

        fn render_scale(&mut self, window: &Window) -> f32 {
            // Device pixels the canvas really has per CSS pixel. That is `devicePixelRatio` on real devices,
            // but the two can disagree (device emulation in developer tools reports the emulated ratio while
            // the canvas keeps the real pixels), and capping by the wrong one shrinks the picture twice. So
            // measure it: winit's physical size over the canvas' CSS size.
            use winit::platform::web::WindowExtWebSys;
            let css_width = window.canvas().map_or(0, |c| c.client_width());
            let ratio = if css_width > 0 {
                window.inner_size().width as f64 / css_width as f64
            } else {
                pixel_ratio()
            };
            let ratio = if ratio.is_finite() && ratio > 0.0 {
                ratio
            } else {
                1.0
            };
            self.css_ratio = ratio.min(MAX_PIXEL_RATIO) as f32 * self.quality;
            (MAX_PIXEL_RATIO / ratio).min(1.0) as f32 * self.quality
        }

        fn ui_scale(&mut self, _window: &Window, _render_scale: f32) -> f32 {
            self.css_ratio
        }

        fn pointer_position(
            &mut self,
            window: &Window,
            pos: (f32, f32),
            _render_scale: f32,
        ) -> (f32, f32) {
            // winit reports pointers in CSS pixels times `devicePixelRatio`; the surface has `css_ratio`
            // pixels per CSS pixel. Going through CSS pixels is right on real devices (where it equals
            // the default mapping) and also where the canvas has fewer real pixels than the ratio says
            // (device emulation), where the default would put every touch in the wrong place.
            let to_surface = self.css_ratio / window.scale_factor().max(0.01) as f32;
            (pos.0 * to_surface, pos.1 * to_surface)
        }

        fn render_scale_changed(&mut self) -> bool {
            std::mem::take(&mut self.quality_changed)
        }

        fn power_preference(&mut self) -> wgpu::PowerPreference {
            wgpu::PowerPreference::LowPower
        }

        fn backends(&mut self) -> wgpu::Backends {
            if self.force_webgl {
                wgpu::Backends::GL
            } else {
                wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL
            }
        }

        fn on_ready(&mut self, gpu: &str) {
            GPU.with(|g| *g.borrow_mut() = gpu.to_string());
            web_sys::console::log_1(&JsValue::from_str(&format!("graphics: {gpu}")));
        }

        fn after_update(&mut self, _window: &Window, top: &dyn mb_app::game::Game) {
            // The page keeps its history entry in step; it is told when "somewhere to go back to" starts
            // or ends, so nothing has to poll.
            let depth = top.nav_depth();
            let before = NAV_DEPTH.with(|d| d.replace(depth));
            if (before > 0) != (depth > 0) {
                call_page("mbNav", &[JsValue::from_bool(depth > 0)]);
            }
            // The status line is built only while an automated check asks for it.
            STATUS.with(|s| {
                let mut s = s.borrow_mut();
                if s.0 {
                    *s = (false, top.status());
                }
            });
        }

        fn restart_hint(&self) -> &'static str {
            "Reload the page to continue"
        }

        /// Called once per frame, which also makes it the frame counter.
        fn poll_input(&mut self, input: &mut InputState) {
            self.frames = self.frames.wrapping_add(1);
            FRAMES.with(|f| f.set(self.frames));
            Self::run_host_commands();
            TILT.with(|t| t.borrow().apply(input));
            // A "back" gesture is one press of Cancel. It is not a key: the on-screen controls stay.
            if self.back_held {
                self.back_held = false;
                input.set_action(Action::Cancel, false);
            } else if BACK_PENDING.with(|b| b.replace(false)) {
                let touch = input.touch_active();
                input.set_action(Action::Cancel, true);
                input.set_touch_active(touch);
                self.back_held = true;
            }
            // `navigator.getGamepads()` builds a fresh array of objects on every call; without a pad there is
            // nothing to read. One more read after the pad went away releases what it held.
            let present = PAD_PRESENT.with(Cell::get);
            if !(present || self.pad_active) {
                return;
            }
            self.pad_active = present;
            let (axes, buttons) = Self::read_pad();
            for (i, axis) in PAD_AXES.into_iter().enumerate() {
                if axes[i] != self.pad_axes[i] {
                    self.pad_axes[i] = axes[i];
                    input.set_axis(axis, axes[i]);
                }
            }
            for (i, (_, action)) in PAD_BUTTONS.into_iter().enumerate() {
                if buttons[i] != self.pad_buttons[i] {
                    self.pad_buttons[i] = buttons[i];
                    input.set_action(action, buttons[i]);
                }
            }
        }

        fn on_frame(&mut self, sample: &mb_app::perf::FrameSample) {
            // Automatic quality fallback: when most frames of a two-second stretch of play miss 60 Hz, render
            // fewer pixels. It only ever goes down (no oscillation) and stops at half resolution; if the
            // device is slow for another reason, the picture is at least not made worse than that.
            if self.auto_quality && self.quality > QUALITY_FLOOR {
                self.watched += 1;
                self.slow += (sample.interval_ms > SLOW_FRAME_MS) as u32;
                if self.watched >= QUALITY_WINDOW {
                    if self.slow as f32 >= QUALITY_WINDOW as f32 * QUALITY_SLOW_SHARE {
                        self.quality = (self.quality * QUALITY_STEP).max(QUALITY_FLOOR);
                        self.quality_changed = true;
                        QUALITY.with(|q| q.set(self.quality));
                        web_sys::console::info_1(&JsValue::from_str(&format!(
                            "frames are slow: render scale lowered to {:.0} %",
                            self.quality * 100.0
                        )));
                    }
                    self.watched = 0;
                    self.slow = 0;
                }
            }
            PERF.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.record(*sample);
                }
            });
        }

        fn on_error(&mut self, msg: &str) {
            web_sys::console::error_1(&JsValue::from_str(msg));
            call_page("mbOnError", &[JsValue::from_str(msg)]);
        }
    }
}
