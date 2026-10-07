//! Browser entry point (wasm32). Everything here is gated on `target_arch = "wasm32"` so the host build of the
//! workspace is unaffected.
//!
//! Flow: `index.html` asks for a disc image through `<input type=file>`, slices the `File` into 16 MiB chunks and
//! feeds them into a [`DiscBuffer`] (no upload anywhere; the bytes only ever live in this tab's wasm memory),
//! then calls [`start`], which opens the disc through `mb-assets`, builds the first game state and hands the
//! winit event loop to the browser (`spawn_app`).
//!
//! Why the whole image is held in memory: nod's `DiscStream` is synchronous, while `Blob` reads are async on the
//! main thread (`FileReaderSync` exists only in workers). Holding the RVZ/ISO (about 0.5 GB for an uncompressed
//! RVZ, 1.46 GB for a full ISO) keeps the engine identical to desktop. See docs/WEB.md for the trade-offs and the
//! worker-based alternative.

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::Arc;

    use mb_app::content::Content;
    use mb_app::game::Game;
    use mb_app::input::{Action, Axis, InputState};
    use mb_app::shell::{self, Platform};
    use mb_app::ui::Insets;
    use mb_assets::disc::DiscPack;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use winit::platform::web::WindowAttributesExtWebSys;
    use winit::window::{Window, WindowAttributes};

    const CANVAS_ID: &str = "mb-canvas";

    #[wasm_bindgen(start)]
    pub fn init() {
        console_error_panic_hook::set_once();
    }

    /// A disc image being assembled chunk by chunk from JS.
    #[wasm_bindgen]
    pub struct DiscBuffer {
        data: Vec<u8>,
    }

    #[wasm_bindgen]
    impl DiscBuffer {
        /// Allocate `len` bytes up front (fails with a readable message instead of aborting the tab).
        #[wasm_bindgen(constructor)]
        pub fn new(len: f64) -> Result<DiscBuffer, JsValue> {
            let len = len as usize;
            let mut data = Vec::new();
            data.try_reserve_exact(len).map_err(|_| {
                JsValue::from_str(&format!(
                    "not enough memory for a {} MiB disc image in this browser tab",
                    len >> 20
                ))
            })?;
            data.resize(len, 0);
            Ok(DiscBuffer { data })
        }

        pub fn write_chunk(&mut self, offset: f64, chunk: &[u8]) -> Result<(), JsValue> {
            let offset = offset as usize;
            let end = offset + chunk.len();
            if end > self.data.len() {
                return Err(JsValue::from_str("chunk past the end of the disc image"));
            }
            self.data[offset..end].copy_from_slice(chunk);
            Ok(())
        }
    }

    /// First game state. Single place to switch to the main menu once it lands.
    fn initial_game() -> Box<dyn Game> {
        Box::new(mb_app::viewer::StageViewer::new(1, None, None))
    }

    /// Validate the disc and start the game on the page's canvas. Returns once the browser owns the event loop.
    #[wasm_bindgen]
    pub fn start(disc: DiscBuffer) -> Result<String, JsValue> {
        let pack = DiscPack::from_vec(disc.data)
            .map_err(|e| JsValue::from_str(&format!("could not read the disc image: {e}")))?;
        let region = pack.smb1_region().ok_or_else(|| {
            JsValue::from_str(&format!(
                "this is not a Super Monkey Ball 1 disc image (disc id {})",
                pack.game_id()
            ))
        })?;
        let content = Content::new(Arc::new(pack));
        let event_loop = winit::event_loop::EventLoop::new()
            .map_err(|e| JsValue::from_str(&format!("event loop: {e}")))?;
        shell::run(event_loop, WebPlatform::new(), content, initial_game())
            .map_err(|e| JsValue::from_str(&e))?;
        Ok(region.to_string())
    }

    struct WebPlatform {
        pad_buttons: [bool; 4],
        insets: Insets,
        frames: u32,
    }

    impl WebPlatform {
        fn new() -> Self {
            Self {
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
            let dpr = win.device_pixel_ratio() as f32;
            let px = |name: &str| -> f32 {
                style
                    .get_property_value(name)
                    .ok()
                    .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
                    .unwrap_or(0.0)
                    * dpr
            };
            Some(Insets {
                left: px("--sai-left"),
                top: px("--sai-top"),
                right: px("--sai-right"),
                bottom: px("--sai-bottom"),
            })
        }
    }

    impl Platform for WebPlatform {
        fn window_attributes(&mut self, attrs: WindowAttributes) -> WindowAttributes {
            let canvas = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(CANVAS_ID))
                .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
            // No explicit inner size: winit then follows the canvas' CSS size (full window, see index.html).
            attrs.with_canvas(canvas).with_prevent_default(true)
        }

        fn insets(&mut self, _window: &Window) -> Insets {
            self.insets_tick()
        }

        fn poll_input(&mut self, input: &mut InputState) {
            let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
                return;
            };
            let Ok(pads) = nav.get_gamepads() else {
                return;
            };
            let pad = pads
                .iter()
                .filter_map(|p| p.dyn_into::<web_sys::Gamepad>().ok())
                .find(|p| p.connected());
            let Some(pad) = pad else {
                return;
            };
            let axis = |i: u32| -> f32 {
                let v = pad.axes().get(i).as_f64().unwrap_or(0.0) as f32;
                if v.abs() < 0.15 {
                    0.0
                } else {
                    v
                }
            };
            // Stick up is negative on the Gamepad API; the engine's MoveY is positive forward.
            input.set_axis(Axis::MoveX, axis(0));
            input.set_axis(Axis::MoveY, -axis(1));
            input.set_axis(Axis::LookX, axis(2));
            input.set_axis(Axis::LookY, -axis(3));
            // Standard mapping: 0 = A, 1 = B, 3 = Y, 9 = Start.
            let map = [
                (0u32, Action::Confirm),
                (1, Action::Cancel),
                (3, Action::ResetCamera),
                (9, Action::Pause),
            ];
            for (slot, (idx, action)) in map.iter().enumerate() {
                let down = pad
                    .buttons()
                    .get(*idx)
                    .dyn_into::<web_sys::GamepadButton>()
                    .map(|b| b.pressed())
                    .unwrap_or(false);
                if down != self.pad_buttons[slot] {
                    self.pad_buttons[slot] = down;
                    input.set_action(*action, down);
                }
            }
        }

        fn run_local(&mut self, fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>) {
            wasm_bindgen_futures::spawn_local(fut);
        }

        fn on_error(&mut self, msg: &str) {
            web_sys::console::error_1(&JsValue::from_str(msg));
            // index.html defines window.mbOnError to show the message over the canvas.
            if let Some(win) = web_sys::window() {
                if let Ok(f) = js_sys::Reflect::get(&win, &JsValue::from_str("mbOnError")) {
                    if let Some(f) = f.dyn_ref::<js_sys::Function>() {
                        let _ = f.call1(&JsValue::NULL, &JsValue::from_str(msg));
                    }
                }
            }
        }
    }

    impl WebPlatform {
        fn insets_tick(&mut self) -> Insets {
            if self.frames % 120 == 0 {
                if let Some(i) = Self::read_insets() {
                    self.insets = i;
                }
            }
            self.frames = self.frames.wrapping_add(1);
            self.insets
        }
    }
}
