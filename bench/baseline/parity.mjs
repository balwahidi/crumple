#!/usr/bin/env node
// Parity check: are the jSquash builds used by run.mjs byte-identical to the codec
// binaries the Squoosh web app actually ships?
//
// Encodes each image twice with the options from run.mjs: once through jSquash (exactly
// as run.mjs does) and once through the prebuilt single-threaded wasm committed in a
// Squoosh clone (codecs/*/enc/*.wasm, codecs/oxipng/pkg). Exits 1 if any output differs.
//
//   node baseline/parity.mjs [--squoosh <clone>] [--only kodim01,kodim13,example]
//
// The Squoosh clone defaults to $SQUOOSH_DIR, then ../squoosh next to the repo.

import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const benchDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const opt = (name, dflt) => (args.includes(name) ? args[args.indexOf(name) + 1] : dflt);
const squoosh = resolve(opt('--squoosh', process.env.SQUOOSH_DIR || join(benchDir, '..', '..', 'squoosh')));
const names = opt('--only', 'kodim01,kodim13,example').split(',').map((s) => s.trim()).filter(Boolean);
const sq = (rel) => join(squoosh, 'codecs', rel);

process.env.CRUMPLE_BENCH_IMPORT = '1';
const { CODECS, SQUOOSH_DEFAULTS: O, loadPng } = await import(pathToFileURL(join(benchDir, 'baseline', 'run.mjs')).href);

// Squoosh's web builds are Emscripten ENVIRONMENT=worker glue; it only reads
// self.location.href, and instantiateWasm below avoids any fetch.
globalThis.self ??= { location: { href: pathToFileURL(sq('') + '/').href } };

async function squooshEmscripten(js, wasmFile) {
  const factory = (await import(pathToFileURL(sq(js)).href)).default;
  const mod = await WebAssembly.compile(await readFile(sq(wasmFile)));
  return factory({
    noInitialRun: true,
    instantiateWasm: (imports, cb) => { const i = new WebAssembly.Instance(mod, imports); cb(i); return i.exports; },
  });
}

// The binaries Squoosh's encoder workers load when wasm threads are unavailable
// (src/features/encoders/*/worker/*.ts). webp uses the SIMD build, as Squoosh does
// when SIMD is available.
const moz = await squooshEmscripten('mozjpeg/enc/mozjpeg_enc.js', 'mozjpeg/enc/mozjpeg_enc.wasm');
const webp = await squooshEmscripten('webp/enc/webp_enc_simd.js', 'webp/enc/webp_enc_simd.wasm');
const avif = await squooshEmscripten('avif/enc/avif_enc.js', 'avif/enc/avif_enc.wasm');
const jxl = await squooshEmscripten('jxl/enc/jxl_enc.js', 'jxl/enc/jxl_enc.wasm');
const oxi = await import(pathToFileURL(sq('oxipng/pkg/squoosh_oxipng.js')).href);
await oxi.default(await WebAssembly.compile(await readFile(sq('oxipng/pkg/squoosh_oxipng_bg.wasm'))));

// Embind value_objects ignore extra fields, so jSquash-only keys are harmless here.
const squooshEncode = {
  mozjpeg: (i) => moz.encode(i.data, i.width, i.height, O.mozjpeg),
  webp: (i) => webp.encode(i.data, i.width, i.height, O.webp),
  avif: (i) => avif.encode(i.data, i.width, i.height, O.avif),
  jxl: (i) => jxl.encode(i.data, i.width, i.height, O.jxl),
  // Squoosh's optimise() takes no alpha flag: codecs/oxipng/src/lib.rs hard-codes optimize_alpha = true.
  oxipng: (i) => oxi.optimise(i.data, i.width, i.height, O.oxipng.level, O.oxipng.interlace),
};

const png = await loadPng();
const clone = (img) => new ImageData(new Uint8ClampedArray(img.data), img.width, img.height);
let mismatches = 0;
console.log(`Squoosh clone: ${squoosh}`);
console.log('| Image | Codec | jSquash bytes | Squoosh bytes | Identical |\n|---|---|---:|---:|---|');
for (const c of CODECS) c.impl = await c.load(png);
for (const name of names) {
  const buf = await readFile(join(benchDir, 'corpus', 'kodak', `${name}.png`));
  const src = await png.decode(buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));
  for (const c of CODECS) {
    const a = new Uint8Array(await c.impl.encode(clone(src)));
    const b = new Uint8Array(squooshEncode[c.name](clone(src)));
    const same = Buffer.from(a).equals(Buffer.from(b));
    if (!same) mismatches++;
    console.log(`| ${name} | ${c.name} | ${a.length} | ${b.length} | ${same ? 'yes' : 'NO'} |`);
  }
}
console.log(mismatches ? `\n${mismatches} mismatch(es)` : '\nAll outputs byte-identical.');
process.exit(mismatches ? 1 : 0);
