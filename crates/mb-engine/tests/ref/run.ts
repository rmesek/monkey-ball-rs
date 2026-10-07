// Headless driver for the WebMonkeyBall reference simulation (read-only use of the reference sources).
//
//   deno run --allow-read --allow-write --sloppy-imports --allow-env --import-map import_map.json run.ts \
//        <reference src dir> <stage id> <decompressed-or-lz stage file> <inputs file> <frames> <out file>
//
// Runs the real `Game` (fixed-tick mode, no HUD/audio) with a scripted stick feed and writes one line per tick:
//   F tick hash px py pz vx vy vz state flags rotX rotY rotZ apeYaw camRotY intro stageTimer goalTimer ringout xrot zrot
// f64 values are the raw IEEE bits as 16 hex digits. A final `END reason` line is written when the reference
// leaves the plain gameplay loop (result replay, stage reload, course advance).
import "./shim.ts";

const [srcDir, stageIdArg, stageFile, inputsFile, framesArg, outFile] = Deno.args;
const stageId = Number(stageIdArg);
const frames = Number(framesArg);
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
  const [x, y] = l.split(" ").map(Number);
  return { x, y };
});

const { Game } = await import(base + "game.ts");
const { hashSimState } = await import(base + "sim_hash.ts");

const game = new Game();
game.init();
game.setFixedTickMode(true, 1);
await game.loadStage(stageId);
game.setInputFeed(feed);
game.paused = false;
game.simPerf.enabled = false;
game.rollbackPerf.enabled = false;

const f64 = new DataView(new ArrayBuffer(8));
const bits = (v: number) => {
  f64.setFloat64(0, v);
  return f64.getBigUint64(0).toString(16).padStart(16, "0");
};

const lines: string[] = [];
const player = game.players[0];
for (let i = 0; i < frames; i++) {
  game.update(game.fixedStep);
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
  ].join(" "));
}
Deno.writeTextFileSync(outFile, lines.join("\n") + "\n");
