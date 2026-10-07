// Prints FNV-1a (32-bit) checksums of the reference's SIN_TABLE / ATAN_TABLE float bits: `sin atan` (hex).
const [srcDir] = Deno.args;
const base = "file:///" + srcDir.split("\\").join("/").replace(/\/?$/, "/");
const { SIN_TABLE, ATAN_TABLE } = await import(base + "math_lut.ts");
const u32 = (t: Float32Array) => new Uint32Array(t.buffer, t.byteOffset, t.length);
const fnv = (t: Float32Array) => {
  let h = 0x811c9dc5;
  for (const v of u32(t)) h = Math.imul(h ^ v, 16777619) >>> 0;
  return h.toString(16).padStart(8, "0");
};
console.log(`${fnv(SIN_TABLE)} ${fnv(ATAN_TABLE)}`);
