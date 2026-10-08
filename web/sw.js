// Service worker for the hosted / installed page: keeps the bundle in a cache so the page starts with no
// network at all. Cache first, same origin only, GET only. It never sees the game data (that lives in the
// Origin Private File System and is read by the page directly) and the page works the same without it.
//
// scripts/build-web fills in the two constants: the version is a hash of the bundle, so a new build installs
// a new cache and drops the old one.
"use strict";

const VERSION = "__MB_VERSION__";
const FILES = __MB_FILES__;
const CACHE = "monkey-ball-" + VERSION;

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(FILES.map((f) => new Request(f, { cache: "reload" }))))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith("monkey-ball-") && k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET" || new URL(request.url).origin !== self.location.origin) return;
  event.respondWith(
    caches.open(CACHE).then(async (cache) => {
      // The page itself may be asked for with a query string (?gl=1, ?fps=1): the bundle has one page.
      const hit = (await cache.match(request)) || (request.mode === "navigate" ? await cache.match("./") : null);
      return hit || fetch(request);
    }),
  );
});
