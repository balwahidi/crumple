#!/usr/bin/env node
// Re-scores the outputs of a bench run without any Crumple code: each output is decoded with
// jSquash (Squoosh's mozjpeg, libwebp and libavif/dav1d wasm builds), written as PNG and scored
// with `ssimulacra2_rs image <original> <decoded>`. That is the path bench/baseline/run.mjs used
// for the Squoosh baseline, so these scores compare directly with it.
//
//   node bench/crumple/rescore.mjs [--targets 70,80,90]
//
// Needs `npm ci` in bench/ (for the jSquash packages) and `ssimulacra2_rs` on PATH (or
// SSIMULACRA2_BIN), as in bench/README.md. Reads bench/out/crumple/t<T>/, which run.mjs leaves
// behind, and writes decoded PNGs to bench/out/crumple/rescore/.

import { execFileSync } from 'node:child_process';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const benchDir = join(here, '..');
const corpusDir = join(benchDir, 'corpus', 'kodak');
const outDir = join(benchDir, 'out', 'crumple');
const jsq = join(benchDir, 'node_modules', '@jsquash');
const SSIM_BIN = process.env.SSIMULACRA2_BIN || 'ssimulacra2_rs';
const args = process.argv.slice(2);
const i = args.indexOf('--targets');
const TARGETS = (i > -1 ? args[i + 1] : '70,80,90').split(',').map(Number);

// Node has no ImageData, and the jSquash decoders construct one (same polyfill as the baseline).
globalThis.ImageData ??= class ImageData {
  constructor(data, width, height) {
    if (typeof data === 'number') { height = width; width = data; data = new Uint8ClampedArray(width * height * 4); }
    Object.assign(this, { data, width, height, colorSpace: 'srgb' });
  }
};
const wasm = async (rel) => WebAssembly.compile(await readFile(join(jsq, rel)));
const imp = (rel) => import(pathToFileURL(join(jsq, rel)).href);
const pngEnc = await imp('png/encode.js');
await pngEnc.init(await wasm('png/codec/pkg/squoosh_png_bg.wasm'));
const decoders = {};
for (const [ext, pkg, file] of [['jpg', 'jpeg', 'mozjpeg_dec.wasm'], ['webp', 'webp', 'webp_dec.wasm'], ['avif', 'avif', 'avif_dec.wasm']]) {
  const d = await imp(`${pkg}/decode.js`);
  await d.init(await wasm(`${pkg}/codec/dec/${file}`));
  decoders[ext] = d.default;
}

const median = (xs) => { const s = [...xs].sort((a, b) => a - b), m = s.length >> 1; return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2; };
for (const T of TARGETS) {
  const dir = join(outDir, `t${T}`);
  const tmp = join(outDir, 'rescore', `t${T}`);
  await mkdir(tmp, { recursive: true });
  const rows = [];
  for (const f of (await readdir(dir)).sort()) {
    const ext = f.split('.').pop();
    const stem = f.slice(0, -(ext.length + 1));
    const ref = join(corpusDir, `${stem}.png`);
    let dist = join(dir, f);
    if (ext !== 'png') {
      const buf = await readFile(dist);
      const img = await decoders[ext](buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));
      dist = join(tmp, `${stem}.png`);
      await writeFile(dist, Buffer.from(await pngEnc.default(new ImageData(new Uint8ClampedArray(img.data), img.width, img.height))));
    }
    const out = execFileSync(SSIM_BIN, ['image', ref, dist], { encoding: 'utf8' });
    rows.push({ file: f, score: Number(out.match(/Score:\s*(-?[\d.]+)/)[1]) });
  }
  const scores = rows.map((r) => r.score);
  const below = rows.filter((r) => r.score < T).map((r) => `${r.file} ${r.score.toFixed(3)}`);
  console.log(`target ${T}: ${rows.length} outputs, min ${Math.min(...scores).toFixed(3)}, median ${median(scores).toFixed(3)}, `
    + `below target: ${below.length ? below.join(', ') : 'none'}`);
}
