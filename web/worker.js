// The import worker. Classic worker script: the page starts it from this file.
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

// (The page passes the build's version in this script's URL; the glue is fetched with the same one.)
importScripts("./pkg/mb_web.js" + self.location.search);

let job = null; // live mode: the open image
let readFailed = false; // the browser could not read the picked file (it is gone, or access to it has ended)
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
  const gameId = job.gameId();
  let handle;
  try {
    handle = await fileHandle.createSyncAccessHandle();
  } catch (e) {
    throw storageError("the game data file is in use (is the game open in another tab?)");
  }
  let size;
  try {
    // (Chrome 102 to 107 had truncate, flush, getSize and close return promises; awaiting covers both.)
    await handle.truncate(0);
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
      await handle.truncate(0);
      await handle.close();
      handle = null;
      await dir.removeEntry(store.pack).catch(() => {});
      post({ type: "cancelled" });
      return;
    }
    size = job.finish(write);
    job.free();
    job = null;
    await handle.flush();
    // Read the whole pack back and compare checksums before calling it complete.
    post({ type: "progress", phase: "verify", done: total, total });
    const read = (offset, length) => {
      const bytes = new Uint8Array(length);
      const got = handle.read(bytes, { at: offset });
      return got === length ? bytes : bytes.subarray(0, got);
    };
    wasm_bindgen.verify_pack(read, await handle.getSize());
  } finally {
    if (handle) await handle.close();
  }
  // The marker makes the import count. Written last.
  const marker = await dir.getFileHandle(store.marker, { create: true });
  const markerHandle = await marker.createSyncAccessHandle();
  try {
    const text = new TextEncoder().encode(
      JSON.stringify({ size, version: wasm_bindgen.pack_version(), gameId, imported: new Date().toISOString() }),
    );
    await markerHandle.truncate(0);
    markerHandle.write(text, { at: 0 });
    await markerHandle.flush();
  } finally {
    await markerHandle.close();
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
      if (typeof FileReaderSync !== "function") {
        throw new Error("This browser cannot read a local file from a worker (no FileReaderSync), which the game needs to read the disc image.");
      }
      const reader = new FileReaderSync();
      const file = m.file;
      const read = (offset, length) => {
        try {
          return new Uint8Array(reader.readAsArrayBuffer(file.slice(offset, offset + length)));
        } catch (e) {
          readFailed = true;
          throw e;
        }
      };
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
      readFailed = false;
      try {
        const files = m.paths.map((path) => job.readFile(path));
        post({ type: "read", id: m.id, files }, files.map((f) => f.buffer));
      } catch (e) {
        // `gone`: not a damaged image but a file the browser can no longer read at all.
        post({ type: "read", id: m.id, gone: readFailed, error: typeof e === "string" ? e : (e && e.message) || String(e) });
      }
    }
  } catch (e) {
    fail(e);
  }
};
