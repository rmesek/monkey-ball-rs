// End-to-end check of the browser build, in the installed Chrome, through the real file picker.
//
//   deno run -A scripts/web-e2e.ts [--keep] [--lan]
//
// It serves web/dist with `python -m http.server` (the plain static server the bundle is meant for), starts
// Chrome with a throw-away profile, and drives it over the DevTools protocol: the disc image is set on the
// page's own <input type=file>, exactly as a user would pick it. No npm packages, nothing downloaded.
// Needs: the bundle (scripts/build-web), Chrome, Python 3, and your disc image (MB_DISC, or
// "Super Monkey Ball*.rvz|iso|gcm" next to the repository). Missing any of them it says so and exits 0.
//
//   --lan   also open the page through this machine's network address (plain http, not a secure context):
//           the server then listens on all interfaces for the length of the run.
//   --keep  leave the browser open at the end.

const root = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const dist = `${root}web/dist`;
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
if (!exists(`${dist}/index.html`)) skip("web/dist is not built (run scripts/build-web.ps1 or .sh first)");
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
    };
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
  const mem = `${((await p.ev("mbPerf.wasmMemoryBytes()")) / 1048576).toFixed(0)} MB wasm memory`;
  console.log(`     while playing: ${mem}; last load ${await p.ev("JSON.stringify(mbPerf.lastLoad())")}; music buffers ${(audio.musicBytes / 1048576).toFixed(0)} MB, of it in wasm memory ${(audio.musicPcmBytes / 1048576).toFixed(0)} MB`);

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
  // Options -> Controls: TILT (the sixth row), back, Practice -> Beginner -> stage 1.
  await p.key("ArrowDown"); await p.key("ArrowDown"); await p.key("Enter");
  for (let i = 0; i < 5; i++) await p.key("ArrowDown");
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
  for (let i = 0; i < 5; i++) await p.key("ArrowDown");
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

  // ---- 8. the single file, opened from disk -------------------------------------------------------------
  if (exists(`${dist}/monkey-ball.html`)) {
    await p.nav(`file:///${dist.replace(/^\//, "")}/monkey-ball.html`);
    check("single file: opens from disk", await p.wait(pickerReady, 30000), (await p.ev(landingText)) ?? "");
    await p.pick(discPath);
    // What a page opened from disk may do differs by browser: Chrome treats it as a secure context (the
    // import is stored), others do not (the session plays from the file). Both are fine; say which.
    check("single file: starts after picking the image", await p.wait(started, 120000), `mode ${await p.ev("mbStats.mode")}; ${await p.ev("mbStats.gpu")}; saves ${await p.ev("mbStats.saveMode")}; secure context ${await p.ev("isSecureContext")}`);
    await playToFloorTwo(p, "single file");
  } else console.log("     (no web/dist/monkey-ball.html: build with --single to check the single file)");

  // ---- 9. offline: stored data and the service worker, with the server gone ---------------------------
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
