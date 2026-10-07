// Runs the read-only reference parser (WebMonkeyBall src/gma.ts + gx_displaylist.ts) over GMA files and
// prints one line per model: file \t model \t shapes \t dlists \t triangles \t position checksum.
// usage: deno run -A gma_ref.ts <WebMonkeyBall dir> <scratch dir> <gma file>...
const [refDir, scratch, ...files] = Deno.args;
const dst = `${scratch}/refsrc`;
Deno.mkdirSync(dst, { recursive: true });
// The reference imports siblings as "./x.js"; copy to scratch with ".ts" specifiers so Deno can load them.
for (const f of ["gma", "gx", "gx_displaylist"]) {
  const src = Deno.readTextFileSync(`${refDir}/src/${f}.ts`).replaceAll(".js'", ".ts'");
  Deno.writeTextFileSync(`${dst}/${f}.ts`, src);
}
const toUrl = (p: string) => "file:///" + p.replaceAll("\\", "/").replace(/^\//, "");
const { parseGma, ModelFlags } = await import(toUrl(`${dst}/gma.ts`));
const { parseDisplayList } = await import(toUrl(`${dst}/gx_displaylist.ts`));
const tpl = { textures: new Map() };
for (const file of files) {
  const d = Deno.readFileSync(file);
  const buf = d.buffer.slice(d.byteOffset, d.byteOffset + d.byteLength);
  const gma = parseGma(buf, tpl);
  for (const [name, model] of gma.nameMap) {
    let tris = 0, sum = 0, dl = 0;
    for (const shape of model.shapes) {
      for (const dlist of shape.dlists) {
        dl++;
        const out = parseDisplayList(dlist.data, shape.material.vtxAttrs, (model.flags & ModelFlags.Vat16Bit) !== 0);
        const p = out.positions;
        tris += p.length / 9;
        for (let i = 0; i < p.length; i += 3) sum += p[i] + 2 * p[i + 1] + 3 * p[i + 2];
      }
    }
    console.log([file.split(/[\\/]/).pop(), name, model.shapes.length, dl, tris, sum].join("\t"));
  }
}
