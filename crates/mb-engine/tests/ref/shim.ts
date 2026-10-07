// Minimal browser-global stand-ins so the reference game modules can be imported and instantiated headless.
const g: any = globalThis;
const dummy: any = new Proxy(function () {}, {
  get: (_t, k) => (k === Symbol.toPrimitive ? () => 0 : k === "then" ? undefined : dummy),
  set: () => true,
  apply: () => dummy,
  construct: () => dummy,
});
for (const n of ["WebGLRenderingContext", "WebGL2RenderingContext", "HTMLElement", "HTMLCanvasElement", "Image", "Audio", "AudioContext"]) {
  if (!g[n]) g[n] = dummy;
}
if (!g.window) g.window = g;
g.document = dummy;
try { g.navigator = { userAgent: "deno", getGamepads: () => [], maxTouchPoints: 0 }; } catch { /* read-only navigator */ }
g.localStorage = { getItem: () => null, setItem: () => {}, removeItem: () => {} };
