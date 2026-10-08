// The page's script. Plain classic script, no bundler, no imports (so the same file also works inlined in
// the single-file build opened from disk). It
//   1. loads the engine (pkg/mb_web.js + .wasm, built from crates/mb-web),
//   2. finds game data imported earlier in this browser's Origin Private File System, or asks for the disc
//      image and has a worker (worker.js) copy the game's files out of it,
//   3. starts the game on the canvas and looks after the page around it: fullscreen, wake lock, the back
//      gesture, device tilt, error dialogs.
// The disc image is only ever read with Blob.slice() on this device. The only requests this script makes are
// for its own bundle files; see docs/WEB.md.
"use strict";

(() => {
  const wasm = wasm_bindgen; // the glue script's global: wasm(...) initialises, wasm.<name> are the exports

  // Where the imported data lives in the Origin Private File System. worker.js writes it, this file reads
  // and deletes it. The marker is written last: a pack without one is an interrupted import.
  // There is one pack: importing again (the same disc or another region) replaces it, never adds to it.
  const STORE = { dir: "monkey-ball", pack: "game.pack", marker: "game.json", saves: "saves" };
  const MB = 1048576;
  // Stamped by the build: makes every bundle file's URL new with every build, so a plain static server
  // without cache headers cannot hand out a stale script next to a new engine.
  const versionMeta = document.querySelector('meta[name="mb-version"]');
  const VERSION_QUERY = versionMeta && versionMeta.content ? "?v=" + versionMeta.content : "";

  const $ = (id) => document.getElementById(id);
  const landing = $("landing");
  const canvas = $("mb-canvas");
  const pick = $("pick");
  const cancel = $("cancel");
  const removeButton = $("remove");
  const fileInput = $("file");
  const statusEl = $("status");
  const progress = $("progress");
  const bar = $("bar");
  const params = new URLSearchParams(location.search);

  let wasmModule = null; // compiled once, shared with the worker
  let worker = null;
  let busy = false;
  let running = false;
  const stats = (window.mbStats = { mode: "none" });

  function setStatus(text, isError = false) {
    statusEl.textContent = text;
    statusEl.classList.toggle("error", isError);
  }

  function setProgress(fraction) {
    progress.hidden = fraction === null;
    if (fraction !== null) {
      const pct = Math.max(0, Math.min(100, Math.round(fraction * 100)));
      bar.firstElementChild.style.width = pct + "%";
      bar.setAttribute("aria-valuenow", String(pct));
    }
  }

  function describeError(e) {
    if (typeof e === "string") return e;
    if (e && e.name === "NotReadableError") {
      return "The browser could not read the file (it may have been moved or changed). Choose it again.";
    }
    if (e && e.name === "QuotaExceededError") return "This browser has no room left for the game data.";
    return (e && e.message) || String(e);
  }

  // ---- the engine ---------------------------------------------------------------------------------------

  // The single-file build carries the module in the page (base64, optionally gzipped); the hosted build
  // fetches the file next to the page.
  async function compileEngine() {
    const embedded = $("mb-wasm");
    if (embedded) {
      const raw = Uint8Array.from(atob(embedded.textContent.trim()), (c) => c.charCodeAt(0));
      if (embedded.dataset.encoding !== "gzip") return WebAssembly.compile(raw);
      if (typeof DecompressionStream !== "function") {
        throw new Error("this browser is too old for the single-file build (no DecompressionStream)");
      }
      const stream = new Blob([raw]).stream().pipeThrough(new DecompressionStream("gzip"));
      return WebAssembly.compile(await new Response(stream).arrayBuffer());
    }
    const url = "./pkg/mb_web_bg.wasm" + VERSION_QUERY;
    if (WebAssembly.compileStreaming) {
      try {
        return await WebAssembly.compileStreaming(fetch(url));
      } catch (e) {
        // A host that does not label .wasm as application/wasm: compile from the bytes instead.
        console.info("streaming compile failed, compiling from bytes:", e && e.message);
      }
    }
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return WebAssembly.compile(await response.arrayBuffer());
  }

  // The import worker: worker.js next to the page, or (single-file build) the glue and the worker source
  // carried in the page, started from a Blob.
  function makeWorker() {
    const source = $("mb-worker");
    if (!source) return new Worker("./worker.js" + VERSION_QUERY);
    const blob = new Blob([$("mb-glue").textContent, "\n", source.textContent], { type: "text/javascript" });
    const url = URL.createObjectURL(blob);
    const w = new Worker(url);
    setTimeout(() => URL.revokeObjectURL(url), 10000);
    return w;
  }

  // ---- on-device storage (Origin Private File System) -----------------------------------------------------

  async function storeDir(create) {
    if (!(navigator.storage && navigator.storage.getDirectory)) return null;
    try {
      const root = await navigator.storage.getDirectory();
      return await root.getDirectoryHandle(STORE.dir, { create });
    } catch (e) {
      return null; // not a secure context, private mode, or nothing stored yet
    }
  }

  // Whether the import can be stored: the storage exists and has room.
  async function storageVerdict(needBytes) {
    if (params.get("nostore") === "1") return { ok: false, why: "storage was switched off with ?nostore=1" };
    if (!window.isSecureContext) {
      return { ok: false, why: "this page is not a secure context (plain http or a local file)" };
    }
    if (!(await storeDir(true))) return { ok: false, why: "this browser provides no private file storage here" };
    try {
      const { quota = 0, usage = 0 } = await navigator.storage.estimate();
      if (quota && quota - usage < needBytes) {
        return {
          ok: false,
          why: `only ${Math.floor((quota - usage) / MB)} MB of storage are free here (${Math.ceil(needBytes / MB)} MB needed)`,
        };
      }
    } catch (e) {
      // No estimate: try, and fall back if the write fails.
    }
    return { ok: true };
  }

  async function removeStored() {
    const dir = await storeDir(false);
    if (!dir) return;
    for (const name of [STORE.marker, STORE.pack]) {
      for (let attempt = 0; attempt < 15; attempt++) {
        try {
          await dir.removeEntry(name);
          break;
        } catch (e) {
          if (e && e.name === "NotFoundError") break;
          // Still locked by a worker that was just stopped: give the browser a moment.
          await new Promise((r) => setTimeout(r, 200));
        }
      }
    }
  }

  // The stored pack as a File, or null. Anything half-written or left by an interrupted import is removed.
  async function findStored() {
    const dir = await storeDir(false);
    if (!dir) return null;
    let marker = null;
    let pack = null;
    try {
      marker = JSON.parse(await (await (await dir.getFileHandle(STORE.marker)).getFile()).text());
    } catch (e) {}
    try {
      pack = await (await dir.getFileHandle(STORE.pack)).getFile();
    } catch (e) {}
    if (marker && pack && pack.size === marker.size) {
      // A pack written by another version of the page (other format, other set of files): gone, ask again.
      if (marker.version === wasm.pack_version()) return pack;
      await removeStored();
      return { outdated: true };
    }
    if (marker || pack) {
      await removeStored();
      return { incomplete: Boolean(marker) };
    }
    return null;
  }

  // ---- saves ----------------------------------------------------------------------------------------------
  // Exactly one place: files in the private file system next to the game pack where that exists, else
  // localStorage, else nowhere (the game then says so at every save). The engine keeps each value in two
  // checksummed slots (mb_game::SlotStorage), so a write cut short by a closed tab loses nothing older.

  let savesDir = null;
  let saveChain = Promise.resolve();

  async function prepareSaves() {
    const dir = await storeDir(true);
    try {
      if (dir && "createWritable" in FileSystemFileHandle.prototype) {
        savesDir = await dir.getDirectoryHandle(STORE.saves, { create: true });
        for await (const [name, handle] of savesDir.entries()) {
          if (handle.kind !== "file" || name.endsWith(".crswap")) continue;
          wasm.preload_save(name, new Uint8Array(await (await handle.getFile()).arrayBuffer()));
        }
        wasm.set_save_mode("opfs");
        return "opfs";
      }
    } catch (e) {
      savesDir = null;
    }
    try {
      localStorage.setItem("monkey-ball-rs.probe", "1");
      localStorage.removeItem("monkey-ball-rs.probe");
      wasm.set_save_mode("local");
      return "local";
    } catch (e) {
      wasm.set_save_mode("memory");
      return "memory";
    }
  }

  // Called by the engine. Writes are queued so they land in order; each one is the browser's own
  // write-then-swap (createWritable), on top of the engine's two slots.
  window.mbSaveWrite = (key, bytes) => {
    saveChain = saveChain
      .then(async () => {
        const handle = await savesDir.getFileHandle(key, { create: true });
        const writable = await handle.createWritable();
        await writable.write(bytes);
        await writable.close();
      })
      .catch((e) => wasm.save_failed("browser storage: " + describeError(e)));
  };
  window.mbSaveRemove = (key) => {
    saveChain = saveChain.then(() => savesDir.removeEntry(key)).catch(() => {});
  };

  // What the options screen says about data on this device.
  async function storageInfo(mode, saveMode) {
    const lines = [];
    if (mode === "stored") {
      let kept = false;
      try {
        kept = navigator.storage.persisted ? await navigator.storage.persisted() : false;
      } catch (e) {}
      lines.push(
        `Game data: ${Math.round((stats.packBytes || 0) / MB)} MB in this browser` +
          (kept ? "" : " (the browser may clear it)"),
      );
    } else {
      lines.push("Game data: read from the picked file, not kept");
    }
    lines.push(
      saveMode === "opfs"
        ? "Saves: in this browser, next to the game data"
        : saveMode === "local"
          ? "Saves: in this browser (local storage)"
          : "Saves: not kept (this page has no storage)",
    );
    stats.saveMode = saveMode;
    return lines;
  }

  async function describeStorage() {
    const note = $("storage-note");
    try {
      const { quota = 0, usage = 0 } = await navigator.storage.estimate();
      const persisted = navigator.storage.persisted ? await navigator.storage.persisted() : false;
      note.textContent =
        `Storage used by this page: ${(usage / MB).toFixed(0)} MB` +
        (quota ? ` of ${(quota / MB).toFixed(0)} MB available` : "") +
        (persisted ? " (kept until you remove it)." : " (the browser may clear it when the device runs low).");
    } catch (e) {
      note.textContent = "";
    }
  }

  // ---- starting the game ----------------------------------------------------------------------------------

  async function startGame(data, mode) {
    setStatus("Starting...");
    setProgress(null);
    // The canvas must be laid out before the game starts: its CSS size becomes the window size.
    canvas.hidden = false;
    // `?scale=0.5` renders at half the device resolution (weak GPUs); `?autoscale=0` keeps the game from
    // lowering the resolution on its own when frames stay slow.
    const scale = Number(params.get("scale") || 1);
    wasm.set_render_options(scale > 0 ? scale : 1, params.get("autoscale") !== "0");
    wasm.set_caps(mode === "stored", fullscreenSupported(), tiltSupported());
    stats.mode = mode;
    wasm.set_storage_info(await storageInfo(mode, await prepareSaves()));
    const t0 = performance.now();
    try {
      // `?gl=1` skips WebGPU (for machines where the browser's WebGPU misbehaves).
      await wasm.start(data, params.get("gl") === "1"); // consumes `data`
    } catch (e) {
      canvas.hidden = true;
      throw e;
    }
    running = true;
    landing.hidden = true;
    canvas.focus();
    stats.mode = mode;
    stats.startMs = performance.now() - t0;
    watchFrames(performance.now());
    keepAwake();
    watchNavigation();
  }

  // Game data imported on an earlier visit: no picker, straight into the game.
  async function startFromStorage() {
    const found = await findStored();
    if (!found) return false;
    if (!(found instanceof Blob)) {
      // Leftovers were cleaned up. A marker without its pack means the browser dropped the data.
      if (found.outdated) {
        setStatus("This version of the page stores the game data differently. Choose the disc image once more.");
      } else if (found.incomplete) {
        setStatus(
          "The imported game data on this device was incomplete or has been removed by the browser. " +
            "Choose the disc image again.",
          true,
        );
      }
      return false;
    }
    let data;
    try {
      data = await wasm.GameData.openPack(found);
    } catch (e) {
      // Another format version, or damaged: import again.
      console.info("stored game data rejected:", describeError(e));
      await removeStored();
      setStatus("The imported game data on this device is from another version of this page. Choose the disc image again.");
      return false;
    }
    stats.packBytes = found.size;
    await startGame(data, "stored");
    return true;
  }

  // ---- importing ----------------------------------------------------------------------------------------

  function stopWorker() {
    if (worker) worker.terminate();
    worker = null;
  }

  // Stop a running import: ask the worker (it cleans up after itself between two pieces), and end it for good
  // if it does not answer. The pending runImport() promise is rejected with `cancelled` set.
  let abortImport = null;
  async function cancelImport() {
    const w = worker;
    if (w) {
      await new Promise((done) => {
        const timer = setTimeout(done, 3000);
        w.addEventListener("message", (e) => {
          if (e.data.type === "cancelled") {
            clearTimeout(timer);
            done();
          }
        });
        w.postMessage({ type: "cancel" });
      });
    }
    stopWorker();
    if (abortImport) abortImport();
    await removeStored();
  }

  // Run one import in a fresh worker. Resolves with { kind: "stored" } or { kind: "live", read, names, gameId }.
  function runImport(file, mode, onProgress) {
    stopWorker();
    worker = makeWorker();
    const w = worker;
    const pending = new Map(); // live-mode file requests
    let nextId = 0;
    return new Promise((resolve, reject) => {
      const fail = (e) => {
        for (const p of pending.values()) p.reject(e);
        pending.clear();
        reject(e);
      };
      abortImport = () => {
        const e = new Error("cancelled");
        e.cancelled = true;
        fail(e);
      };
      w.onerror = (e) => fail(new Error(`the import worker failed: ${e.message || "could not be started"}`));
      w.onmessageerror = () => fail(new Error("the import worker sent something unreadable"));
      w.onmessage = (event) => {
        const m = event.data;
        if (m.type === "opened") {
          stats.gameId = m.gameId;
          stats.region = m.region;
          stats.importBytes = m.total;
          stats.importFiles = m.files;
          onProgress("opened", 0, m.total);
        } else if (m.type === "progress") {
          onProgress(m.phase, m.done, m.total);
        } else if (m.type === "stored") {
          stats.packBytes = m.size;
          resolve({ kind: "stored" });
        } else if (m.type === "live") {
          const read = (paths) =>
            new Promise((ok, bad) => {
              const id = ++nextId;
              pending.set(id, { resolve: ok, reject: bad });
              w.postMessage({ type: "read", id, paths });
            });
          resolve({ kind: "live", read, names: m.names, gameId: m.gameId });
        } else if (m.type === "read") {
          const p = pending.get(m.id);
          pending.delete(m.id);
          if (p) m.error ? p.reject(new Error(m.error)) : p.resolve(m.files);
        } else if (m.type === "error") {
          const e = new Error(m.message);
          e.kind = m.kind;
          fail(e);
        }
      };
      w.postMessage({ type: "import", module: wasmModule, file, mode, store: STORE });
    });
  }

  async function importFile(file) {
    if (!wasmModule || busy || running) return;
    busy = true;
    pick.disabled = true;
    removeButton.hidden = true;
    cancel.hidden = false;
    const t0 = performance.now();
    let cancelled = false;
    cancel.onclick = async () => {
      cancelled = true;
      cancel.disabled = true;
      setStatus("Cancelling...");
      await cancelImport();
      cancel.disabled = false;
      setProgress(null);
      setStatus("Import cancelled. Nothing was kept.");
    };
    try {
      if (!(file.size > 0)) {
        throw new Error(
          "The file is empty. Supported: RVZ (uncompressed or Zstandard) and ISO/GCM images of Super Monkey Ball (USA/EUR/JPN).",
        );
      }
      // The first bytes say what the file is: refuse anything unsupported before spending time on it.
      const head = new Uint8Array(await file.slice(0, wasm.header_len()).arrayBuffer());
      wasm.check_header(head);
      setStatus(`Opening ${file.name}...`);
      setProgress(0);

      const onProgress = (phase, done, total) => {
        if (cancelled) return;
        if (phase === "verify") {
          setStatus("Checking the copied files...");
        } else {
          setStatus(`Copying the game's files: ${(done / MB).toFixed(0)} of ${(total / MB).toFixed(0)} MB`);
        }
        setProgress(total ? done / total : 0);
      };
      const verdict = await storageVerdict(160 * MB);
      let result = null;
      let liveWhy = verdict.ok ? "" : verdict.why;
      if (verdict.ok) {
        try {
          result = await runImport(file, "store", onProgress);
        } catch (e) {
          // Storage refused the pack (quota, no file access in this browser): play from the image instead.
          if (cancelled || e.cancelled || !(e.kind === "quota" || e.kind === "storage")) throw e;
          await removeStored();
          liveWhy = e.kind === "quota" ? "this browser ran out of storage for the game data" : describeError(e);
        }
      }
      if (cancelled) return;
      if (!result) {
        console.info("not storing the import:", liveWhy);
        setStatus("Opening the disc image...");
        result = await runImport(file, "live", onProgress);
        stats.liveWhy = liveWhy;
      }
      if (cancelled) return;
      stats.importMs = performance.now() - t0;
      cancel.hidden = true;
      if (result.kind === "stored") {
        stopWorker();
        // Ask the browser not to clear the data under storage pressure (it may say no; the options
        // screen and the landing page say which).
        if (navigator.storage.persist) stats.persisted = await navigator.storage.persist().catch(() => false);
        if (!(await startFromStorage())) throw new Error("The copied game data could not be opened again.");
      } else {
        // The worker stays: it hands out single files from the image for this session.
        await startGame(wasm.GameData.live(result.read, result.names, result.gameId), "live");
        console.info(
          `Playing from the disc image for this session (${liveWhy}); it will be asked for again next time.`,
        );
      }
      console.info(`import: ${stats.mode}, ${(stats.importMs / 1000).toFixed(1)} s`);
    } catch (e) {
      if (!cancelled && !e.cancelled) {
        console.error(e);
        stopWorker();
        await removeStored();
        setProgress(null);
        setStatus(describeError(e), true);
      }
    } finally {
      busy = false;
      cancel.hidden = true;
      if (!running) pick.disabled = false;
    }
  }

  // Remove the imported game data. Saves stay (the game has its own "erase save data").
  async function removeData() {
    stopWorker();
    await removeStored();
    location.reload();
  }

  // ---- what the game asks of the page (crates/mb-app/src/host.rs) --------------------------------------------

  const fullscreenTarget = document.documentElement;
  const fullscreenSupported = () => Boolean(fullscreenTarget.requestFullscreen && document.fullscreenEnabled !== false);

  function setFullscreen(on) {
    if (!fullscreenSupported()) return;
    if (on && !document.fullscreenElement) {
      fullscreenTarget
        .requestFullscreen({ navigationUI: "hide" })
        .then(() => {
          // Landscape suits the game best; where the lock is not allowed the player just turns the device.
          if (screen.orientation && screen.orientation.lock) screen.orientation.lock("landscape").catch(() => {});
        })
        .catch((e) => console.info("fullscreen refused:", e && e.message));
    } else if (!on && document.fullscreenElement) {
      document.exitFullscreen().catch(() => {});
    }
  }
  document.addEventListener("fullscreenchange", () => {
    if (running) wasm.set_fullscreen_state(Boolean(document.fullscreenElement));
  });

  // Device tilt: one listener, attached only while the game steers by tilt.
  const tiltSupported = () =>
    "DeviceOrientationEvent" in window && (navigator.maxTouchPoints > 0 || params.get("tilt") === "1");
  const screenAngle = () =>
    Math.round((screen.orientation && typeof screen.orientation.angle === "number" ? screen.orientation.angle : window.orientation) || 0);
  const onOrientation = (e) => {
    if (e.beta !== null && e.gamma !== null) wasm.device_orientation(e.beta, e.gamma, screenAngle());
  };
  let tiltOn = false;
  function setTilt(on) {
    if (on === tiltOn) return;
    tiltOn = on;
    if (!on) {
      window.removeEventListener("deviceorientation", onOrientation);
      return;
    }
    window.addEventListener("deviceorientation", onOrientation);
    // iOS hands out orientation only after a permission prompt, which must come from a tap.
    const ask = window.DeviceOrientationEvent && DeviceOrientationEvent.requestPermission;
    if (typeof ask === "function") {
      canvas.addEventListener("pointerup", () => ask.call(DeviceOrientationEvent).catch(() => {}), { once: true });
    }
  }

  window.mbHost = (command, argument) => {
    if (command === "remove-data") removeData();
    else if (command === "fullscreen") setFullscreen(Boolean(argument));
    else if (command === "tilt") setTilt(Boolean(argument));
  };

  // Keep the screen on while the game is in front (released by the browser whenever the page is hidden).
  function keepAwake() {
    if (!("wakeLock" in navigator)) return;
    const acquire = () => {
      if (document.visibilityState === "visible") navigator.wakeLock.request("screen").catch(() => {});
    };
    document.addEventListener("visibilitychange", acquire);
    acquire();
  }

  // The browser's back gesture (the system one on a phone) goes back inside the game while there is somewhere to go:
  // one extra history entry exists exactly while the game is in a sub-menu or a stage, so "back" pops it
  // (and the game takes one step back) instead of leaving the page. At the title menu there is no entry
  // and "back" leaves the page as usual.
  function watchNavigation() {
    let entry = false;
    let ignorePop = false;
    try {
      window.addEventListener("popstate", () => {
        if (ignorePop) {
          ignorePop = false;
          return;
        }
        if (entry) {
          entry = false;
          wasm.nav_back();
        }
      });
      setInterval(() => {
        const deep = wasm.nav_depth() > 0;
        if (deep && !entry) {
          history.pushState({ mb: 1 }, "");
          entry = true;
        } else if (!deep && entry) {
          entry = false;
          ignorePop = true;
          history.back();
        }
      }, 200);
    } catch (e) {
      // No history here (a sandboxed frame): the on-screen Back button is all there is.
    }
  }

  // ---- errors after the game has started ----------------------------------------------------------------

  // Fatal errors (called from Rust, see crates/mb-web): the engine has stopped. Game data problems never
  // get here: a stage that cannot be loaded is reported inside the game.
  window.mbOnError = (msg) => {
    $("fatal-text").textContent = String(msg);
    $("fatal-remove").hidden = stats.mode !== "stored";
    $("fatal").hidden = false;
  };
  // With the game data on the device a restart is quick: reload, and the title menu is back.
  $("reload").addEventListener("click", () => location.reload());
  $("fatal-remove").addEventListener("click", removeData);
  removeButton.addEventListener("click", removeData);

  // A lost WebGL context takes every texture and buffer with it. preventDefault keeps the browser from giving
  // up on the canvas; the engine does not rebuild its GPU state in place, so offer the restart.
  canvas.addEventListener("webglcontextlost", (e) => {
    e.preventDefault();
    window.mbOnError(
      "The browser took the graphics context away (the device ran low on graphics memory, or the GPU was reset).",
    );
  });

  // ---- frame timing -----------------------------------------------------------------------------------------
  // Recorded inside the engine once enabled (`?fps=1`, or `mbPerf.enable()` from the console):
  //   mbPerf.summary()  rolling window (about the last 1000 frames): avg / p50 / p99 / worst frame interval,
  //                     update and render CPU time, sim ticks per frame, draw calls
  //   mbPerf.reset()    start a fresh window        mbPerf.frames()  raw [interval, update, render, ticks] rows
  //   mbPerf.log()      print the summary and the interval histogram to the console
  //   mbPerf.lastLoad() where the time of the last stage load went (reads, LZSS, parsing, textures, meshes)
  window.mbPerf = {
    enable: (on = true) => wasm.perf_enable(on),
    reset: () => wasm.perf_reset(),
    summary: () => {
      const text = wasm.perf_json();
      return text ? JSON.parse(text) : null;
    },
    lastLoad: () => {
      const text = wasm.perf_last_load();
      return text ? JSON.parse(text) : null;
    },
    frames: () => {
      const flat = wasm.perf_frames();
      const rows = [];
      for (let i = 0; i + 3 < flat.length; i += 4) rows.push([flat[i], flat[i + 1], flat[i + 2], flat[i + 3]]);
      return rows;
    },
    log: () => {
      const s = window.mbPerf.summary();
      if (!s) return console.info("frame timing is off (add ?fps=1 or call mbPerf.enable())");
      console.info(perfLines(s).join("\n"));
    },
    // Bytes of game files held in memory right now, and files read without being prepared (always none).
    preparedBytes: () => wasm.prepared_bytes(),
    misses: () => wasm.content_misses(),
    wasmMemoryBytes: () => wasm.wasm_memory_bytes(),
    audio: () => JSON.parse(wasm.audio_debug_json()),
    navDepth: () => wasm.nav_depth(),
    // What the game is showing, one frame old on the first call ("menu: title", "loading stage 3", ...).
    status: () => wasm.game_status(),
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
    if (showFps) wasm.perf_enable(true);
    let first = false;
    const tick = () => {
      const now = performance.now();
      if (!first && wasm.frame_count() > 0) {
        first = true;
        stats.firstFrameMs = now - tStart;
        stats.gpu = wasm.gpu_info();
        console.info(`first frame after ${(now - tStart).toFixed(0)} ms on ${wasm.gpu_info()}`);
      }
      if (showFps) {
        const s = window.mbPerf.summary();
        if (s && !s.frames) {
          // Only frames of play are recorded; a still menu draws nothing at all.
          fpsEl.textContent = "no frames of play yet | " + wasm.gpu_info();
        } else if (s) {
          stats.fps = 1000 / s.avg;
          fpsEl.textContent =
            perfLines(s).join("\n") +
            `\n${canvas.width}x${canvas.height} (scale ${wasm.render_quality().toFixed(2)}) | ${wasm.gpu_info()}`;
        }
      } else if (first) {
        return; // nothing left to report
      }
      setTimeout(tick, showFps ? 500 : 100);
    };
    tick();
  }

  // ---- picking the file -------------------------------------------------------------------------------------

  pick.addEventListener("click", () => fileInput.click());
  fileInput.addEventListener("change", () => {
    const file = fileInput.files && fileInput.files[0];
    fileInput.value = "";
    if (file) importFile(file);
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
    if (file && !running) importFile(file);
  });

  // Gamepads are polled only while one is connected.
  const padsChanged = () =>
    wasm.set_gamepad_present(Array.from(navigator.getGamepads ? navigator.getGamepads() : []).some((p) => p && p.connected));
  window.addEventListener("gamepadconnected", () => wasmModule && padsChanged());
  window.addEventListener("gamepaddisconnected", () => wasmModule && padsChanged());

  // Right-drag looks around in the game; keep the context menu out of the way. Clicking gives keyboard focus.
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  canvas.addEventListener("pointerdown", () => canvas.focus());

  // ---- start-up -----------------------------------------------------------------------------------------

  // Offline start for the hosted and installed page: a small service worker serves the bundle from a cache.
  // The page does not depend on it (an app's WebView or the single-file build have none).
  function registerServiceWorker() {
    if (!("serviceWorker" in navigator) || !window.isSecureContext || $("mb-wasm")) return;
    if (!/^https?:$/.test(location.protocol)) return;
    navigator.serviceWorker.register("./sw.js").catch((e) => console.info("no service worker:", e && e.message));
  }

  async function boot() {
    if (typeof WebAssembly !== "object") {
      setStatus("This browser does not support WebAssembly.", true);
      return;
    }
    try {
      wasmModule = await compileEngine();
      await wasm({ module_or_path: wasmModule });
    } catch (e) {
      console.error(e);
      setStatus("The engine could not be loaded (" + describeError(e) + ").", true);
      return;
    }
    registerServiceWorker();
    padsChanged();
    setStatus("");
    try {
      if (await startFromStorage()) return;
    } catch (e) {
      console.error(e);
      setStatus(describeError(e), true);
      removeButton.hidden = false;
    }
    pick.disabled = false;
    if (!window.isSecureContext || !(navigator.storage && navigator.storage.getDirectory)) {
      $("step-keep").textContent =
        "The game reads what it needs straight from the file while you play. This page cannot keep data on " +
        "the device (it was opened from a file or over plain http, or the browser has no private storage), " +
        "so it asks for the file again next time.";
    } else {
      describeStorage();
    }
  }

  boot();
})();
