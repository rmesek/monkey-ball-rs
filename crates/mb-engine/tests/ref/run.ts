// Headless driver for the WebMonkeyBall reference simulation (read-only use of the reference sources).
//
//   deno run --allow-read --allow-write --sloppy-imports --allow-env --import-map import_map.json run.ts \
//        <reference src dir> <stage id> <decompressed-or-lz stage file> <inputs file> <frames> <out file> \
//        [replay|noreplay] [tail ticks]
//
// Runs the real `Game` (fixed-tick mode, no HUD/audio) with a scripted stick feed and writes one line per tick:
//   F tick hash px py pz vx vy vz state flags rotX rotY rotZ apeYaw camRotY intro stageTimer goalTimer ringout xrot zrot
//     visualHash effects confetti effectsState confettiState visualRng
//     camEyeX camEyeY camEyeZ camLookX camLookY camLookZ camRotX camRotZ camTimer
//     ringoutSkip goalSkip timeover
// f64 values are the raw IEEE bits as 16 hex digits. `visualHash` is `hashSimState(.., { includeVisual: true })`;
// `effectsState` / `confettiState` are FNV hashes over every field of every live particle (see `effectsState`).
//
// `replay` (default): a final `END reason tick` line is written when the reference leaves the plain gameplay
// loop (result replay, stage reload, course advance).
// `noreplay`: result replays are switched off (`autoRecordInputs = false`), so a fall-out or a time-over runs
// to its end and reloads the stage. The reload is awaited, a `RELOAD tick` line is written and the run goes on
// as the next attempt with the stick feed from its start. A goal sequence that ends writes `END goal-finished
// tick`; after it the stage runtime alone is advanced `tail` ticks (one `T k visualHash effects confetti
// effectsState confettiState visualRng` line each), which is how the confetti is followed to its end.
//
// A line of the inputs file is `x y` or `x y buttons`. With the third column the feed frame carries `buttons`,
// and its bit 0 is what the game's input device reports as the primary action during that tick: the reference
// asks the device (`input.isPrimaryActionDown()`), not the feed, whether to fast-forward the intro, to end a
// fall-out sequence or to leave a goal sequence. `ringoutSkip` and `goalSkip` are the two timers that gate the
// last two (`ringoutSkipTimerFrames`, `goalSkipTimerFrames`), `timeover` is `timeoverTimerFrames`.
import "./shim.ts";

const [srcDir, stageIdArg, stageFile, inputsFile, framesArg, outFile, modeArg, tailArg] = Deno.args;
const stageId = Number(stageIdArg);
const frames = Number(framesArg);
const noReplay = modeArg === "noreplay";
const tail = Number(tailArg ?? 0);
const base = "file:///" + srcDir.split("\\").join("/").replace(/\/?$/, "/");

const stageBytes = Deno.readFileSync(stageFile);
const id3 = String(stageId).padStart(3, "0");
(globalThis as any).fetch = async (url: string) => {
  if (String(url).endsWith(`st${id3}/STAGE${id3}.lz`)) {
    return { ok: true, status: 200, statusText: "OK", arrayBuffer: async () => stageBytes.buffer.slice(stageBytes.byteOffset, stageBytes.byteOffset + stageBytes.byteLength) };
  }
  return { ok: false, status: 404, statusText: "not served", arrayBuffer: async () => { throw new Error("404"); } };
};
console.warn = () => {};

const feed = Deno.readTextFileSync(inputsFile).trim().split("\n").filter((l) => l.length).map((l) => {
  const [x, y, buttons] = l.split(" ").map(Number);
  return buttons === undefined ? { x, y } : { x, y, buttons };
});

const { Game } = await import(base + "game.ts");
const { hashSimState } = await import(base + "sim_hash.ts");

const game = new Game();
game.init();
game.setFixedTickMode(true, 1);
if (noReplay) game.autoRecordInputs = false;
await game.loadStage(stageId);
game.setInputFeed(feed);
game.paused = false;
game.simPerf.enabled = false;
game.rollbackPerf.enabled = false;
// Ticks done in the current attempt, which is the index of the feed frame the next tick consumes.
let attemptTick = 0;
if (feed.some((f: any) => f.buttons !== undefined)) {
  game.input.isPrimaryActionDown = () => ((feed[attemptTick] as any)?.buttons & 1) === 1;
}

const f64 = new DataView(new ArrayBuffer(8));
const bits = (v: number) => {
  f64.setFloat64(0, v);
  return f64.getBigUint64(0).toString(16).padStart(16, "0");
};
const hex = (v: number) => (v >>> 0).toString(16).padStart(8, "0");

// FNV-1a over 32-bit words: integers as they are, floats as the two halves of their f64 bits (high first).
const word = (h: number, v: number) => Math.imul(h ^ (v >>> 0), 16777619) >>> 0;
const int = (h: number, v: number) => word(h, v | 0);
const num = (h: number, v: number) => {
  f64.setFloat64(0, v);
  return word(word(h, f64.getUint32(0)), f64.getUint32(4));
};
const vec = (h: number, v: any) => num(num(num(h, v.x), v.y), v.z);
const KINDS = ["coli", "colistar", "coliflash", "levitate"];

// Every field of every ball effect except `id` (the reference counts ids across stage loads).
function effectsState(effects: any[]) {
  let h = 0x811c9dc5;
  for (const e of effects) {
    h = int(h, KINDS.indexOf(e.kind));
    for (const v of [e.pos, e.prevPos, e.groundPos, e.groundNormal, e.groundVel, e.glowPos, e.surfaceNormal, e.vel]) h = vec(h, v);
    for (const v of [e.glowDist, e.scale, e.scaleTarget, e.alpha, e.colorR, e.colorG, e.colorB, e.baseY]) h = num(h, v);
    for (const v of [e.glowRotX, e.glowRotY, e.hasGround ? 1 : 0, e.life, e.age, e.rotX, e.rotY, e.rotZ, e.prevRotX, e.prevRotY, e.prevRotZ, e.rotVelX, e.rotVelY, e.rotVelZ]) h = int(h, v);
  }
  return h;
}

function confettiState(confetti: any[]) {
  let h = 0x811c9dc5;
  for (const c of confetti) {
    h = int(h, c.modelIndex);
    for (const v of [c.pos, c.prevPos, c.vel, c.groundPos, c.groundNormal, c.groundVel]) h = vec(h, v);
    for (const v of [c.scale, c.groundBias]) h = num(h, v);
    for (const v of [c.onGround ? 1 : 0, c.life, c.rotX, c.rotY, c.rotZ, c.prevRotX, c.prevRotY, c.prevRotZ, c.rotVelX, c.rotVelY, c.rotVelZ]) h = int(h, v);
  }
  return h;
}

const player = game.players[0];
const visual = () => {
  const rt = game.stageRuntime;
  const h = hashSimState([player.ball], [game.world, player.world], rt, { includeVisual: true });
  return [hex(h), rt.effects.length, rt.confetti.length, hex(effectsState(rt.effects)), hex(confettiState(rt.confetti)), hex(rt.visualRng.state)];
};

const camera = () => {
  const c = player.camera;
  return [bits(c.eye.x), bits(c.eye.y), bits(c.eye.z), bits(c.lookAt.x), bits(c.lookAt.y), bits(c.lookAt.z), c.rotX, c.rotZ, c.timerCurr];
};

const lines: string[] = [];
for (let i = 0; i < frames; i++) {
  const goalTimerBefore = player.goalTimerFrames;
  game.update(game.fixedStep);
  attemptTick += 1;
  if (noReplay && game.loadingStage) {
    while (game.loadingStage) await new Promise((r) => setTimeout(r, 0));
    game.setInputFeed(feed);
    attemptTick = 0;
    lines.push(`RELOAD ${i}`);
    continue;
  }
  if (noReplay && goalTimerBefore > 0 && player.goalTimerFrames === 0) {
    lines.push(`END goal-finished ${i}`);
    for (let k = 0; k < tail; k++) {
      game.stageRuntime.advance(1, false, game.world, null, null, null, null, true);
      lines.push(["T", k, ...visual()].join(" "));
    }
    break;
  }
  if (game.loadingStage || game.activeResultReplay || game.pendingAdvance) {
    lines.push(`END ${game.activeResultReplay ? "result-replay" : "reload-or-advance"} ${i}`);
    break;
  }
  const b = player.ball;
  const h = hashSimState([b], [game.world, player.world], game.stageRuntime);
  lines.push([
    "F", i, h.toString(16).padStart(8, "0"),
    bits(b.pos.x), bits(b.pos.y), bits(b.pos.z), bits(b.vel.x), bits(b.vel.y), bits(b.vel.z),
    b.state, b.flags, b.rotX, b.rotY, b.rotZ, b.apeYaw, player.camera.rotY,
    game.introTimerFrames, game.stageTimerFrames, player.goalTimerFrames, player.ringoutTimerFrames,
    player.world.xrot, player.world.zrot,
    ...visual(),
    ...camera(),
    player.ringoutSkipTimerFrames, player.goalSkipTimerFrames, game.timeoverTimerFrames,
  ].join(" "));
}
Deno.writeTextFileSync(outFile, lines.join("\n") + "\n");
