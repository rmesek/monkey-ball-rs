// The import worker. Classic worker script: the page starts it from this file, or (single-file build) from a
// Blob that already contains the engine's glue script in front of this code.
//
// It receives the user's disc image as a File and reads it with synchronous Blob slices (FileReaderSync),
// which only workers have; the engine's disc and RVZ readers (the same wasm module the page runs) do the
// parsing. Two jobs:
//   "store": copy the files the game uses into one pack file in the Origin Private File System, check the
//            copy, and mark it complete. The page then reads the pack by itself; this worker ends.
//   "live":  where that storage is unavailable, stay alive and hand out single files from the image when the
//            game asks for them. Nothing is kept; the image is asked for again on the next visit.
// Either way the image is never held in memory: reads are a megabyte or one RVZ group at a time.
"use strict";

if (typeof wasm_bindgen === "undefined") importScripts("./pkg/mb_web.js");

let job = null; // live mode: the open image
let cancelled = false; // the page asked to stop a "store" import

// Let queued messages (a cancel) be handled in the middle of a long copy.
const breathe = () => new Promise((resolve) => setTimeout(resolve, 0));

const post = (message, transfer) => self.postMessage(message, transfer || []);

function fail(e, kind) {
  const quota = e && e.name === "QuotaExceededError";
  post({
    type: "error",
    kind: quota ? "quota" : kind || (e && e.kind) || "image",
    message: typeof e === "string" ? e : (e && e.message) || String(e),
  });
}

function storageError(message) {
  const e = new Error(message);
  e.kind = "storage";
  return e;
}

async function storeImport(store) {
  let dir;
  try {
    const root = await navigator.storage.getDirectory();
    dir = await root.getDirectoryHandle(store.dir, { create: true });
    // An unfinished import from before: start clean.
    await dir.removeEntry(store.marker).catch(() => {});
  } catch (e) {
    throw storageError("this browser provides no private file storage here");
  }
  const fileHandle = await dir.getFileHandle(store.pack, { create: true });
  if (typeof fileHandle.createSyncAccessHandle !== "function") {
    throw storageError("this browser cannot write files from a worker");
  }
  let handle;
  try {
    handle = await fileHandle.createSyncAccessHandle();
  } catch (e) {
    throw storageError("the game data file is in use (is the game open in another tab?)");
  }
  let size;
  try {
    handle.truncate(0);
    const write = (bytes, at) => {
      const written = handle.write(bytes, { at });
      if (written !== bytes.byteLength) {
        const e = new Error("the browser's storage is full");
        e.kind = "quota";
        throw e;
      }
    };
    const total = job.totalBytes();
    let last = 0;
    while (job.step(write)) {
      const now = performance.now();
      if (now - last > 100) {
        last = now;
        post({ type: "progress", phase: "copy", done: job.doneBytes(), total });
        await breathe();
        if (cancelled) break;
      }
    }
    if (cancelled) {
      // Leave nothing behind: empty the file while it is still ours, then delete it.
      handle.truncate(0);
      handle.close();
      handle = null;
      await dir.removeEntry(store.pack).catch(() => {});
      post({ type: "cancelled" });
      return;
    }
    size = job.finish(write);
    job.free();
    job = null;
    handle.flush();
    // Read the whole pack back and compare checksums before calling it complete.
    post({ type: "progress", phase: "verify", done: total, total });
    const read = (offset, length) => {
      const bytes = new Uint8Array(length);
      const got = handle.read(bytes, { at: offset });
      return got === length ? bytes : bytes.subarray(0, got);
    };
    wasm_bindgen.verify_pack(read, handle.getSize());
  } finally {
    if (handle) handle.close();
  }
  // The marker makes the import count. Written last.
  const marker = await dir.getFileHandle(store.marker, { create: true });
  const markerHandle = await marker.createSyncAccessHandle();
  try {
    const text = new TextEncoder().encode(JSON.stringify({ size, imported: new Date().toISOString() }));
    markerHandle.truncate(0);
    markerHandle.write(text, { at: 0 });
    markerHandle.flush();
  } finally {
    markerHandle.close();
  }
  post({ type: "stored", size });
}

self.onmessage = async (event) => {
  const m = event.data;
  try {
    if (m.type === "cancel") {
      cancelled = true;
    } else if (m.type === "import") {
      await wasm_bindgen({ module_or_path: m.module });
      const reader = new FileReaderSync();
      const file = m.file;
      const read = (offset, length) => new Uint8Array(reader.readAsArrayBuffer(file.slice(offset, offset + length)));
      job = wasm_bindgen.ImportJob.open(read, file.size);
      post({
        type: "opened",
        gameId: job.gameId(),
        region: job.region(),
        total: job.totalBytes(),
        files: job.paths().length,
      });
      if (m.mode === "store") {
        await storeImport(m.store);
      } else {
        post({ type: "live", names: job.paths(), gameId: job.gameId() });
      }
    } else if (m.type === "read") {
      try {
        const files = m.paths.map((path) => job.readFile(path));
        post({ type: "read", id: m.id, files }, files.map((f) => f.buffer));
      } catch (e) {
        post({ type: "read", id: m.id, error: typeof e === "string" ? e : (e && e.message) || String(e) });
      }
    }
  } catch (e) {
    fail(e);
  }
};
