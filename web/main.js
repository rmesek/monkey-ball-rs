// Loader for the browser build. Plain ES module, no bundler. It gets a disc image from the file picker or a
// drop, hands it to the wasm module piece by piece and starts the game on the canvas. The image is only ever
// read with Blob.slice() inside this tab; this script performs no network request other than loading the
// engine files that sit next to it (and, for developers on localhost, the optional /dev-disc route).

import init, {
  DiscImage, start, check_header, header_len, frame_count, gpu_info,
  perf_enable, perf_reset, perf_json, perf_frames, perf_last_load,
  set_gamepad_present, set_render_options, render_quality,
} from "./pkg/mb_web.js";

// Size of the pieces the image is held in. Many small ArrayBuffers are far easier for a browser (a phone
// especially) to provide than one contiguous block of the whole image.
const CHUNK = 8 * 1024 * 1024;

const $ = (id) => document.getElementById(id);
const landing = $("landing");
const canvas = $("mb-canvas");
const pick = $("pick");
const fileInput = $("file");
const statusEl = $("status");
const progress = $("progress");
const bar = $("bar");
const params = new URLSearchParams(location.search);

let engineReady = false;
let busy = false;
let running = false;

function setStatus(text, isError = false) {
  statusEl.textContent = text;
  statusEl.classList.toggle("error", isError);
}

function setProgress(fraction) {
  progress.hidden = fraction === null;
  if (fraction !== null) {
    const pct = Math.round(fraction * 100);
    bar.firstElementChild.style.width = pct + "%";
    bar.setAttribute("aria-valuenow", String(pct));
  }
}

function describeError(e) {
  if (typeof e === "string") return e;
  if (e instanceof RangeError || (e && /memory|allocation/i.test(String(e.message)))) {
    return "Not enough memory in this browser tab to hold the disc image. Close other tabs and try again.";
  }
  if (e && e.name === "NotReadableError") {
    return "The browser could not read the file (it may have been moved or changed). Choose it again.";
  }
  return (e && e.message) || String(e);
}

// Fatal errors after the game has started (called from Rust, see crates/mb-web).
window.mbOnError = (msg) => {
  $("fatal-text").textContent = String(msg);
  $("fatal").hidden = false;
};
$("reload").addEventListener("click", () => location.reload());

// A disc source is { name, size, read(offset, length) -> Promise<Uint8Array> }.
function fileSource(file) {
  return {
    name: file.name,
    size: file.size,
    read: async (offset, length) => new Uint8Array(await file.slice(offset, offset + length).arrayBuffer()),
  };
}

// Developer aid (`xtask serve-web --disc <path>` + `?dev-disc=1`): the local dev server exposes the developer's
// own image so the page can be tested without a file dialog. Same origin, localhost only.
async function devDiscSource() {
  const head = await fetch("/dev-disc", { method: "HEAD" });
  if (!head.ok) throw new Error("the dev server has no disc (start it with: xtask serve-web --disc <path>)");
  const size = Number(head.headers.get("Content-Length"));
  return {
    name: "dev disc",
    size,
    read: async (offset, length) => {
      const r = await fetch("/dev-disc", { headers: { Range: `bytes=${offset}-${offset + length - 1}` } });
      if (r.status !== 206) throw new Error(`dev disc read failed (HTTP ${r.status})`);
      return new Uint8Array(await r.arrayBuffer());
    },
  };
}

async function readDisc(source) {
  // Refuse containers the browser build cannot read before spending time and memory on the whole file.
  check_header(await source.read(0, Math.min(header_len(), source.size)));
  const disc = new DiscImage(source.size, CHUNK);
  try {
    for (let offset = 0; offset < source.size; offset += CHUNK) {
      const length = Math.min(CHUNK, source.size - offset);
      disc.push(await source.read(offset, length));
      setProgress((offset + length) / source.size);
    }
    return disc;
  } catch (e) {
    disc.free();
    throw e;
  }
}

async function loadDisc(source) {
  if (!engineReady || busy || running) return;
  busy = true;
  pick.disabled = true;
  const t0 = performance.now();
  try {
    if (!(source.size > 0)) throw new Error("The file is empty.");
    setStatus(`Reading ${source.name} (${(source.size / 1048576).toFixed(0)} MB)...`);
    setProgress(0);
    const disc = await readDisc(source);
    const tRead = performance.now();
    setStatus("Opening the disc...");
    // Let the status paint before the synchronous open.
    await new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
    // The canvas must be laid out before the game starts: its CSS size becomes the window size.
    canvas.hidden = false;
    // `?scale=0.5` renders at half the device resolution (weak GPUs); `?autoscale=0` keeps the game from
    // lowering the resolution on its own when frames stay slow.
    const scale = Number(params.get("scale") || 1);
    set_render_options(scale > 0 ? scale : 1, params.get("autoscale") !== "0");
    let region;
    try {
      // `?gl=1` skips WebGPU (for machines where the browser's WebGPU misbehaves).
      region = start(disc, params.get("gl") === "1"); // consumes `disc`
    } catch (e) {
      canvas.hidden = true;
      throw e;
    }
    running = true;
    landing.hidden = true;
    canvas.focus();
    const tStart = performance.now();
    window.mbStats = { region, readMs: tRead - t0, openMs: tStart - tRead, bytes: source.size };
    console.info(
      `disc: Super Monkey Ball (${region}), ${source.size} bytes, read ${(tRead - t0).toFixed(0)} ms, ` +
        `open ${(tStart - tRead).toFixed(0)} ms`,
    );
    watchFrames(tStart);
  } catch (e) {
    console.error(e);
    setProgress(null);
    setStatus(describeError(e), true);
    pick.disabled = false;
  } finally {
    busy = false;
  }
}

// Frame timing, recorded inside the engine once enabled (`?fps=1`, or `mbPerf.enable()` from the console):
//   mbPerf.summary()  rolling window (about the last 1000 frames): avg / p50 / p99 / worst frame interval,
//                     update and render CPU time, sim ticks per frame, draw calls
//   mbPerf.reset()    start a fresh window        mbPerf.frames()  raw [interval, update, render, ticks] rows
//   mbPerf.log()      print the summary and the interval histogram to the console
//   mbPerf.lastLoad() where the time of the last stage load went (disc read, LZSS, parsing, textures, meshes)
window.mbPerf = {
  enable: (on = true) => perf_enable(on),
  reset: () => perf_reset(),
  summary: () => {
    const text = perf_json();
    return text ? JSON.parse(text) : null;
  },
  lastLoad: () => {
    const text = perf_last_load();
    return text ? JSON.parse(text) : null;
  },
  frames: () => {
    const flat = perf_frames();
    const rows = [];
    for (let i = 0; i + 3 < flat.length; i += 4) rows.push([flat[i], flat[i + 1], flat[i + 2], flat[i + 3]]);
    return rows;
  },
  log: () => {
    const s = window.mbPerf.summary();
    if (!s) return console.info("frame timing is off (add ?fps=1 or call mbPerf.enable())");
    console.info(perfLines(s).join("\n"));
  },
};

const HIST_LABELS = ["<=6", "<=10", "<=14", "<=20", "<=34", "<=50", ">50"];

function perfLines(s) {
  const hist = s.hist.map((n, i) => (n ? `${HIST_LABELS[i]}ms:${n}` : "")).filter(Boolean).join(" ");
  return [
    `${(1000 / s.avg).toFixed(1)} fps | frame avg ${s.avg.toFixed(2)} p99 ${s.p99.toFixed(2)} ` +
      `worst ${s.worst.toFixed(2)} ms | long ${s.long}/${s.frames}`,
    `update ${s.updateAvg.toFixed(2)} (max ${s.updateWorst.toFixed(2)}) ms | ` +
      `render ${s.renderAvg.toFixed(2)} (max ${s.renderWorst.toFixed(2)}) ms`,
    `ticks/frame 0:${s.ticks[0]} 1:${s.ticks[1]} 2:${s.ticks[2]} 3+:${s.ticks[3]} | draws ${s.draws} ` +
      `binds ${s.stateChanges} | upload ${(s.uploadBytes / 1024).toFixed(1)} KiB | wakeups ${s.wakeups.toFixed(1)}`,
    hist,
  ];
}

// Records when the first frame appears and, with ?fps=1, shows the frame timing overlay.
function watchFrames(tStart) {
  const showFps = params.get("fps") === "1";
  const fpsEl = $("fps");
  fpsEl.hidden = !showFps;
  if (showFps) perf_enable(true);
  let first = false;
  const tick = () => {
    const now = performance.now();
    if (!first && frame_count() > 0) {
      first = true;
      window.mbStats.firstFrameMs = now - tStart;
      window.mbStats.gpu = gpu_info();
      console.info(`first frame after ${(now - tStart).toFixed(0)} ms on ${gpu_info()}`);
    }
    if (showFps) {
      const s = window.mbPerf.summary();
      if (s && !s.frames) {
        // Only frames of play are recorded; a still menu draws nothing at all.
        fpsEl.textContent = "no frames of play yet | " + gpu_info();
      } else if (s) {
        window.mbStats.fps = 1000 / s.avg;
        fpsEl.textContent =
          perfLines(s).join("\n") +
          `\n${canvas.width}x${canvas.height} (scale ${render_quality().toFixed(2)}) | ${gpu_info()}`;
      }
    } else if (first) {
      return; // nothing left to report
    }
    setTimeout(tick, showFps ? 500 : 100);
  };
  tick();
}

// File picker.
pick.addEventListener("click", () => fileInput.click());
fileInput.addEventListener("change", () => {
  const file = fileInput.files && fileInput.files[0];
  fileInput.value = "";
  if (file) loadDisc(fileSource(file));
});

// Drag and drop anywhere on the page.
let dragDepth = 0;
const hasFiles = (e) => e.dataTransfer && Array.from(e.dataTransfer.types || []).includes("Files");
window.addEventListener("dragenter", (e) => {
  if (!hasFiles(e) || running) return;
  e.preventDefault();
  dragDepth++;
  document.body.classList.add("dragging");
});
window.addEventListener("dragover", (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault(); // otherwise the browser navigates to the dropped file
  e.dataTransfer.dropEffect = running ? "none" : "copy";
});
window.addEventListener("dragleave", () => {
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) document.body.classList.remove("dragging");
});
window.addEventListener("drop", (e) => {
  e.preventDefault();
  dragDepth = 0;
  document.body.classList.remove("dragging");
  const file = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
  if (file && !running) loadDisc(fileSource(file));
});

// Gamepads are polled only while one is connected.
const padsChanged = () =>
  set_gamepad_present(Array.from(navigator.getGamepads ? navigator.getGamepads() : []).some((p) => p && p.connected));
window.addEventListener("gamepadconnected", () => engineReady && padsChanged());
window.addEventListener("gamepaddisconnected", () => engineReady && padsChanged());

// A lost WebGL context takes every texture and buffer with it. preventDefault keeps the browser from giving
// up on the canvas; the engine cannot rebuild its GPU state in place, so say what happened instead of leaving a
// frozen picture. Progress is saved at every stage, so a reload continues from there.
canvas.addEventListener("webglcontextlost", (e) => {
  e.preventDefault();
  window.mbOnError(
    "The browser took the graphics context away (the device ran low on graphics memory, or the GPU was reset). " +
      "Reload the page to continue; your progress is saved.",
  );
});

// Right-drag looks around in the game; keep the context menu out of the way. Clicking gives keyboard focus.
canvas.addEventListener("contextmenu", (e) => e.preventDefault());
canvas.addEventListener("pointerdown", () => canvas.focus());

async function boot() {
  if (typeof WebAssembly !== "object") {
    setStatus("This browser does not support WebAssembly.", true);
    return;
  }
  try {
    await init();
  } catch (e) {
    console.error(e);
    setStatus(
      "The engine could not be loaded (" + describeError(e) + "). If you built this page yourself, run " +
        "scripts/build-web first so that web/pkg/ exists, and open the page through a web server.",
      true,
    );
    return;
  }
  engineReady = true;
  padsChanged();
  pick.disabled = false;
  setStatus("");
  const local = ["localhost", "127.0.0.1", "[::1]"].includes(location.hostname);
  if (local && params.get("dev-disc") === "1") {
    try {
      await loadDisc(await devDiscSource());
    } catch (e) {
      console.error(e);
      setStatus(describeError(e), true);
    }
  }
}

boot();
