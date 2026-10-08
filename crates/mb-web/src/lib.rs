//! Browser entry point (wasm32). Everything here is gated on `target_arch = "wasm32"` so the host build of the
//! workspace is unaffected.
//!
//! Flow: `web/main.js` takes a disc image from `<input type=file>` or a drop, reads the `File` in fixed-size
//! pieces (`Blob.slice().arrayBuffer()`) and hands each piece to a [`DiscImage`] as a `Uint8Array`. The pieces
//! stay where the browser put them, on the JS heap; nothing is uploaded and nothing is copied into wasm linear
//! memory up front. [`start`] then opens the disc through `mb-assets` (nod reads through [`ChunkStream`], which
//! copies just the requested ranges into wasm memory), builds the first game state and hands the winit event
//! loop to the browser (`spawn_app`).
//!
//! Why the image is read into memory at all: nod's `DiscStream` is synchronous and the game loads files
//! synchronously, while `Blob` reads are asynchronous on the main thread (`FileReaderSync` exists only in
//! workers). Why it is not one big wasm `Vec`: wasm32 linear memory is limited to 4 GiB, never shrinks, and must
//! grow as one contiguous block, which is the allocation that fails first on phones. See docs/WEB.md.

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::io;
    use std::rc::Rc;

    use js_sys::Uint8Array;
    use mb_app::content::Content;
    use mb_app::input::{Action, Axis, InputState};
    use mb_app::shell::{self, Platform};
    use mb_app::ui::Insets;
    use mb_assets::disc::{DiscPack, ImageKind, SNIFF_LEN};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use winit::platform::web::WindowAttributesExtWebSys;
    use winit::window::{Window, WindowAttributes};

    const CANVAS_ID: &str = "mb-canvas";
    /// Render at most this many device pixels per CSS pixel: phones report 3 and more, which costs fill rate
    /// for no visible gain at this art's resolution.
    const MAX_PIXEL_RATIO: f64 = 2.0;
    /// Largest piece size accepted (a `Uint8Array` view index must fit `u32`, and pieces should stay small).
    const MAX_CHUNK_LEN: f64 = (1u32 << 30) as f64;

    thread_local! {
        /// Frames presented since start; `web/main.js` derives the `?fps=1` readout from it.
        static FRAMES: Cell<u32> = const { Cell::new(0) };
        /// Adapter description once the GPU is up ("... (BrowserWebGpu, ...)" or "... (Gl, ...)").
        static GPU: RefCell<String> = const { RefCell::new(String::new()) };
    }

    #[wasm_bindgen(start)]
    pub fn init() {
        // Panics go to the console with a stack and to the page's error box instead of a silent freeze.
        std::panic::set_hook(Box::new(|info| {
            console_error_panic_hook::hook(info);
            report_to_page(&format!("internal error: {info}"));
        }));
    }

    /// `web/main.js` defines `window.mbOnError` to show a message over the canvas.
    fn report_to_page(msg: &str) {
        if let Some(win) = web_sys::window() {
            if let Ok(f) = js_sys::Reflect::get(&win, &JsValue::from_str("mbOnError")) {
                if let Some(f) = f.dyn_ref::<js_sys::Function>() {
                    let _ = f.call1(&JsValue::NULL, &JsValue::from_str(msg));
                }
            }
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

    /// Number of bytes from the start of the file [`check_header`] wants.
    #[wasm_bindgen]
    pub fn header_len() -> usize {
        SNIFF_LEN
    }

    /// Look at the first bytes of a file before it is read in full: fails with a message for the user when the
    /// container or its compression cannot be read in the browser (e.g. an LZMA2-compressed RVZ).
    #[wasm_bindgen]
    pub fn check_header(header: &[u8]) -> Result<(), JsValue> {
        ImageKind::sniff(header)
            .check_supported()
            .map_err(|e| JsValue::from_str(&e))
    }

    /// A disc image held by the page as equally sized `Uint8Array` pieces (the last one may be shorter).
    #[wasm_bindgen]
    pub struct DiscImage {
        chunks: Vec<Uint8Array>,
        chunk_len: u64,
        len: u64,
        received: u64,
    }

    #[wasm_bindgen]
    impl DiscImage {
        /// `len`: size of the image in bytes. `chunk_len`: size of every piece but the last.
        #[wasm_bindgen(constructor)]
        pub fn new(len: f64, chunk_len: f64) -> Result<DiscImage, JsValue> {
            if !((1.0..=9.0e15).contains(&len) && len.fract() == 0.0) {
                return Err(JsValue::from_str("the file is empty"));
            }
            if !((1.0..=MAX_CHUNK_LEN).contains(&chunk_len) && chunk_len.fract() == 0.0) {
                return Err(JsValue::from_str("invalid piece size"));
            }
            Ok(DiscImage {
                chunks: Vec::new(),
                chunk_len: chunk_len as u64,
                len: len as u64,
                received: 0,
            })
        }

        /// Append the next piece. The array is kept by reference (no copy); the caller must not modify it.
        pub fn push(&mut self, chunk: Uint8Array) -> Result<(), JsValue> {
            let expected = self.chunk_len.min(self.len - self.received);
            if u64::from(chunk.length()) != expected {
                return Err(JsValue::from_str(&format!(
                    "disc image piece {} has {} bytes, expected {expected}",
                    self.chunks.len(),
                    chunk.length()
                )));
            }
            self.received += expected;
            self.chunks.push(chunk);
            Ok(())
        }

        /// True once every byte has been supplied.
        pub fn complete(&self) -> bool {
            self.received == self.len
        }
    }

    /// Random-access reader over the pieces of a [`DiscImage`], for nod.
    #[derive(Clone)]
    struct ChunkStream {
        chunks: Rc<[Uint8Array]>,
        chunk_len: u64,
        len: u64,
    }

    // SAFETY: nod requires `DiscStream: Send` because its optional `threading` feature moves clones to worker
    // threads. That feature is off on wasm32 (mb-assets/Cargo.toml), and `wasm32-unknown-unknown` as built here
    // has no threads at all, so the JS handles inside never leave the thread that created them.
    unsafe impl Send for ChunkStream {}

    impl ChunkStream {
        fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
            let end = offset
                .checked_add(buf.len() as u64)
                .filter(|end| *end <= self.len)
                .ok_or(io::ErrorKind::UnexpectedEof)?;
            let mut pos = offset;
            let mut done = 0usize;
            while pos < end {
                let index = (pos / self.chunk_len) as usize;
                let within = pos % self.chunk_len;
                let n = (self.chunk_len - within).min(end - pos);
                let chunk = self.chunks.get(index).ok_or(io::ErrorKind::UnexpectedEof)?;
                // A view of the piece (no copy on the JS side), copied once into the caller's buffer.
                chunk
                    .subarray(within as u32, (within + n) as u32)
                    .copy_to(&mut buf[done..done + n as usize]);
                pos += n;
                done += n as usize;
            }
            Ok(())
        }
    }

    impl mb_assets::disc::DiscStream for ChunkStream {
        fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
            self.read_at(buf, offset)
        }

        fn stream_len(&mut self) -> io::Result<u64> {
            Ok(self.len)
        }
    }

    /// Validate the disc and start the game on the page's canvas. Returns the disc's region once the browser
    /// owns the event loop; later fatal errors are reported through `window.mbOnError`.
    /// `force_webgl`: skip WebGPU even where it is available (the page's `?gl=1`).
    #[wasm_bindgen]
    pub fn start(disc: DiscImage, force_webgl: bool) -> Result<String, JsValue> {
        if !disc.complete() {
            return Err(JsValue::from_str("the disc image was not read completely"));
        }
        let stream = ChunkStream {
            chunks: disc.chunks.into(),
            chunk_len: disc.chunk_len,
            len: disc.len,
        };
        // Unsupported containers come back as `Unsupported` with a message that is already complete.
        let pack = DiscPack::from_stream(Box::new(stream)).map_err(|e| {
            JsValue::from_str(&match e.kind() {
                io::ErrorKind::Unsupported => e.to_string(),
                _ => format!("could not read this file as a GameCube disc image: {e}"),
            })
        })?;
        let region = pack.smb1_region().ok_or_else(|| {
            JsValue::from_str(&format!(
                "this is not a Super Monkey Ball 1 disc image (disc id {})",
                pack.game_id()
            ))
        })?;
        let content = Content::from_disc(pack);
        let event_loop = winit::event_loop::EventLoop::new()
            .map_err(|e| JsValue::from_str(&format!("event loop: {e}")))?;
        shell::run(
            event_loop,
            WebPlatform::new(force_webgl),
            content,
            mb_app::initial_game(Box::new(LocalStorage::default())),
        )
        .map_err(|e| JsValue::from_str(&e))?;
        Ok(region.to_string())
    }

    /// Save data in the browser's `localStorage` (hex text, since values are opaque bytes). Where storage is
    /// unavailable (private mode, blocked site data) the session falls back to memory, so play continues and
    /// only persistence is lost.
    #[derive(Default)]
    struct LocalStorage {
        fallback: mb_app::mb_game::MemoryStorage,
    }

    impl LocalStorage {
        const PREFIX: &'static str = "monkey-ball-rs.";

        fn store() -> Option<web_sys::Storage> {
            web_sys::window()?.local_storage().ok()?
        }
    }

    impl mb_app::mb_game::Storage for LocalStorage {
        fn read(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
            let Some(store) = Self::store() else {
                return self.fallback.read(key);
            };
            let Ok(Some(hex)) = store.get_item(&format!("{}{key}", Self::PREFIX)) else {
                return self.fallback.read(key);
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
            let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
            let stored = Self::store()
                .map(|s| s.set_item(&format!("{}{key}", Self::PREFIX), &hex).is_ok())
                .unwrap_or(false);
            if stored {
                Ok(())
            } else {
                self.fallback.write(key, data)
            }
        }

        fn remove(&mut self, key: &str) -> io::Result<()> {
            if let Some(s) = Self::store() {
                let _ = s.remove_item(&format!("{}{key}", Self::PREFIX));
            }
            self.fallback.remove(key)
        }
    }

    const PAD_AXES: [Axis; 4] = [Axis::MoveX, Axis::MoveY, Axis::LookX, Axis::LookY];
    /// Standard mapping: 0 = A, 1 = B, 3 = Y, 9 = Start.
    const PAD_BUTTONS: [(u32, Action); 4] = [
        (0, Action::Confirm),
        (1, Action::Cancel),
        (3, Action::ResetCamera),
        (9, Action::Pause),
    ];
    const PAD_DEADZONE: f32 = 0.15;

    struct WebPlatform {
        /// What the gamepad last wrote. Axes and buttons are only written when they change, so an idle or absent
        /// pad never overwrites keyboard/touch input, and a pad that disconnects is released exactly once.
        pad_axes: [f32; 4],
        pad_buttons: [bool; 4],
        insets: Insets,
        frames: u32,
        force_webgl: bool,
    }

    fn pixel_ratio() -> f64 {
        web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .filter(|r| r.is_finite() && *r > 0.0)
            .unwrap_or(1.0)
    }

    impl WebPlatform {
        fn new(force_webgl: bool) -> Self {
            Self {
                force_webgl,
                pad_axes: [0.0; 4],
                pad_buttons: [false; 4],
                insets: Insets::default(),
                frames: 0,
            }
        }

        /// `--sai-*` custom properties are defined in index.html from `env(safe-area-inset-*)`.
        fn read_insets() -> Option<Insets> {
            let win = web_sys::window()?;
            let root = win.document()?.document_element()?;
            let style = win.get_computed_style(&root).ok()??;
            // Render-target pixels per CSS pixel (see `render_scale`).
            let scale = pixel_ratio().min(MAX_PIXEL_RATIO) as f32;
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
            if self.frames.is_multiple_of(120) {
                if let Some(i) = Self::read_insets() {
                    self.insets = i;
                }
            }
            self.insets
        }

        /// Current state of the first connected gamepad, or everything released when there is none.
        fn read_pad() -> ([f32; 4], [bool; 4]) {
            let mut axes = [0.0f32; 4];
            let mut buttons = [false; 4];
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

        fn render_scale(&mut self, _window: &Window) -> f32 {
            (MAX_PIXEL_RATIO / pixel_ratio()).min(1.0) as f32
        }

        fn web_backends(&mut self) -> wgpu::Backends {
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

        /// Called once per presented frame, which also makes it the frame counter.
        fn poll_input(&mut self, input: &mut InputState) {
            self.frames = self.frames.wrapping_add(1);
            FRAMES.with(|f| f.set(self.frames));
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

        fn run_local(&mut self, fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>) {
            wasm_bindgen_futures::spawn_local(fut);
        }

        fn on_error(&mut self, msg: &str) {
            web_sys::console::error_1(&JsValue::from_str(msg));
            report_to_page(msg);
        }
    }
}
