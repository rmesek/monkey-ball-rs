// Reference-side hashing for tests/disc.rs: decodes TPL files with WebMonkeyBall's own decoder
// (read-only import) and prints "<file>\t<texture>\t<level>\t<w>x<h>\t<fnv1a32 hex>" per level.
// usage: deno run -A --sloppy-imports ref_hash.ts <path to WebMonkeyBall/src> <tpl file>...
const [srcDir, ...files] = Deno.args;
const { parseAVTpl, buildTplTextureData } = await import(
  new URL("file:///" + srcDir.replaceAll("\\", "/") + "/tpl.ts").href
);

function fnv1a(data: Uint8Array): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < data.length; i++) {
    h ^= data[i];
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h >>> 0;
}

const lines: string[] = [];
for (const f of files) {
  const bytes = Deno.readFileSync(f);
  const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  const tpl = parseAVTpl(buf, "t");
  for (const idx of [...tpl.textures.keys()].sort((a: number, b: number) => a - b)) {
    let tex;
    try {
      tex = buildTplTextureData(tpl, idx);
    } catch (e) {
      lines.push(`${f}\t${idx}\terr`);
      continue;
    }
    tex.levels.forEach((l: any, i: number) => {
      if (!l.data) {
        lines.push(`${f}\t${idx}\t${i}\tnull`);
        return;
      }
      lines.push(`${f}\t${idx}\t${i}\t${l.width}x${l.height}\t${fnv1a(l.data).toString(16)}`);
    });
  }
}
console.log(lines.join("\n"));
