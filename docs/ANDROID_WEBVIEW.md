# Putting the site inside an Android app (WebView)

The static site built into `web/dist/` is the whole game for the browser. An Android app can show the same
files in a `WebView`. This note is for whoever writes that app. Nothing in this repository builds an APK,
and **nothing here was run on Android**: there is no Android SDK or device on the machine this was written
on. (A native Android build of the Rust crates, without a web view, is a later target of the project and
a different thing; `docs/ARCHITECTURE.md` says what it needs and compares the two.) Each statement below is marked

* **[checked]**: verified here, in desktop Chrome 154, by `scripts/web-e2e.ts` or by reading the code;
* **[platform]**: from the Android and Chromium documentation as remembered, to be confirmed on a device.

## What goes into the APK

Copy the contents of `web/dist/` unchanged into the app's assets, in a folder of its own, for example
`app/src/main/assets/site/`. Every reference in the site is relative, so the folder name is free
**[checked]**: the browser test serves the bundle from `/assets/site/` and verifies that no request leaves
that folder).

| File | Needed in an app |
|---|---|
| `index.html`, `guard.js`, `main.js`, `worker.js`, `pkg/mb_web.js`, `pkg/mb_web_bg.wasm` | yes |
| `sw.js`, `manifest.webmanifest`, `icon*.png`, `icon.svg` | no (offline start and installation for browsers); harmless if left in |

There is no native code: no NDK, no `.so` files, no ABI splits, nothing affected by the 16 KB page size
requirement. The game is one 4.0 MB WebAssembly module and about 250 KB of script and markup.

## Serve the assets with `WebViewAssetLoader`, not `file://`

**[platform]** Use `androidx.webkit.WebViewAssetLoader` with an `AssetsPathHandler` and load
`https://appassets.androidplatform.net/assets/site/index.html`. The loader answers requests from
`WebViewClient.shouldInterceptRequest`, including the page's `fetch` of the wasm module and the worker
script. It takes the path from the URL, so the `?v=<hash>` the page appends to its own files does no harm.

Why not `file:///android_asset/site/index.html`:

* That address is not a secure context. The page then has no private file system: it asks for the game file
  at every launch and plays from it for the session (see "Without a secure context").
* A page on `file://` may not `fetch` the wasm file or start `worker.js` from a neighbouring file unless
  `allowFileAccessFromFileURLs` is switched on, which lets any page content read the app's files. Do not.

MIME types as the loader guesses them from the extension: `.js` must come out as a JavaScript type (it
does) because the worker script is checked. `.wasm` should be `application/wasm`; if it is not, the page
compiles from the bytes instead of streaming (**[checked]**, same path as a mislabelling web server).
`.webmanifest` does not matter in a web view.

## Settings

**[platform]** unless marked.

| `WebSettings` | Value | Why |
|---|---|---|
| `javaScriptEnabled` | `true` | required |
| `domStorageEnabled` | `true` | `localStorage` is the fallback place for saves. With it off the page still runs (**[checked]** with `localStorage` returning `null`: saves then last until the page closes, and the game says so at every save) |
| `mediaPlaybackRequiresUserGesture` | `false`, or leave the default | With the default the sound starts at the first touch (the page resumes its `AudioContext` on `pointerdown`/`touchend`/`keydown`, **[checked]**); `false` lets it start with the game |
| `allowFileAccess`, `allowContentAccess` | `false` | The page never opens `file://` or `content://` addresses itself. The picked game file arrives through the file chooser as a `File` object |
| `allowFileAccessFromFileURLs`, `allowUniversalAccessFromFileURLs` | `false` (the default) | not needed with the asset loader |
| `setSupportZoom` | `false` | the page already disables zoom, scrolling and text selection on the game canvas (**[checked]** in the markup) |

Do not add a JavaScript interface (`addJavascriptInterface`): the page needs none.

The page makes no request to any other host: its Content-Security-Policy (a `<meta>` in `index.html`, which
a web view honours like a browser) allows only its own origin and forbids forms and `<base>`
(**[checked]**). The app may additionally answer every request that is not for
`appassets.androidplatform.net` with an empty error response in `shouldInterceptRequest`.

Keep hardware acceleration on (the default): the game draws with WebGL2, which is the only backend in a web
view. Without WebGL2 the page says so and does not start (**[checked]** with WebGL taken away).

## The file chooser

The page asks for the user's disc image with `<input type="file">`. A `WebView` shows nothing for that by
itself. **[platform]**

```kotlin
override fun onShowFileChooser(view: WebView, callback: ValueCallback<Array<Uri>>, params: FileChooserParams): Boolean {
    pending?.onReceiveValue(null)          // an earlier request that never got an answer
    pending = callback
    pickFile.launch(arrayOf("*/*"))        // ActivityResultContracts.OpenDocument()
    return true
}
// in the result callback:  pending?.onReceiveValue(if (uri != null) arrayOf(uri) else null); pending = null
```

* Ask for `*/*`. Do not use `params.createIntent()` or the accept types: `.rvz` and `.gcm` have no
  registered media type and a filter built from them can make the right file unselectable. (For the same
  reason the page drops its own extension filter on touch devices, **[checked]** in `main.js`.) The page
  judges the file by its content and says exactly what is wrong with a wrong one.
* Always answer the callback, with `null` when the user cancels, or the input stays dead.
* The page reads the file in slices through the content URI; the 475 MB file is never loaded whole
  (**[checked]**: the import adds about 130 MB to the browser's memory at its peak on desktop).
* When the page can store the game data (see below) it needs the file only during the import, a few
  seconds. When it cannot, it keeps reading from the file for the whole session, so the URI permission must
  last as long as the activity. If the file disappears or the permission ends, the page shows "The game
  file can no longer be read" with one button that returns to the chooser (**[checked]** by deleting the
  file under a running session).

## Storage

**[platform]** A web view keeps the page's storage in the app's private data directory (`app_webview/`).
It is removed with the app or by "Clear storage", and no other app can read it.

* Game data: one 144 MB file in the Origin Private File System, written once at import. Whether the Origin
  Private File System is available in a given WebView version, and what quota `navigator.storage.estimate()`
  reports there, has to be seen on a device. The page checks the quota before the import (it wants 160 MB
  free) and handles a refusal or a write that fails part-way: it then plays from the picked file and the
  options screen says why nothing is kept (**[checked]** with the file system taken away).
* Saves: small files next to the game data; `localStorage` where the file system is missing; memory only
  where both are.
* The player removes the game data from the game's options screen ("Remove game data").
* **Exclude `app_webview` from Android's automatic backup** (`android:dataExtractionRules` /
  `android:fullBackupContent`), or the 144 MB of game data are uploaded to the user's cloud backup. The site
  itself never sends anything anywhere.

## Back button

The page keeps one history entry while the game is in a sub-menu or a stage, and none at the title
(**[checked]**). So:

```kotlin
onBackPressedDispatcher.addCallback(this) { if (webView.canGoBack()) webView.goBack() else finish() }
```

Alternatively call `webView.evaluateJavascript("window.mbBack ? mbBack() : false") { handled -> ... }`:
`mbBack()` takes one step back inside the game and returns `false` at the title, where the app should act.

## Pause, resume, losing the renderer

* **[platform]** Forward `onPause()` and `onResume()` of the activity to the `WebView`. That makes the page
  hidden and visible. **[checked]** (by hiding the tab in Chrome, which raises the same event): while hidden
  the page produces no frames, its audio clock is stopped, a running stage is paused, and time away is not
  counted as game time; when shown again it redraws and the audio resumes.
* **[platform]** The Screen Wake Lock API is not available in a web view. Keep the screen on from the app
  (`view.keepScreenOn = true`) while the game is in front.
* **[platform]** Implement `WebViewClient.onRenderProcessGone` (API 26): return `true`, remove and destroy
  the `WebView`, create a new one and load the page again. Nothing is lost that was saved: progress is
  written when a stage ends, not at exit, in two checksummed slots, and the game data on the device is
  either complete or ignored. **[checked]** (closing the tab mid-play and mid-import): a fresh page is back
  at the title menu about 0.1 s after the engine is compiled, without asking for the file.
* A lost WebGL context shows a dialog with a restart button (the page reloads itself).

## Fullscreen, orientation, sensors, gamepads

* **[platform]** `requestFullscreen` only works in a web view whose `WebChromeClient` implements
  `onShowCustomView`. An app is normally immersive by itself; where the page finds no fullscreen support it
  hides its own fullscreen option (**[checked]**).
* The game works in portrait and landscape and follows rotation mid-stage. Fix the orientation in the
  manifest if you want one. `screen.orientation.lock` is not expected to work in a web view; the page
  ignores its failure.
* Tilt control uses `deviceorientation` events. **[platform]** They need a secure context, which the asset
  loader gives. The option is offered only when the device reports the event type.
* Gamepads are read through the Gamepad API when one is connected.

## Versions

The page needs **Android System WebView 97** or newer and says so in words on anything older instead of
stopping silently (**[checked]** by simulating engines that lack a feature). What sets the number:

| Feature the build uses | Chrome / WebView | Safari | Firefox |
|---|---|---|---|
| `'wasm-unsafe-eval'` in the Content-Security-Policy (older engines refuse to compile WebAssembly under the policy) | 97 | 16 | 102 (older versions did not apply the policy to WebAssembly) |
| WebAssembly reference types (two tables in the module) | 96 | 15 | 79 |
| WebAssembly bulk memory, non-trapping conversions, sign extension, multi-value | 75 / 75 / 74 / 85 | 15 / 15 / 14.1 / 13.1 | 79 / 64 / 62 / 78 |
| WebGL2 | 56 | 15 | 51 |
| Storing the game data (Origin Private File System with synchronous access handles) | 102 (108 for the all-synchronous form; the code awaits the calls, so both shapes work) | 15.2 | 111 |

The version numbers in this table are from the browsers' documentation, not from tests.
**[platform]** WebView updates through the Play Store on Android 5 and later, but the last version for
Android 5 is 95, so the practical floor is Android 6 (API 23), whose WebView stops at 106: enough to play
and, with the awaited calls, to store. Only Chrome 154 was run here.

## Memory, for small devices

**[checked]** in desktop Chrome 154 on WebGL2 (version 0.2.0, `scripts/web-e2e.ts`) unless a row says
otherwise, so indicative only; nothing was measured in a web view.

| | Size |
|---|---|
| WebAssembly memory while playing (it grows on demand from 1.4 MB and never shrinks) | 19.1 MB with one stage loaded, whichever it is and with Reflections on or off; 23.5 MB after several loads in one session (0.1.0: 22 to 28 MB) |
| Music of the current stage, held by the browser as float samples | 24 MB (`st1`, measured) to 54 MB (water stages, computed from the stream sizes) |
| Sound effects, as float samples | 3.3 MB (measured for 0.1.0; not taken again) |
| Textures on the GPU, heaviest stage (stage 109, space background: 82 textures, RGBA8 with mip levels) | 16.7 MB; typical 6 to 15 MB (counted by the loader in the desktop tests) |
| Mirror floors (37 stages, only with Options -> Reflections on, which is the default): one more render target, colour and depth at half the frame's size each way | 2 bytes per pixel of the frame: 1.84 MB under 1280 x 720 (computed) |
| JavaScript heap | 2 MB (measured for 0.1.0; not taken again) |
| Extra memory during the one-time import | about 130 MB at the peak, released afterwards (measured for 0.1.0; not taken again) |

**Reflections on a weak device.** The mirror floors draw what they show a second time. **[checked]** in
desktop Chrome with the CPU slowed six times, on WebGL2 (the only backend in a web view): the heaviest
mirror stages take 9.4 to 12.5 ms of a 16.6 ms frame at the 99th percentile, 4 to 6 ms of it for the mirror,
and no WebAssembly memory beyond the figure above (`docs/WEB.md`, "Mirror floors in the browser"). The
player turns it off in Options -> Reflections; the choice is saved. **[platform]** How a phone's GPU copes
with the second target was not measured.

**WebGL2 in a web view.** **[platform]** The engine waits for the graphics driver after each submission
(`glFinish` in place of polling a sync object) so that memory used by a load is given back during the load.
That was run in desktop Chrome only (no warning in its log, frame pacing unchanged); how Android WebView's
driver takes it has to be seen on a device.

The largest single allocations inside the WebAssembly memory are the files of the stage being loaded (up to
10.6 MB, released after the load) and one music stream (up to 4.9 MB, released once decoded).

## Without a secure context

Applies to `file://` and to plain `http://` on a network address.

* No Origin Private File System: the game data cannot be kept; the user picks the file at every launch and
  the page reads from it while playing.
* Saves go to `localStorage` (if DOM storage is enabled), otherwise they last until the page closes.
* No service worker (irrelevant in an app), no WebGPU (WebGL2 is used anyway).
* Device tilt may be unavailable.
