// End-to-end check of the browser build, in the installed Chrome, through the real file picker.
//
//   deno run -A scripts/web-e2e.ts [--keep] [--lan]
//
// It serves target/site with `python -m http.server` (the plain static server the bundle is meant for), starts
// Chrome with a throw-away profile, and drives it over the DevTools protocol: the disc image is set on the
// page's own <input type=file>, exactly as a user would pick it. No npm packages, nothing downloaded.
// Needs: the bundle (cargo run --release -p xtask -- build-web), Chrome, Python 3, and your disc image (MB_DISC, or
// "Super Monkey Ball*.rvz|iso|gcm" next to the repository). Missing any of them it says so and exits 0.
//
//   --lan   also open the page through this machine's network address (plain http, not a secure context):
//           the server then listens on all interfaces for the length of the run.
//   --keep  leave the browser open at the end.

const root = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const dist = `${root}target/site`;
const flags = new Set(Deno.args);
const HTTP = 8791 + Math.floor(Math.random() * 100);
const CDP = 9400 + Math.floor(Math.random() * 100);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function skip(why: string): never {
  console.log(`SKIPPED: ${why}`);
  Deno.exit(0);
}
const exists = (p: string) => { try { Deno.statSync(p); return true; } catch { return false; } };

// ---- prerequisites ----------------------------------------------------------------------------------------
if (!exists(`${dist}/index.html`)) skip("target/site is not built (run: cargo run --release -p xtask -- build-web)");
function findDisc(): string | null {
  const env = Deno.env.get("MB_DISC");
  if (env) return exists(env) ? env : null;
  const parent = `${root}..`;
  for (const e of Deno.readDirSync(parent)) {
    const n = e.name.toLowerCase();
    if (n.startsWith("super monkey ball") && /\.(rvz|iso|gcm)$/.test(n)) return `${parent}/${e.name}`;
  }
  return null;
}
const disc = findDisc();
if (!disc) skip("no disc image (set MB_DISC or put 'Super Monkey Ball (USA).rvz' next to the repository)");
const discPath = Deno.realPathSync(disc);
function findChrome(): string | null {
  const candidates = [
    Deno.env.get("CHROME") ?? "",
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium", "/usr/bin/chromium-browser",
  ];
  return candidates.find((c) => c && exists(c)) ?? null;
}
const chromePath = findChrome();
if (!chromePath) skip("Chrome was not found (set CHROME to its executable)");
const python = ["python", "python3"].find((p) => {
  try { return new Deno.Command(p, { args: ["--version"], stdout: "null", stderr: "null" }).outputSync().success; } catch { return false; }
});
if (!python) skip("Python 3 was not found (the check serves the bundle with python -m http.server)");

// ---- server and browser -----------------------------------------------------------------------------------
// A short path: Chrome's cache storage breaks under very long profile paths on Windows.
const scratch = Deno.makeTempDirSync({ prefix: "mbe2e" });
const bind = flags.has("--lan") ? "0.0.0.0" : "127.0.0.1";
let server = new Deno.Command(python, { args: ["-m", "http.server", String(HTTP), "--bind", bind, "--directory", dist], stdout: "null", stderr: "null" }).spawn();
const chrome = new Deno.Command(chromePath, {
  args: [`--remote-debugging-port=${CDP}`, `--user-data-dir=${scratch}/profile`, "--no-first-run", "--no-default-browser-check",
    "--window-size=1100,860", "--disable-backgrounding-occluded-windows", "--disable-renderer-backgrounding",
    "--disable-background-timer-throttling", "--disable-features=CalculateNativeWinOcclusion", "--autoplay-policy=no-user-gesture-required",
    "about:blank"],
  stdout: "null", stderr: "null",
}).spawn();

let failures = 0;
const results: string[] = [];
function check(name: string, ok: boolean, detail = "") {
  if (!ok) failures++;
  const line = `${ok ? "ok  " : "FAIL"} ${name}${detail ? "  (" + detail + ")" : ""}`;
  results.push(line);
  console.log(line);
}

async function cleanup() {
  if (!flags.has("--keep")) { try { chrome.kill(); } catch { /* gone */ } }
  try { server.kill(); } catch { /* gone */ }
  await sleep(800);
  if (!flags.has("--keep")) { try { Deno.removeSync(scratch, { recursive: true }); } catch { /* Chrome still holds a file */ } }
}

// One DevTools session on a page target.
class Page {
  ws!: WebSocket; id = 0; pending = new Map<number, (v: any) => void>(); problems: string[] = []; targetId = "";
  // What the browser itself says about graphics (a WebGL call it refused, a shader it could not compile, a
  // lost context): these are log entries, not console calls of the page.
  gfx: string[] = [];
  static async open(targetId?: string): Promise<Page> {
    const p = new Page();
    let target: any;
    for (let i = 0; i < 50; i++) {
      try {
        const list = await (await fetch(`http://127.0.0.1:${CDP}/json`)).json();
        target = targetId ? list.find((t: any) => t.id === targetId) : list.find((t: any) => t.type === "page");
        if (target) break;
      } catch { /* not up yet */ }
      await sleep(200);
    }
    if (!target) throw new Error("Chrome did not open a page");
    p.targetId = target.id;
    p.ws = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((r) => (p.ws.onopen = r));
    p.ws.onmessage = (e) => {
      const m = JSON.parse(e.data as string);
      if (m.id && p.pending.has(m.id)) { p.pending.get(m.id)!(m); p.pending.delete(m.id); }
      if (m.method === "Runtime.exceptionThrown") p.problems.push(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text);
      if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") p.problems.push(m.params.args.map((a: any) => a.value ?? a.description ?? "").join(" ").slice(0, 300));
      if (m.method === "Log.entryAdded" && /error|warning/.test(m.params.entry.level) && (m.params.entry.source === "rendering" || /WebGL|WebGPU|WGSL|GLSL|shader|GL_|context|fence|sync|finish/i.test(m.params.entry.text))) p.gfx.push(String(m.params.entry.text).slice(0, 300));
    };
    await p.send("Log.enable");
    await p.send("Runtime.enable");
    await p.send("Page.enable");
    return p;
  }
  send(method: string, params: any = {}): Promise<any> {
    return new Promise((resolve) => { const i = ++this.id; this.pending.set(i, resolve); this.ws.send(JSON.stringify({ id: i, method, params })); });
  }
  async ev(expression: string): Promise<any> {
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (r.result?.exceptionDetails) return undefined;
    return r.result?.result?.value;
  }
  async wait(expression: string, ms = 60000): Promise<boolean> {
    const t0 = Date.now();
    while (Date.now() - t0 < ms) { if (await this.ev(expression)) return true; await sleep(100); }
    return false;
  }
  async nav(url: string) { await this.send("Page.navigate", { url }); await sleep(300); }
  // Run `source` in every new document of this page before its own scripts (to take browser features away).
  async inject(source: string): Promise<string> { return (await this.send("Page.addScriptToEvaluateOnNewDocument", { source })).result.identifier; }
  async uninject(identifier: string) { await this.send("Page.removeScriptToEvaluateOnNewDocument", { identifier }); }
  async pick(path: string) {
    const doc = await this.send("DOM.getDocument", {});
    const node = await this.send("DOM.querySelector", { nodeId: doc.result.root.nodeId, selector: "#file" });
    await this.send("DOM.setFileInputFiles", { files: [path], nodeId: node.result.nodeId });
  }
  async key(code: string, hold = 60) {
    const map: Record<string, [string, number]> = { Enter: ["Enter", 13], Escape: ["Escape", 27], KeyW: ["w", 87], ArrowUp: ["ArrowUp", 38], ArrowDown: ["ArrowDown", 40] };
    const [k, vk] = map[code];
    const p = { code, key: k, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk };
    await this.send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...p });
    await sleep(hold);
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", ...p });
    await sleep(140);
  }
  async touch(type: string, x?: number, y?: number) {
    await this.send("Input.dispatchTouchEvent", { type, touchPoints: x === undefined ? [] : [{ x, y, id: 1, radiusX: 8, radiusY: 8, force: 1 }] });
  }
  async tap(x: number, y: number, hold = 80) { await this.touch("touchStart", x, y); await sleep(hold); await this.touch("touchEnd"); await sleep(200); }
  async swipe(x: number, y1: number, y2: number) {
    await this.touch("touchStart", x, y1);
    for (let i = 1; i <= 8; i++) { await sleep(15); await this.touch("touchMove", x, y1 + ((y2 - y1) * i) / 8); }
    await this.touch("touchEnd"); await sleep(120);
  }
  // The game's own status line (one frame old on the first call, so ask twice).
  async status(): Promise<string> { await this.ev("mbPerf.status()"); await sleep(120); return (await this.ev("mbPerf.status()")) ?? ""; }
  async waitStatus(test: (s: string) => boolean, ms = 30000): Promise<string> {
    const t0 = Date.now(); let s = "";
    while (Date.now() - t0 < ms) { s = await this.status(); if (test(s)) return s; await sleep(100); }
    return s;
  }
  close() { try { this.ws.close(); } catch { /* closed */ } }
}

const base = `http://localhost:${HTTP}/`;
const landingText = "document.getElementById('status').textContent";
const started = "window.mbStats && mbStats.firstFrameMs > 0";
const pickerReady = "!document.getElementById('pick').disabled";
const stored = `navigator.storage.getDirectory().then(async r => { const out = []; try { const d = await r.getDirectoryHandle('monkey-ball'); for await (const [n, h] of d.entries()) if (h.kind === 'file') out.push(n + ':' + (await h.getFile()).size); } catch (e) {} return out.sort().join(' '); })`;

async function playToFloorTwo(p: Page, label: string, hold = false) {
  await p.send("Page.bringToFront");
  await p.ev("document.getElementById('mb-canvas').focus()");
  await p.key("Enter"); await sleep(300); await p.key("Enter");
  const s1 = await p.waitStatus((s) => s.startsWith("stage 1 "), 20000);
  check(`${label}: floor 1 loads and runs`, s1.startsWith("stage 1 "), s1);
  // Hold "forward". The key is pressed again every two seconds: a press that arrives while the page is
  // still taking focus is lost, and a repeated press of a held key changes nothing.
  let s2 = "";
  for (let i = 0; i < 20 && !s2.startsWith("stage 2 "); i++) {
    await p.ev("document.getElementById('mb-canvas').focus()");
    await p.send("Input.dispatchKeyEvent", { type: "rawKeyDown", code: "KeyW", key: "w", windowsVirtualKeyCode: 87, nativeVirtualKeyCode: 87 });
    s2 = await p.waitStatus((s) => s.startsWith("stage 2 "), 2000);
  }
  check(`${label}: through the goal into floor 2`, s2.startsWith("stage 2 "), s2);
  if (!hold) await p.send("Input.dispatchKeyEvent", { type: "keyUp", code: "KeyW", key: "w", windowsVirtualKeyCode: 87, nativeVirtualKeyCode: 87 });
  return s2.startsWith("stage 2 ");
}

// ---- measuring: frame times, GPU work per frame, wasm memory (docs/WEB.md, "Measurements") -------------------
// The budgets: what version 0.1.0 measured on this set-up (docs/WEB.md) plus the allowances below.
const V010 = {
  beginnerMb: 23, expertMb: 28, // wasm memory after floors 1 and 2 of Beginner, and with Expert 50 loaded later
  beginner: { updateMs: 0.16, renderMs: 5.1, draws: 275 }, // Beginner 1 standing at the start, WebGL2, 6x CPU throttle
  expert: { long: 2, frames: 4917, p99: 6.6, displayMs: 6.06 }, // Expert 50, 30 s of steering, the same, on a 165 Hz display
  // wasm memory of a page that loaded nothing but that stage, six seconds into play: a build of the v0.1.0
  // tag, measured on 2026-10-13 with this script's steps (two runs; the lower where they differed).
  freshMb: { 89: 26.1, 76: 22.2, 21: 28.1 } as Record<number, number>,
};
const MEMORY_BUDGET_MB = 2; // wasm memory may grow by this much over 0.1.0
const SLOWER = 1.5; // a time fails beyond this factor of 0.1.0's, so that a busy machine does not fail it
const FRAME_BUDGET_MS = 16.6; // one frame at 60 Hz
const CPU_THROTTLE = 6; // a desktop CPU slowed to about a mid-range phone's (the factor 0.1.0 was measured with)
const PARTICLE_DRAWS = 12; // what the particles may add to a frame (crates/mb-app/tests/budgets.rs)
const MB = 1048576;

const wasmMb = async (p: Page) => ((await p.ev("mbPerf.wasmMemoryBytes()")) ?? 0) / MB;
const throttle = (p: Page, rate: number) => p.send("Emulation.setCPUThrottlingRate", { rate });
const KEYS: Record<string, [string, number]> = { Enter: ["Enter", 13], Escape: ["Escape", 27], ArrowUp: ["ArrowUp", 38], ArrowDown: ["ArrowDown", 40], KeyW: ["w", 87], KeyD: ["d", 68], KeyS: ["s", 83], KeyA: ["a", 65] };
const keyEvent = (p: Page, type: string, code: string) => p.send("Input.dispatchKeyEvent", { type, code, key: KEYS[code][0], windowsVirtualKeyCode: KEYS[code][1], nativeVirtualKeyCode: KEYS[code][1] });
// Menu keys, quicker than Page.key: a stage list is up to 50 rows long.
async function press(p: Page, code: string, times = 1) {
  for (let i = 0; i < times; i++) { await keyEvent(p, "rawKeyDown", code); await sleep(45); await keyEvent(p, "keyUp", code); await sleep(75); }
}

// Load the page at `url` with the game data already on the device: the title menu, keys going to the game,
// frame timings recorded from the first frame of play.
async function title(p: Page, url: string) {
  await p.nav(url);
  if (!(await p.wait(started, 30000))) throw new Error(`the game did not start at ${url}`);
  await p.send("Page.bringToFront");
  await p.ev("document.getElementById('mb-canvas').focus(); mbPerf.enable()");
  await sleep(200);
}
// Rows of the practice menus: the course, then the stage in its list (course order, bonus floors included).
const STAGE = {
  1: [0, 0], // Beginner 1
  92: [1, 9], // the bonus floor whose floor is a wave, and a mirror
  21: [1, 10], // night: window flipbooks, and a strip of mirror
  76: [2, 35], // a mirror floor, second in draw calls
  81: [2, 40], // storm: fire flipbooks
  89: [2, 48], // the stage with the most draw calls, and a mirror
  90: [2, 49], // Expert 50
} as const;
type StageId = keyof typeof STAGE;
// A fresh page, then Practice -> course -> stage. Needs "Unlock all stages". Returns once the stage is up.
async function practice(p: Page, url: string, id: StageId) {
  await title(p, url);
  await press(p, "ArrowDown"); await press(p, "Enter");
  await press(p, "ArrowDown", STAGE[id][0]); await press(p, "Enter");
  await press(p, "ArrowDown", STAGE[id][1]); await press(p, "Enter");
  const s = await p.waitStatus((x) => x.startsWith(`stage ${id} `), 20000);
  if (!s.startsWith(`stage ${id} `)) throw new Error(`stage ${id} did not start (${s})`);
}
const inPlay = async (p: Page) => / Playing /.test(await p.waitStatus((x) => / Playing /.test(x), 20000));

// Options -> the row at `row`, by keys, on a page loaded without touch or tilt: "Unlock all stages" is row 0,
// "Reflections" row 5 (after the stick curve, the two volumes and Fullscreen).
const UNLOCK_ROW = 0, REFLECTIONS_ROW = 5;
async function chooseOption(p: Page, url: string, row: number) {
  await title(p, url);
  await press(p, "ArrowDown", 2); await press(p, "Enter");
  if ((await p.status()) !== "menu: options") throw new Error(`Options did not open (${await p.status()})`);
  await press(p, "ArrowDown", row); await press(p, "Enter");
  await sleep(500); // the page writes the save file asynchronously
  await press(p, "Escape");
}

// One frame of play as the engine recorded it: milliseconds since the frame before, update and render time,
// simulation ticks, then model draw calls, pipeline and bind switches, bytes written to GPU buffers, the
// mirror floor's draw calls, particle instances, particle quads, 1 when the ball's shadow was drawn.
type Frame = [number, number, number, number, number, number, number, number, number, number, number];
type Run = {
  frames: number; p50: number; p99: number; worst: number; long: number; update: number; render: number; work99: number;
  draws: number; drawsMax: number; binds: number; upload: number; uploadMax: number;
  mirror: number; mirrorMax: number; instancesMax: number; quadsMax: number; shadowed: number; // `shadowed`: frames with the shadow
};
function summarize(rows: Frame[]): Run {
  const n = rows.length;
  const sorted = (f: (r: Frame) => number) => rows.map(f).sort((a, b) => a - b);
  const at = (v: number[], q: number) => (v.length ? v[Math.min(v.length - 1, Math.floor(v.length * q))] : 0);
  const mean = (f: (r: Frame) => number) => rows.reduce((a, r) => a + f(r), 0) / Math.max(1, n);
  const interval = sorted((r) => r[0]), draws = sorted((r) => r[4]), upload = sorted((r) => r[6]);
  const p50 = at(interval, 0.5);
  return {
    frames: n, p50, p99: at(interval, 0.99), worst: at(interval, 1),
    long: rows.filter((r) => r[0] > p50 * 1.5).length, // as mb_app::perf counts them
    update: mean((r) => r[1]), render: mean((r) => r[2]), work99: at(sorted((r) => r[1] + r[2]), 0.99),
    draws: at(draws, 0.5), drawsMax: at(draws, 1), binds: at(sorted((r) => r[5]), 0.5), upload: at(upload, 0.5), uploadMax: at(upload, 1),
    mirror: at(sorted((r) => r[7]), 0.5), mirrorMax: at(sorted((r) => r[7]), 1), instancesMax: at(sorted((r) => r[8]), 1), quadsMax: at(sorted((r) => r[9]), 1),
    shadowed: rows.filter((r) => r[10] > 0).length,
  };
}
const show = (r: Run) =>
  `${r.frames} frames, one every ${r.p50.toFixed(1)} ms, p99 ${r.p99.toFixed(1)}, worst ${r.worst.toFixed(1)}, ${r.long} long; update ${r.update.toFixed(2)} + render ${r.render.toFixed(2)} ms, p99 of their sum ${r.work99.toFixed(1)}; ` +
  `${r.draws} draws (most ${r.drawsMax}) and ${r.mirror} for the mirror (most ${r.mirrorMax}), ${r.binds} binds, ${(r.upload / 1024).toFixed(1)} KiB uploaded`;
// The engine keeps the newest 1024 frames. This moves the ones recorded since the last call into a list on the
// page without turning them into text (the page is CPU-throttled while it is measured, and a slow read-out
// would be a long frame of its own). Returns the number of frames since the recording was reset.
const TAKE_FRAMES = `(() => {
  const w = window.mbRows || (window.mbRows = { seen: 0, f: [], c: [] });
  const total = mbPerf.total(), n = Math.min(total - w.seen, 1024);
  if (n > 0) { const f = wasm_bindgen.perf_frames(), c = wasm_bindgen.perf_counts(); w.f.push(f.slice(f.length - 4 * n)); w.c.push(c.slice(c.length - 7 * n)); }
  w.seen = total;
  return total;
})()`;
const startFrames = (p: Page) => p.ev("mbPerf.reset(); window.mbRows = null; 0");
const takeFrames = async (p: Page): Promise<number> => (await p.ev(TAKE_FRAMES)) ?? 0;
async function endFrames(p: Page): Promise<Frame[]> {
  await takeFrames(p);
  const [f, c] = JSON.parse((await p.ev(`(() => { const flat = (a) => a.reduce((o, x) => { for (const v of x) o.push(Math.round(v * 1000) / 1000); return o; }, []); return JSON.stringify([flat(window.mbRows.f), flat(window.mbRows.c)]); })()`)) ?? "[[],[]]");
  const rows: Frame[] = [];
  for (let i = 0; i * 4 < f.length; i++) rows.push([f[i * 4], f[i * 4 + 1], f[i * 4 + 2], f[i * 4 + 3], ...c.slice(i * 7, i * 7 + 7)] as Frame);
  return rows;
}
// Every frame of the next `ms` milliseconds of play. `each` runs about ten times a second (steering).
async function record(p: Page, ms: number, each?: (elapsed: number) => Promise<void>): Promise<Run> {
  await startFrames(p);
  const t0 = Date.now(); let taken = t0;
  while (Date.now() - t0 < ms) {
    if (each) await each(Date.now() - t0);
    await sleep(100);
    if (Date.now() - taken >= 1500) { taken = Date.now(); await takeFrames(p); } // 1024 frames are 4 s at 250 Hz
  }
  return summarize(await endFrames(p));
}
// A timing is taken a second time before it counts as failed: one busy moment of the machine is no regression.
async function measured<T>(name: string, run: () => Promise<T>, ok: (r: T) => boolean, text: (r: T) => string): Promise<T> {
  let r = await run(); let first = "";
  if (!ok(r)) { first = `first try: ${text(r)}; second: `; console.log(`     (taking it again) ${name}: ${text(r)}`); r = await run(); }
  check(name, ok(r), first + text(r));
  return r;
}
// The display's own frame in ms (the median of 90 frames of an idle page): 6.06 at 165 Hz, 16.7 at 60 Hz.
const displayFrameMs = async (p: Page): Promise<number> => (await p.ev(`new Promise((done) => { const t = []; const f = (now) => { t.push(now); if (t.length < 91) requestAnimationFrame(f); else done(t.slice(1).map((x, i) => x - t[i]).sort((a, b) => a - b)[45]); }; requestAnimationFrame(f); })`)) ?? 0;

// ---- the page's save files (crates/mb-game/src/storage.rs and progress.rs) ------------------------------------
// A value is kept in two slot files, `<key>.a` and `<key>.b`: "MBSL", a generation (u64 LE), the value's length
// and its FNV-1a checksum (u32 LE each), the value. The newest complete one counts.
const fnv1a = (b: Uint8Array) => { let h = 0x811c9dc5; for (const x of b) h = Math.imul(h ^ x, 0x01000193) >>> 0; return h; };
function slotRecord(generation: number, value: Uint8Array): number[] {
  const out = new Uint8Array(20 + value.length); const v = new DataView(out.buffer);
  out.set(new TextEncoder().encode("MBSL")); v.setBigUint64(4, BigInt(generation), true); v.setUint32(12, value.length, true); v.setUint32(16, fnv1a(value), true); out.set(value, 20);
  return Array.from(out);
}
type SaveFiles = Record<string, number[]>;
const SAVES_DIR = `navigator.storage.getDirectory().then((r) => r.getDirectoryHandle('monkey-ball')).then((d) => d.getDirectoryHandle('saves', { create: true }))`;
async function readSaves(p: Page): Promise<SaveFiles> {
  return JSON.parse((await p.ev(`${SAVES_DIR}.then(async (d) => { const out = {}; for await (const [n, h] of d.entries()) if (h.kind === 'file') out[n] = Array.from(new Uint8Array(await (await h.getFile()).arrayBuffer())); return JSON.stringify(out); })`)) ?? "{}");
}
// Replace everything in the saves folder. The running page does not read the files again: reload it next.
async function writeSaves(p: Page, files: SaveFiles): Promise<boolean> {
  return (await p.ev(`${SAVES_DIR}.then(async (d) => { const old = []; for await (const [n] of d.entries()) old.push(n); for (const n of old) await d.removeEntry(n); const files = ${JSON.stringify(files)}; for (const n in files) { const w = await (await d.getFileHandle(n, { create: true })).createWritable(); await w.write(new Uint8Array(files[n])); await w.close(); } return true; })`)) === true;
}
// The profile as the game would read it: generation and text of the newest complete slot.
async function readProfile(p: Page): Promise<{ generation: number; text: string } | null> {
  const files = await readSaves(p); let best: { generation: number; text: string } | null = null;
  for (const slot of ["a", "b"]) {
    const b = Uint8Array.from(files[`profile.${slot}`] ?? []);
    if (b.length < 20 || new TextDecoder().decode(b.subarray(0, 4)) !== "MBSL") continue;
    const v = new DataView(b.buffer), value = b.subarray(20), generation = Number(v.getBigUint64(4, true));
    if (value.length !== v.getUint32(12, true) || fnv1a(value) !== v.getUint32(16, true)) continue;
    if (!best || generation > best.generation) best = { generation, text: new TextDecoder().decode(value) };
  }
  return best;
}
const savedSetting = async (p: Page, name: string) => ((await readProfile(p))?.text.match(new RegExp(`^setting ${name} (.+)$`, "m")) ?? [])[1];
const signSave = (body: string) => `${body}crc ${fnv1a(new TextEncoder().encode(body)).toString(16).padStart(8, "0")}\n`;
// A save as version 0.1.0 wrote it: nine settings (it knew no `reflections`), then progress. Not the defaults,
// so that it shows when they are read: music off, a stick curve of 1.50, floors 1 and 2 of Beginner reached.
const SAVE_0_1_0 = "MBSAVE 1\nsetting music_volume 0\nsetting sfx_volume 40\nsetting announcer_volume 30\nsetting control_mode stick\n" +
  "setting joystick_size 100\nsetting left_handed 0\nsetting tilt_range_deg 25\nsetting input_falloff 150\nsetting unlock_all 0\n" +
  "course beginner 0 0 4200 1,2\nstage 1 900 1\n";

try {
  let p = await Page.open();

  // ---- 1. files that are not the game ----------------------------------------------------------------
  await p.nav(base + "?gl=1");
  check("page loads from python -m http.server", await p.wait(pickerReady, 30000));
  const types = await p.ev(`Promise.all(['pkg/mb_web_bg.wasm','manifest.webmanifest','main.js'].map(f => fetch(f).then(r => r.headers.get('content-type')))).then(a => a.join(' | '))`);
  console.log(`     content types from the server: ${types}`);
  const bad: [string, Uint8Array | null, RegExp][] = [
    ["empty.rvz", new Uint8Array(0), /empty/i],
    ["random.iso", crypto.getRandomValues(new Uint8Array(65536)), /not a GameCube disc image/i],
    ["notes.rvz", new TextEncoder().encode("WIA\x01 this is not a disc image at all ..............................................................."), /WIA/],
    ["truncated.rvz", null, /incomplete|damaged/i],
  ];
  for (const [name, bytes, expect] of bad) {
    const path = `${scratch}/${name}`;
    if (bytes) Deno.writeFileSync(path, bytes);
    else { // the first 100 MB of the real image
      const f = Deno.openSync(discPath); const out = Deno.openSync(path, { write: true, create: true });
      const buf = new Uint8Array(1 << 20);
      for (let i = 0; i < 100; i++) { const n = f.readSync(buf) ?? 0; out.writeSync(buf.subarray(0, n)); }
      f.close(); out.close();
    }
    await p.pick(path);
    await p.wait(`document.getElementById('status').classList.contains('error') && ${pickerReady}`, 20000);
    const msg = (await p.ev(landingText)) ?? "";
    check(`${name}: a specific message and a usable picker`, expect.test(msg) && !/dolphin|convert/i.test(msg) && (await p.ev(pickerReady)) === true, msg.slice(0, 110));
    Deno.removeSync(path);
    await p.ev("document.getElementById('status').classList.remove('error')");
  }
  check("nothing was stored by the failed attempts", (await p.ev(stored)) === "", await p.ev(stored));

  // ---- 2. the real image through the real picker --------------------------------------------------------
  await p.pick(discPath);
  check("import: the game starts after picking the image", await p.wait(started, 120000), (await p.ev(landingText)) ?? "");
  const stats = JSON.parse((await p.ev("JSON.stringify(mbStats)")) ?? "{}");
  check("import: stored on the device", stats.mode === "stored", `${(stats.importMs / 1000).toFixed(1)} s, pack ${(stats.packBytes / 1048576).toFixed(0)} MB, saves ${stats.saveMode}, persisted ${stats.persisted}`);
  check("import: only the pack and its marker are in storage", /^game\.json:\d+ game\.pack:\d+$/.test((await p.ev(stored)) ?? ""), await p.ev(stored));
  check("menu is showing", (await p.status()) === "menu: title", await p.status());

  // ---- 3. play ----------------------------------------------------------------------------------------
  if (await playToFloorTwo(p, "WebGL2", true)) {
    // Still holding forward: off the edge of floor 2, and the floor restarts without a load.
    const before = (await p.ev("JSON.stringify(mbPerf.lastLoad())")) ?? "";
    let sawLoading = false; let retried = false;
    for (let i = 0; i < 150 && !retried; i++) {
      const s = await p.ev("mbPerf.status()") as string ?? "";
      sawLoading ||= s.startsWith("loading");
      retried = /FallOut/.test(s) ? false : retried || (i > 20 && /stage 2 .*Intro/.test(s));
      await sleep(100);
    }
    await p.send("Input.dispatchKeyEvent", { type: "keyUp", code: "KeyW", key: "w", windowsVirtualKeyCode: 87, nativeVirtualKeyCode: 87 });
    check("a lost life restarts the floor in place (no loading screen, no new load)", retried && !sawLoading && before === await p.ev("JSON.stringify(mbPerf.lastLoad())"));
  }
  check("no file was read without being prepared; nothing is held after loading", (await p.ev("mbPerf.misses().length + mbPerf.preparedBytes()")) === 0);
  const audio = JSON.parse((await p.ev("JSON.stringify(mbPerf.audio())")) ?? "{}");
  check("audio is running", audio.state === "Running" && audio.musicPlaying === true && audio.errors === 0, `music ${audio.musicKey}, ${audio.sourcesStarted} sounds started`);
  const playMb = await wasmMb(p);
  const mem = `${playMb.toFixed(0)} MB wasm memory`;
  console.log(`     while playing: ${mem}; last load ${await p.ev("JSON.stringify(mbPerf.lastLoad())")}; music buffers ${(audio.musicBytes / 1048576).toFixed(0)} MB, of it in wasm memory ${(audio.musicPcmBytes / 1048576).toFixed(0)} MB`);
  check(`wasm memory after floors 1 and 2 of Beginner: at most ${MEMORY_BUDGET_MB} MB over 0.1.0's ${V010.beginnerMb} MB`, playMb > 1 && playMb <= V010.beginnerMb + MEMORY_BUDGET_MB, `${playMb.toFixed(1)} MB`);

  // ---- 3b. the page is hidden (another tab in front; a web view's onPause) and shown again -------------------
  await p.waitStatus((x) => /stage 2 .*Playing/.test(x), 15000);
  const cover = await p.send("Target.createTarget", { url: "about:blank" });
  const isHidden = await p.wait("document.hidden", 5000);
  await sleep(600);
  const frames0 = await p.ev("wasm_bindgen.frame_count()");
  await sleep(1200);
  const hiddenAudio = JSON.parse((await p.ev("JSON.stringify(mbPerf.audio())")) ?? "{}");
  check("hidden page: no frames are produced and the audio clock is stopped", isHidden && (await p.ev("wasm_bindgen.frame_count()")) === frames0 && hiddenAudio.state === "Suspended", `audio ${hiddenAudio.state}`);
  await p.send("Target.closeTarget", { targetId: cover.result.targetId });
  await p.send("Page.bringToFront");
  await p.wait("!document.hidden", 5000);
  await sleep(600);
  const shown = await p.status();
  const shownAudio = JSON.parse((await p.ev("JSON.stringify(mbPerf.audio())")) ?? "{}");
  check("shown again: the stage was paused while away, frames and audio are back", /stage 2 .*paused/.test(shown) && (await p.ev("wasm_bindgen.frame_count()")) > frames0 && shownAudio.state === "Running", `${shown}; audio ${shownAudio.state}`);

  // ---- 3c. what 0.2.0 added, in the browser: a 0.1.0 save, the new drawing, frame times, memory ------------
  // Here, in the tab the browser started with: Chrome's CPU throttle slows a tab that was opened later in the
  // session about 1.3 times more for the same factor (measured: Beginner 1 renders in 4.5 ms here and in
  // 6.0 ms in the tab section 4 opens), and 0.1.0's figures are from a first tab.
  // (Its own try: a step that cannot be taken fails one check here and the rest of the run goes on. The
  // save files are put back at the end and a stage is started, so section 4 closes the tab in the middle of
  // play, with the save sections 1 to 3 left, as it always did.)
  const savesBefore = await readSaves(p);
  try {
    const gl = base + "?gl=1&autoscale=0";
    const newProblems = (from: number) => p.problems.slice(from).filter((x) => !/favicon/.test(x));

    // -- a profile written by version 0.1.0 is read: settings, progress, and Reflections is on --
    const old = signSave(SAVE_0_1_0);
    await writeSaves(p, { "profile.a": slotRecord(1, new TextEncoder().encode(old)) });
    await title(p, gl);
    await press(p, "ArrowDown"); await press(p, "Enter"); await press(p, "Enter"); // Practice, Beginner
    // Floors 3 and up were never reached and nothing is unlocked: the keys pass over their rows, so the
    // second step down from floor 1 lands on Back (with floor 3 open it would start floor 3).
    await press(p, "ArrowDown", 2); await press(p, "Enter");
    await sleep(600);
    const locked = await p.status();
    await press(p, "Enter"); await press(p, "ArrowDown"); await press(p, "Enter"); // Beginner again, floor 2: reached
    const reached = await p.waitStatus((x) => x.startsWith("stage 2 "), 15000);
    await inPlay(p);
    const quiet = JSON.parse((await p.ev("JSON.stringify(mbPerf.audio())")) ?? "{}");
    check("a 0.1.0 save: its progress and settings are read (floor 3 locked, floor 2 open, music off)",
      locked === "menu: practice" && reached.startsWith("stage 2 ") && quiet.musicPlaying === false && quiet.state === "Running" && (await p.ev("mbStats.fatal")) === undefined,
      `${locked}; ${reached}; music playing ${quiet.musicPlaying}`);
    // Changing one option makes the game write the whole profile: it is the 0.1.0 one, with that option
    // changed and the setting 0.1.0 did not know added, switched on.
    await chooseOption(p, gl, UNLOCK_ROW);
    const rewritten = await readProfile(p);
    // Without the three settings that are gone since (the announcer's volume, two that were never read).
    const kept = SAVE_0_1_0.replace("setting announcer_volume 30\n", "").replace("setting joystick_size 100\nsetting left_handed 0\n", "");
    const expected = signSave(kept.replace("setting unlock_all 0\n", "setting unlock_all 1\nsetting reflections 1\n"));
    check("a 0.1.0 save: written back whole, with Reflections on", rewritten?.text === expected && rewritten.generation === 2,
      rewritten ? `generation ${rewritten.generation}: ${rewritten.text.replaceAll("\n", " | ").slice(0, 400)}` : "no profile");

    // -- the new drawing is there, on WebGL2 and on WebGPU where the browser has it --
    // The engine reports what every frame drew (mbPerf.counts()): the mirror's draw calls, the particles'
    // instances and quads, and whether the ball's shadow was on.
    async function drawing(label: string, url: string, byTouch: boolean) {
      const from = p.problems.length; p.gfx.length = 0;
      // Course Play, floor 1: the fly-in, the ball put down, GO, a roll through the goal. Every frame from
      // the stage's first to the next floor's, with the frame numbers at which GO and the goal were seen.
      await title(p, url);
      await press(p, "Enter"); await press(p, "Enter");
      let s = await p.waitStatus((x) => x.startsWith("stage 1 "), 20000);
      await startFrames(p);
      let playAt = -1, goalAt = -1;
      for (let i = 0; i < 400 && s.startsWith("stage 1 "); i++) {
        if (playAt >= 0 && i % 20 === 0) await keyEvent(p, "rawKeyDown", "KeyW"); // a press lost while the page takes focus is made again
        s = (await p.ev("mbPerf.status()")) ?? "";
        const total = await takeFrames(p); // (often: the engine keeps four seconds of frames on a fast display)
        if (playAt < 0 && / Playing /.test(s)) playAt = total;
        if (goalAt < 0 && / GoalSequence /.test(s)) goalAt = total;
        await sleep(100);
      }
      await keyEvent(p, "keyUp", "KeyW");
      const rows = await endFrames(p);
      if (goalAt >= 0) s = await p.waitStatus((x) => x.startsWith("stage 2 "), 10000); // (the next floor may still be loading)
      const reachedGoal = playAt >= 0 && goalAt > playAt && s.startsWith("stage 2 ");
      // The goal was noticed up to a poll late: the last half second before it is left out of "before the goal".
      let before = Math.max(goalAt, 0);
      for (let ms = 0; before > 0 && ms < 500; before--) ms += rows[before - 1][0];
      const landing = summarize(rows.slice(0, before)), play = summarize(rows.slice(Math.max(playAt, 0), before)), goal = summarize(rows.slice(Math.max(goalAt, 0)));
      // The ball is put down from a height and rolls off at speed: sparks and a star (quads and instances),
      // long before any confetti.
      check(`${label}: a collision draws sparks or stars (floor 1 of Course Play, before the goal)`, reachedGoal && landing.instancesMax + landing.quadsMax > 0,
        `in the ${landing.frames} frames before the goal: up to ${landing.instancesMax} instances and ${landing.quadsMax} quads in a frame`);
      // Confetti is 160 instanced pieces, thrown at the goal.
      check(`${label}: the goal draws its confetti`, reachedGoal && goal.instancesMax > 0,
        `from the goal on (${goal.frames} frames): up to ${goal.instancesMax} instances and ${goal.quadsMax} quads in a frame; ${s}`);
      // The ball is in sight from GO to the goal, and its shadow with it, in every frame.
      check(`${label}: the ball's shadow is on while the ball is in play`, play.frames > 50 && play.shadowed === play.frames, `${play.shadowed} of ${play.frames} frames of play`);
      const all = summarize(rows);
      check(`${label}: a stage without a mirror floor has no mirror draws (floor 1)`, all.frames > 50 && all.mirrorMax === 0, `most in a frame: ${all.mirrorMax}, of ${all.frames} frames; the picture: ${all.draws} draws`);

      // The waved floor, a night stage with its windows, a storm stage with its fires: each loads and plays.
      for (const [id, what] of [[92, "the waved floor"], [21, "night, window flipbooks"], [81, "storm, fire flipbooks"]] as [StageId, string][]) {
        await practice(p, url, id);
        const go = await inPlay(p);
        await startFrames(p); await sleep(1500);
        const r = summarize(await endFrames(p));
        const now = await p.status();
        check(`${label}: stage ${id} (${what}) loads and runs`, go && / Playing /.test(now) && r.frames > 30 && r.draws > 100 && (await p.ev("mbStats.fatal")) === undefined,
          `${now}; ${r.frames} frames in 1.5 s, ${r.draws} draws and ${r.mirror} for the mirror, shadow in ${r.shadowed} frames`);
      }

      // Stage 89 with Reflections on, then off: the same fly-in, the same draws of the picture itself.
      const flyIn = async () => { await practice(p, url, 89); await startFrames(p); await sleep(3000); return summarize(await endFrames(p)); };
      const on = await flyIn();
      if (byTouch) {
        // A phone-sized screen, fingers only: Options, the Reflections row, Back.
        await title(p, url);
        await p.send("Emulation.setDeviceMetricsOverride", { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
        await p.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
        await sleep(700);
        await p.tap(206, 542); // Options
        const opened = await p.status();
        await p.tap(206, 699); // Reflections (the sixth row; a row and its gap are 88 px here)
        await sleep(500);
        check("touch: a tap on the Reflections row turns it off and saves at once", opened === "menu: options" && (await savedSetting(p, "reflections")) === "0", `${opened}; saved: reflections ${await savedSetting(p, "reflections")}`);
        await p.tap(42, 36); // Back
        await p.send("Emulation.clearDeviceMetricsOverride");
        await p.send("Emulation.setTouchEmulationEnabled", { enabled: false });
      } else {
        await chooseOption(p, url, REFLECTIONS_ROW);
      }
      const off = await flyIn(); // (a fresh page: the option has to have survived the reload)
      const kept = await savedSetting(p, "reflections");
      // The mirror's draws come on top of the picture's own, which the option does not touch.
      check(`${label}: stage 89 has mirror draws with Reflections on, and exactly none with it off after a reload`,
        kept === "0" && on.frames > 50 && off.frames > 50 && on.mirror > 0 && off.mirrorMax === 0 && Math.abs(on.draws - off.draws) <= 0.1 * off.draws,
        `on: ${on.mirror} mirror draws a frame (most ${on.mirrorMax}) beside ${on.draws} of the picture; off: most ${off.mirrorMax} beside ${off.draws}; saved: reflections ${kept}`);
      if (byTouch) {
        await title(p, url);
        await p.send("Emulation.setDeviceMetricsOverride", { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
        await p.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
        await sleep(700);
        await p.tap(206, 542); await p.tap(206, 699);
        await sleep(500);
        check("touch: a second tap turns Reflections on again", (await savedSetting(p, "reflections")) === "1", `saved: reflections ${await savedSetting(p, "reflections")}`);
        await p.tap(42, 36);
        await p.send("Emulation.clearDeviceMetricsOverride");
        await p.send("Emulation.setTouchEmulationEnabled", { enabled: false });
      } else {
        await chooseOption(p, url, REFLECTIONS_ROW);
        if ((await savedSetting(p, "reflections")) !== "1") throw new Error("Reflections could not be switched on again");
      }
      // (Chrome on Windows notes on every WebGPU start that it ignores the adapter's power preference, which
      // the page states for the devices that honour it: a notice about the browser, not about the drawing.)
      const said = [...newProblems(from), ...p.gfx.filter((x) => !/powerPreference option is currently ignored/.test(x))];
      check(`${label}: no error in the console and none from the graphics driver, in all of it`, said.length === 0, said.slice(0, 3).join(" || "));
    }
    await drawing("WebGL2", gl, true);
    await title(p, base + "?autoscale=0");
    const gpu = (await p.ev("mbStats.gpu")) ?? "";
    if (/BrowserWebGpu/.test(gpu)) await drawing("WebGPU", base + "?autoscale=0", false);
    else console.log(`     (this Chrome gives the page no WebGPU: ${gpu}; the drawing was checked on WebGL2 only)`);

    // -- frame times on WebGL2 with the CPU slowed six times, as 0.1.0 was measured (docs/WEB.md) --
    const timedFrom = p.problems.length; p.gfx.length = 0;
    await title(p, gl);
    const display = await displayFrameMs(p);
    console.log(`     display: a frame every ${display.toFixed(2)} ms; ${(await p.ev("document.getElementById('mb-canvas').width + ' x ' + document.getElementById('mb-canvas').height")) ?? "?"} pixels; ${(await p.ev("mbStats.gpu")) ?? ""}`);
    const slowed = async <T>(run: () => Promise<T>): Promise<T> => { await throttle(p, CPU_THROTTLE); try { return await run(); } finally { await throttle(p, 1); } };

    // Beginner 1, the ball standing where it is put down.
    const b1 = { ...V010.beginner, ms: V010.beginner.updateMs + V010.beginner.renderMs };
    await measured(`Beginner 1 at ${CPU_THROTTLE}x CPU throttle on WebGL2: update and render within ${SLOWER} times 0.1.0's ${b1.ms.toFixed(2)} ms, no more draw calls`,
      async () => { await practice(p, gl, 1); await inPlay(p); return await slowed(async () => { await sleep(1500); return await record(p, 8000); }); },
      (r) => r.frames > 200 && r.update + r.render <= SLOWER * b1.ms && r.draws <= b1.draws + PARTICLE_DRAWS,
      (r) => `update ${r.update.toFixed(2)} + render ${r.render.toFixed(2)} ms (0.1.0: ${b1.updateMs} + ${b1.renderMs}), ${r.draws} draws (0.1.0: ${b1.draws}); ${show(r)}`);

    // Expert 50, thirty seconds of steering in a square (falls and restarts are part of it).
    const ex = V010.expert;
    // 0.1.0's p99 was 1.09 frames of its display; the limit is 1.5 times that, on this run's display.
    const p99Limit = SLOWER * ex.p99 * Math.max(1, display / ex.displayMs);
    // Two long frames in 4917 is too small a count to scale by 1.5 (three would pass, four fail, and two to
    // three is what a run gives): the limit is three times 0.1.0's share of the frames.
    const longLimit = (frames: number) => Math.ceil(3 * ex.long * frames / ex.frames);
    let memoryAt90 = 0;
    await measured(`Expert 50, 30 s of steering at ${CPU_THROTTLE}x on WebGL2: p99 within ${SLOWER} times 0.1.0's, long frames within 3 times`,
      async () => {
        await practice(p, gl, 90); await inPlay(p);
        const order = ["KeyW", "KeyD", "KeyS", "KeyA"]; let held = -1;
        const r = await slowed(async () => {
          await sleep(1500);
          return await record(p, 30000, async (t) => { const k = Math.floor(t / 700) % 4; if (k === held) return; if (held >= 0) await keyEvent(p, "keyUp", order[held]); await keyEvent(p, "rawKeyDown", order[k]); held = k; });
        });
        if (held >= 0) await keyEvent(p, "keyUp", order[held]);
        memoryAt90 = await wasmMb(p);
        return r;
      },
      (r) => r.frames > 1000 && r.p99 <= p99Limit && r.long <= longLimit(r.frames),
      (r) => `${r.long} long of ${r.frames} (0.1.0: ${ex.long} of ${ex.frames}; limit ${longLimit(r.frames)}), p99 ${r.p99.toFixed(1)} ms (0.1.0: ${ex.p99}; limit ${p99Limit.toFixed(1)}); ${show(r)}`);
    console.log(`     Expert 50 in a fresh page: ${memoryAt90.toFixed(1)} MB wasm memory`);

    // The mirror floors that drew the most in 0.1.0, each from the first frame of its fly-in to six seconds
    // after GO with nobody steering (the simulation is deterministic, so every run draws the same), with
    // Reflections on, as the game ships, and off. Both have to fit a 60 Hz frame, and with the mirror the
    // page's memory has to stay within the budget over what 0.1.0 needed for the same stage.
    const mirrors: StageId[] = [89, 76, 21];
    const stage = async (id: StageId) => {
      await practice(p, gl, id);
      const r = await slowed(async () => { const flyIn = await record(p, 4500); await inPlay(p); return { flyIn, play: await record(p, 6000) }; });
      return { ...r, mb: await wasmMb(p), work99: Math.max(r.flyIn.work99, r.play.work99), p99: Math.max(r.flyIn.p99, r.play.p99) };
    };
    const line = (r: Awaited<ReturnType<typeof stage>>) =>
      `p99 of update + render ${r.work99.toFixed(1)} ms, of the frame ${r.p99.toFixed(1)} ms; fly-in ${r.flyIn.update.toFixed(2)} + ${r.flyIn.render.toFixed(2)} ms, ${r.flyIn.draws} draws (most ${r.flyIn.drawsMax}) + ${r.flyIn.mirror} mirror (most ${r.flyIn.mirrorMax}); ` +
      `play ${r.play.update.toFixed(2)} + ${r.play.render.toFixed(2)} ms, ${r.play.draws} draws (most ${r.play.drawsMax}) + ${r.play.mirror} mirror (most ${r.play.mirrorMax}); ${r.mb.toFixed(1)} MB wasm memory`;
    const withMirror: Record<number, Awaited<ReturnType<typeof stage>>> = {};
    for (const id of mirrors) {
      withMirror[id] = await measured(`stage ${id} with Reflections at ${CPU_THROTTLE}x on WebGL2: update and render fit a ${FRAME_BUDGET_MS} ms frame at p99`,
        () => stage(id), (r) => r.flyIn.frames > 100 && r.play.frames > 100 && r.work99 <= FRAME_BUDGET_MS && r.flyIn.mirrorMax + r.play.mirrorMax > 0, line);
      const mb = withMirror[id].mb;
      check(`stage ${id} with Reflections on: wasm memory at most ${MEMORY_BUDGET_MB} MB over 0.1.0's ${V010.freshMb[id]} MB`, mb > 1 && mb <= V010.freshMb[id] + MEMORY_BUDGET_MB, `${mb.toFixed(1)} MB`);
    }
    await chooseOption(p, gl, REFLECTIONS_ROW);
    if ((await savedSetting(p, "reflections")) !== "0") throw new Error("Reflections could not be switched off");
    for (const id of mirrors) {
      const off = await measured(`stage ${id} without Reflections at ${CPU_THROTTLE}x on WebGL2: update and render fit a ${FRAME_BUDGET_MS} ms frame at p99`,
        () => stage(id), (r) => r.flyIn.frames > 100 && r.play.frames > 100 && r.work99 <= FRAME_BUDGET_MS, line);
      const on = withMirror[id];
      console.log(`     stage ${id}: the mirror costs ${(on.work99 - off.work99).toFixed(1)} ms at p99 of update + render and ${(on.mb - off.mb).toFixed(1)} MB of wasm memory`);
    }
    await chooseOption(p, gl, REFLECTIONS_ROW);
    if ((await savedSetting(p, "reflections")) !== "1") throw new Error("Reflections could not be switched on again");

    // -- WebGPU keeps its pace (the WebGL2 path's fences and flushes are not on it) --
    if (/BrowserWebGpu/.test(gpu)) {
      const webgpu = base + "?autoscale=0";
      await measured(`Beginner 1 at ${CPU_THROTTLE}x CPU throttle on WebGPU: frames keep the display's pace`,
        async () => { await practice(p, webgpu, 1); await inPlay(p); return await slowed(async () => { await sleep(1500); return await record(p, 8000); }); },
        (r) => r.frames > 200 && r.p99 <= p99Limit && r.long <= Math.max(longLimit(r.frames), 3) && r.update + r.render <= b1.ms,
        (r) => `${r.long} long of ${r.frames}, p99 ${r.p99.toFixed(1)} ms (limit ${p99Limit.toFixed(1)}); ${show(r)}`);
    }
    // On WebGL2 the engine waits for the driver at its own fences and flushes what a load staged; a browser
    // that minds either says so in its log (a sync object it refused, a context it slowed down or lost).
    const timedSaid = [...newProblems(timedFrom), ...p.gfx.filter((x) => !/powerPreference option is currently ignored/.test(x))];
    check("the timed runs: no error in the console, no warning from the graphics driver (fences, sync objects, lost contexts)", timedSaid.length === 0, timedSaid.slice(0, 3).join(" || "));
  } catch (e) {
    check("0.2.0 in the browser: the checks ran to the end", false, String(e));
  } finally {
    await throttle(p, 1);
    await p.send("Emulation.clearDeviceMetricsOverride");
    await p.send("Emulation.setTouchEmulationEnabled", { enabled: false });
    await writeSaves(p, savesBefore);
    await p.nav(base + "?gl=1");
    await p.wait(started, 30000);
    await p.send("Page.bringToFront");
    await p.ev("document.getElementById('mb-canvas').focus()");
    await p.key("Enter"); await sleep(300); await p.key("Enter"); // Course Play, Beginner
    await p.waitStatus((x) => x.startsWith("stage 1 "), 20000);
  }


  // ---- 4. the tab is closed in the middle of play; a new one starts from the device --------------------
  const fresh = await p.send("Target.createTarget", { url: "about:blank" });
  await p.send("Target.closeTarget", { targetId: p.targetId });
  p.close();
  p = await Page.open(fresh.result.targetId);
  await p.nav(base + "?gl=1");
  check("after closing the tab mid-play: starts with no picker", await p.wait(started, 30000) && (await p.ev("mbStats.mode")) === "stored", `first frame after ${Math.round((await p.ev("mbStats.firstFrameMs")) ?? 0)} ms`);
  // Course Play marked floors 1 and 2 as reached and saved at the goal: Practice lists floor 2 unlocked.
  await p.ev("document.getElementById('mb-canvas').focus()");
  await p.key("ArrowDown"); await p.key("Enter"); await p.key("Enter"); await p.key("ArrowDown"); await p.key("Enter");
  const s = await p.waitStatus((x) => x.startsWith("stage 2 ") || x.startsWith("loading"), 10000);
  check("the save written before the tab was closed is intact (floor 2 is unlocked in Practice)", s.startsWith("stage 2 ") || s.startsWith("loading stage 2"), s);
  await p.waitStatus((x) => x.startsWith("stage 2 "), 10000);

  // ---- 5. touch only: scroll a long list to its end, pause, back out through every screen -------------
  // (Unlock every stage first, with the keys, so the longest list can be walked.)
  await p.key("Escape"); await p.key("ArrowUp"); await p.key("Enter"); await p.key("Escape"); await p.key("Escape");
  // (The title comes back with Practice selected; Options is the next row.)
  await p.key("ArrowDown"); await p.key("Enter"); await p.key("Enter"); await p.key("Escape");
  check("keys: back at the title", (await p.status()) === "menu: title", await p.status());
  await p.send("Emulation.setDeviceMetricsOverride", { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
  await p.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  await sleep(700);
  check("the back gesture is free to leave the page at the title", (await p.ev("mbPerf.navDepth()")) === 0);
  await p.tap(206, 542); // Options
  check("touch: a tap opens Options", (await p.status()) === "menu: options", await p.status());
  await sleep(400);
  await p.send("Page.navigateToHistoryEntry", { entryId: await p.send("Page.getNavigationHistory").then((h) => h.result.entries[h.result.currentIndex - 1].id) });
  await sleep(700);
  check("the browser's back gesture goes back inside the game, not out of the page", (await p.status()) === "menu: title" && ((await p.ev("location.href")) ?? "").startsWith(base), await p.status());
  await p.tap(206, 477); await p.tap(206, 428); // Practice, Expert (50 floors)
  check("touch: the Expert stage list opens", (await p.status()) === "menu: stage select", await p.status());
  for (let i = 0; i < 14; i++) await p.swipe(200, 800, 300);
  await sleep(1200);
  await p.tap(180, 784); // the last stage row, now at the bottom of the list
  const last = await p.waitStatus((x) => x.startsWith("stage "), 15000);
  check("touch: dragging reaches the last stage of the list and a tap starts it", last.startsWith("stage 90 "), last);
  console.log(`     at Expert 50 (stage 90): ${((await p.ev("mbPerf.wasmMemoryBytes()")) / 1048576).toFixed(0)} MB wasm memory, music buffers ${(JSON.parse((await p.ev("JSON.stringify(mbPerf.audio())")) ?? "{}").musicBytes / 1048576).toFixed(0)} MB`);
  const expertMb = await wasmMb(p);
  check(`wasm memory with Expert 50 loaded: at most ${MEMORY_BUDGET_MB} MB over 0.1.0's ${V010.expertMb} MB`, expertMb > 1 && expertMb <= V010.expertMb + MEMORY_BUDGET_MB, `${expertMb.toFixed(1)} MB`);
  await sleep(500);
  await p.tap(361, 864); // the on-screen pause button
  check("touch: the pause button pauses", /paused/.test(await p.status()), await p.status());
  await p.tap(206, 612); // Quit (practice does not ask)
  check("touch: quit returns to the stage list", (await p.status()) === "menu: stage select", await p.status());
  await p.tap(42, 36); // Back
  check("touch: Back leaves the stage list", (await p.status()) === "menu: practice", await p.status());
  await p.tap(42, 36, 1200); // a slow, deliberate press
  check("touch: a long press on Back still counts", (await p.status()) === "menu: title", await p.status());
  await p.send("Emulation.clearDeviceMetricsOverride");
  await p.send("Emulation.setTouchEmulationEnabled", { enabled: false });

  // ---- 5b. idle cost, and steering by device tilt (emulated sensor) ---------------------------------------
  await p.nav(base + "?gl=1&tilt=1");
  await p.wait(started, 30000);
  await p.send("Performance.enable");
  const taskTime = async () => (await p.send("Performance.getMetrics")).result.metrics.find((m: any) => m.name === "TaskDuration").value as number;
  await sleep(1500);
  const idle0 = await taskTime(); await sleep(4000); const idle = ((await taskTime()) - idle0) / 4;
  check("a still menu costs next to nothing", idle < 0.05, `${(idle * 100).toFixed(1)} % of one core on the page's main thread`);
  await p.ev("document.getElementById('mb-canvas').focus()");
  // Options -> Controls: TILT (the fifth row), back, Practice -> Beginner -> stage 1.
  await p.key("ArrowDown"); await p.key("ArrowDown"); await p.key("Enter");
  for (let i = 0; i < 4; i++) await p.key("ArrowDown");
  await p.key("Enter"); await p.key("Escape");
  await p.send("DeviceOrientation.setDeviceOrientationOverride", { alpha: 0, beta: 40, gamma: 0 }); // held at 40 degrees
  await p.key("ArrowUp"); await p.key("Enter"); await p.key("Enter"); await p.key("Enter");
  await p.waitStatus((x) => /stage 1 .*Playing/.test(x), 20000);
  const play0 = await taskTime(); await sleep(3000); const playing = ((await taskTime()) - play0) / 3;
  console.log(`     main-thread time while playing: ${(playing * 100).toFixed(1)} % of one core`);
  const still = await p.status();
  await p.send("DeviceOrientation.setDeviceOrientationOverride", { alpha: 0, beta: 22, gamma: 0 }); // top edge tipped away
  const cleared = await p.waitStatus((x) => x === "menu: practice clear", 30000);
  check("tilt: tipping the device forward rolls the ball through the goal", /Playing/.test(still) && cleared === "menu: practice clear", `${still} -> ${cleared}`);
  await p.send("DeviceOrientation.clearDeviceOrientationOverride");
  // Back to the stick for the rest of the run.
  await p.key("Escape"); await p.key("Escape"); await p.key("Escape");
  await p.key("ArrowDown"); await p.key("Enter");
  for (let i = 0; i < 4; i++) await p.key("ArrowDown");
  await p.key("Enter"); await p.key("Escape");
  check("keys: back at the title after the tilt run", (await p.status()) === "menu: title", await p.status());

  // ---- 6. remove the data; close the tab in the middle of an import ---------------------------------------
  await p.ev("mbHost('remove-data')");
  await sleep(1500);
  check("remove game data: back at the import screen, storage empty of game data", await p.wait(pickerReady, 20000) && !/game\./.test((await p.ev(stored)) ?? "x"), await p.ev(stored));
  await p.pick(discPath);
  await p.wait(`/Copying/.test(${landingText})`, 20000);
  const fresh2 = await p.send("Target.createTarget", { url: "about:blank" });
  await p.send("Target.closeTarget", { targetId: p.targetId });
  p.close();
  p = await Page.open(fresh2.result.targetId);
  await sleep(500);
  await p.nav(base + "?gl=1");
  const ready = await p.wait(pickerReady, 30000);
  check("after closing the tab mid-import: the import screen again, nothing half-imported is used", ready && (await p.ev("mbStats.mode")) === "none", `storage: '${await p.ev(stored)}'`);
  await p.pick(discPath);
  await sleep(250);
  await p.ev("document.getElementById('cancel').click()");
  await p.wait(`/cancelled/i.test(${landingText}) && ${pickerReady}`, 15000);
  check("cancel: the import stops, nothing is kept, the picker is usable", /cancelled/i.test((await p.ev(landingText)) ?? "") && !/game\./.test((await p.ev(stored)) ?? "x"), `storage: '${await p.ev(stored)}'`);
  await p.pick(discPath);
  check("the import runs again from the start and completes", await p.wait(started, 120000) && (await p.ev("mbStats.mode")) === "stored");

  // ---- 7. no storage: the session plays from the picked file --------------------------------------------
  await p.nav(base + "?gl=1&nostore=1");
  await p.wait(started, 30000);
  await p.ev("mbHost('remove-data')");
  await sleep(1500);
  await p.nav(base + "?gl=1&nostore=1");
  await p.wait(pickerReady, 30000);
  await p.pick(discPath);
  check("without storage: starts from the picked file", await p.wait(started, 60000) && (await p.ev("mbStats.mode")) === "live", `${Math.round((await p.ev("mbStats.importMs")) ?? 0)} ms to open`);
  await playToFloorTwo(p, "without storage");
  check("without storage: nothing was written", !/game\./.test((await p.ev(stored)) ?? "x"));

  // ---- 7b. playing from the picked file, and the file goes away (a copy of the image, deleted) --------------
  const copy = `${scratch}/copy.rvz`;
  Deno.copyFileSync(discPath, copy);
  await p.nav(base + "?gl=1&nostore=1");
  await p.wait(pickerReady, 30000);
  await p.pick(copy);
  await p.wait(started, 60000);
  Deno.removeSync(copy);
  await p.send("Page.bringToFront");
  await p.ev("document.getElementById('mb-canvas').focus()");
  await p.key("ArrowDown"); await p.key("Enter"); await p.key("Enter"); await p.key("Enter"); // Practice, Beginner, floor 1
  const goneShown = await p.wait("!document.getElementById('fatal').hidden", 15000);
  const goneTitle = (await p.ev("document.getElementById('fatal-title').textContent")) ?? "";
  const goneStatus = await p.status();
  check("the picked file disappears mid-session: the game says so and stays alive", goneShown && /no longer be read/.test(goneTitle) && goneStatus.startsWith("menu:"), `'${goneTitle}'; ${goneStatus}`);
  await p.ev("document.getElementById('reload').click()");
  await sleep(500);
  check("... and one button leads back to the file picker", await p.wait(`${pickerReady} && mbStats.mode === 'none'`, 30000));

  // ---- 7c. a bare web view: no private file system, no DOM storage, no service worker, no WebGPU, no extras --
  const bare = await p.inject(`
    delete Navigator.prototype.serviceWorker; delete Navigator.prototype.gpu; delete Navigator.prototype.wakeLock;
    if (self.StorageManager) { delete StorageManager.prototype.getDirectory; delete StorageManager.prototype.persist; delete StorageManager.prototype.estimate; }
    Object.defineProperty(window, "localStorage", { get() { return null; } });
    Object.defineProperty(document, "fullscreenEnabled", { get() { return false; } });
    delete Element.prototype.requestFullscreen; delete window.DeviceOrientationEvent; delete Navigator.prototype.getGamepads;
    delete WebAssembly.compileStreaming; delete WebAssembly.instantiateStreaming;`);
  await p.nav(base);
  check("bare web view: the page loads and explains that nothing can be kept", await p.wait(pickerReady, 30000) && /cannot keep data/.test((await p.ev("document.getElementById('step-keep').textContent")) ?? ""), (await p.ev(landingText)) ?? "");
  await p.pick(discPath);
  check("bare web view: plays from the picked file on WebGL2, saves in memory only", await p.wait(started, 60000) && (await p.ev("mbStats.mode")) === "live" && (await p.ev("mbStats.saveMode")) === "memory" && /Gl,/.test((await p.ev("mbStats.gpu")) ?? ""), `${await p.ev("mbStats.gpu")}; saves ${await p.ev("mbStats.saveMode")}; ${await p.ev("mbStats.liveWhy")}`);
  await playToFloorTwo(p, "bare web view");
  check("bare web view: no errors in the console", p.problems.filter((x) => !/favicon/.test(x)).length === 0, p.problems.slice(0, 3).join(" || "));
  await p.uninject(bare);

  // ---- 7d. engines that cannot run the game get a message, not a silent stop ---------------------------------
  const lacking: [string, string, RegExp][] = [
    ["no web workers", "delete window.Worker;", /web workers/],
    ["no WebGL2 and no WebGPU", "delete Navigator.prototype.gpu; const g = HTMLCanvasElement.prototype.getContext; HTMLCanvasElement.prototype.getContext = function (t, ...a) { return /webgl/.test(t) ? null : g.call(this, t, ...a); };", /neither WebGL2 nor WebGPU/],
    ["a WebAssembly engine that rejects the module", "delete WebAssembly.compileStreaming; WebAssembly.compile = () => Promise.reject(new WebAssembly.CompileError('bulk memory is not supported'));", /cannot run the engine.*Chrome or Android System WebView 97/],
    ["no WebAssembly", "delete window.WebAssembly;", /no WebAssembly/],
  ];
  for (const [name, source, expect] of lacking) {
    const idn = await p.inject(source);
    await p.nav(base + "?gl=1");
    await p.wait("document.getElementById('status').classList.contains('error')", 15000);
    const msg = (await p.ev(landingText)) ?? "";
    check(`${name}: the page says what is missing`, expect.test(msg) && (await p.ev("document.getElementById('pick').disabled")) === true, msg.slice(0, 120));
    await p.uninject(idn);
  }
  p.problems.length = 0;

  // ---- 7e. the same files under a sub-path (as an app's asset loader would serve them) ----------------------
  const nested = `${scratch}/nested/assets/site`;
  Deno.mkdirSync(`${nested}/pkg`, { recursive: true });
  for (const e of Deno.readDirSync(dist)) if (e.isFile) Deno.copyFileSync(`${dist}/${e.name}`, `${nested}/${e.name}`);
  for (const e of Deno.readDirSync(`${dist}/pkg`)) Deno.copyFileSync(`${dist}/pkg/${e.name}`, `${nested}/pkg/${e.name}`);
  const subServer = new Deno.Command(python, { args: ["-m", "http.server", String(HTTP + 1), "--bind", "127.0.0.1", "--directory", `${scratch}/nested`], stdout: "null", stderr: "null" }).spawn();
  try {
    const sub = `http://localhost:${HTTP + 1}/assets/site/`;
    await p.nav(sub + "index.html?gl=1");
    await p.wait(pickerReady, 30000);
    await p.pick(discPath);
    const subStarted = await p.wait(started, 120000) && (await p.status()) === "menu: title";
    const outside = (await p.ev(`JSON.stringify(performance.getEntriesByType("resource").map(r => r.name).filter(n => !n.startsWith(${JSON.stringify(sub)})))`)) ?? "?";
    check("sub-path: imports and starts; every request stays under the sub-path", subStarted && outside === "[]", `mode ${await p.ev("mbStats.mode")}; outside: ${outside}`);
    const scope = (await p.ev("navigator.serviceWorker.ready.then(r => r.scope)")) ?? "";
    check("sub-path: the service worker's scope is the site's own folder", scope === sub, scope);
    await p.ev("mbHost('remove-data')");
    await p.wait(pickerReady, 20000);
  } finally {
    try { subServer.kill(); } catch { /* gone */ }
  }

  if (flags.has("--lan")) {
    const ip = Deno.networkInterfaces().find((i) => i.family === "IPv4" && !i.address.startsWith("127.") && !i.address.startsWith("169.254."))?.address;
    if (ip) {
      await p.nav(`http://${ip}:${HTTP}/`);
      await p.wait(pickerReady, 30000);
      check("network address: not a secure context", (await p.ev("isSecureContext")) === false && (await p.ev("typeof navigator.gpu")) === "undefined");
      await p.pick(discPath);
      check("network address: starts from the picked file on WebGL2", await p.wait(started, 60000) && (await p.ev("mbStats.mode")) === "live" && /Gl,/.test((await p.ev("mbStats.gpu")) ?? ""), `${await p.ev("mbStats.gpu")}; saves ${await p.ev("mbStats.saveMode")}`);
      await playToFloorTwo(p, "network address");
    } else console.log("     --lan: no network address found");
  }

  // ---- 8. offline: stored data and the service worker, with the server gone ---------------------------
  await p.nav(base + "?gl=1");
  await p.wait(pickerReady, 30000);
  await p.pick(discPath);
  await p.wait(started, 120000);
  const sw = await p.wait("navigator.serviceWorker.getRegistration().then(r => !!(r && r.active && r.active.state === 'activated'))", 20000);
  check("service worker is active", sw);
  server.kill();
  await sleep(800);
  await p.nav(base + "?gl=1");
  check("with the server stopped: the installed page still starts and reaches the menu", await p.wait(started, 30000) && (await p.status()) === "menu: title", await p.status());
  server = new Deno.Command(python, { args: ["--version"], stdout: "null", stderr: "null" }).spawn();

  const noise = p.problems.filter((x) => !/favicon/.test(x));
  check("no errors in the console of the last page", noise.length === 0, noise.slice(0, 3).join(" || "));
  p.close();
} catch (e) {
  check("the check itself ran to the end", false, String(e));
} finally {
  await cleanup();
}
console.log(failures ? `\n${failures} check(s) FAILED` : `\nall ${results.length} checks passed`);
Deno.exit(failures ? 1 : 0);
