#!/usr/bin/env node
// Crumple baseline benchmark: Squoosh's codecs (via jSquash WASM builds) at
// Squoosh's own default settings, run in Node.
//
//   node baseline/run.mjs                         full run: per-image measurements, then batch runs
//   node baseline/run.mjs --only kodim01,kodim13  restrict to image names containing any of the strings
//   node baseline/run.mjs --runs 5                encode repetitions per image/codec (default 3)
//   node baseline/run.mjs --batch [--codec c]     internal: batch pass only, prints one JSON line
//
// A full run writes baseline/results.json and baseline/RESULTS.md. A run with --only
// writes out/results.partial.json and out/RESULTS.partial.md instead, so a quick check
// never overwrites the committed baseline. Encoded and decoded outputs go to out/.

import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import os from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const benchDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const corpusDir = join(benchDir, 'corpus');
const outDir = join(benchDir, 'out');
const baselineDir = join(benchDir, 'baseline');
const jsq = join(benchDir, 'node_modules', '@jsquash');

const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const opt = (name, dflt) => {
  const i = args.indexOf(name);
  return i > -1 ? args[i + 1] : dflt;
};
const RUNS = Number(opt('--runs', 3));
const ONLY_ARG = opt('--only', null);
const ONLY = ONLY_ARG ? ONLY_ARG.split(',').map((s) => s.trim()).filter(Boolean) : null;
const BATCH_CODEC = opt('--codec', null);
if (!Number.isInteger(RUNS) || RUNS < 1) throw new Error(`--runs must be a positive integer, got ${opt('--runs')}`);
const SSIM_BIN = process.env.SSIMULACRA2_BIN || 'ssimulacra2_rs';

// --- ImageData polyfill -----------------------------------------------------
// jSquash's decoders construct ImageData and @jsquash/oxipng does
// `data instanceof ImageData`; Node has no ImageData global.
if (typeof globalThis.ImageData === 'undefined') {
  globalThis.ImageData = class ImageData {
    constructor(data, width, height) {
      if (typeof data === 'number') {
        height = width;
        width = data;
        data = new Uint8ClampedArray(width * height * 4);
      }
      this.data = data;
      this.width = width;
      this.height = height;
      this.colorSpace = 'srgb';
    }
  };
}

// --- Squoosh default options ------------------------------------------------
// Copied verbatim from squoosh/src/features/encoders/<codec>/shared/meta.ts.
// jSquash uses the same option names (see README.md for the mapping table).
export const SQUOOSH_DEFAULTS = {
  mozjpeg: {
    quality: 75, baseline: false, arithmetic: false, progressive: true, optimize_coding: true,
    smoothing: 0, color_space: 3 /* YCbCr */, quant_table: 3, trellis_multipass: false,
    trellis_opt_zero: false, trellis_opt_table: false, trellis_loops: 1, auto_subsample: true,
    chroma_subsample: 2, separate_chroma_quality: false, chroma_quality: 75,
  },
  webp: {
    quality: 75, target_size: 0, target_PSNR: 0, method: 4, sns_strength: 50, filter_strength: 60,
    filter_sharpness: 0, filter_type: 1, partitions: 0, segments: 4, pass: 1, show_compressed: 0,
    preprocessing: 0, autofilter: 0, partition_limit: 0, alpha_compression: 1, alpha_filtering: 1,
    alpha_quality: 100, lossless: 0, exact: 0, image_hint: 0, emulate_jpeg_size: 0, thread_level: 0,
    low_memory: 0, near_lossless: 100, use_delta_palette: 0, use_sharp_yuv: 0,
  },
  avif: {
    quality: 50, qualityAlpha: -1, denoiseLevel: 0, tileColsLog2: 0, tileRowsLog2: 0, speed: 6,
    subsample: 1, chromaDeltaQ: false, sharpness: 0, tune: 0 /* auto */, enableSharpYUV: false,
    // jSquash-only extras, pinned to the values that reproduce Squoosh's behaviour:
    bitDepth: 8, lossless: false,
  },
  jxl: {
    effort: 7, quality: 75, progressive: false, epf: -1, lossyPalette: false, decodingSpeedTier: 0,
    photonNoiseIso: 0, lossyModular: false,
    // jSquash-only extra:
    lossless: false,
  },
  oxipng: {
    level: 2, interlace: false,
    // jSquash exposes this as an option. Squoosh's build hard-codes it on:
    // squoosh/codecs/oxipng/src/lib.rs sets `options.optimize_alpha = true`.
    optimiseAlpha: true,
  },
};

// --- Codec loading ----------------------------------------------------------
// In Node, jSquash's default init() tries to fetch() a file:// URL for the wasm,
// which Node's fetch does not support. So every module is compiled from disk
// and handed to the package's init(). The wasm variant must match the JS glue
// jSquash selects in Node: single-threaded for avif/jxl (they check
// isRunningInNode), SIMD for webp (Node 24 supports wasm SIMD), and the
// single-threaded oxipng pkg (the MT one is only used inside a Worker).
const wasm = async (rel) => WebAssembly.compile(await readFile(join(jsq, rel)));
const imp = (rel) => import(pathToFileURL(join(jsq, rel)).href);

export async function loadPng() {
  const enc = await imp('png/encode.js');
  const dec = await imp('png/decode.js');
  const mod = await wasm('png/codec/pkg/squoosh_png_bg.wasm');
  await enc.init(mod);
  await dec.init(mod);
  return { encode: enc.default, decode: dec.default };
}

export const CODECS = [
  {
    name: 'mozjpeg', ext: 'jpg', lossless: false,
    async load() {
      const e = await imp('jpeg/encode.js');
      const d = await imp('jpeg/decode.js');
      await e.init(await wasm('jpeg/codec/enc/mozjpeg_enc.wasm'));
      await d.init(await wasm('jpeg/codec/dec/mozjpeg_dec.wasm'));
      return { encode: (img) => e.default(img, SQUOOSH_DEFAULTS.mozjpeg), decode: (b) => d.default(b) };
    },
  },
  {
    name: 'webp', ext: 'webp', lossless: false,
    async load() {
      const e = await imp('webp/encode.js');
      const d = await imp('webp/decode.js');
      await e.init(await wasm('webp/codec/enc/webp_enc_simd.wasm'));
      await d.init(await wasm('webp/codec/dec/webp_dec.wasm'));
      return { encode: (img) => e.default(img, SQUOOSH_DEFAULTS.webp), decode: (b) => d.default(b) };
    },
  },
  {
    name: 'avif', ext: 'avif', lossless: false,
    async load() {
      const e = await imp('avif/encode.js');
      const d = await imp('avif/decode.js');
      await e.init(await wasm('avif/codec/enc/avif_enc.wasm'));
      await d.init(await wasm('avif/codec/dec/avif_dec.wasm'));
      return { encode: (img) => e.default(img, SQUOOSH_DEFAULTS.avif), decode: (b) => d.default(b) };
    },
  },
  {
    name: 'jxl', ext: 'jxl', lossless: false,
    async load() {
      const e = await imp('jxl/encode.js');
      const d = await imp('jxl/decode.js');
      await e.init(await wasm('jxl/codec/enc/jxl_enc.wasm'));
      await d.init(await wasm('jxl/codec/dec/jxl_dec.wasm'));
      return { encode: (img) => e.default(img, SQUOOSH_DEFAULTS.jxl), decode: (b) => d.default(b) };
    },
  },
  {
    name: 'oxipng', ext: 'png', lossless: true,
    async load(png) {
      const o = await imp('oxipng/optimise.js');
      await o.init(await wasm('oxipng/codec/pkg/squoosh_oxipng_bg.wasm'));
      // Squoosh feeds oxipng raw RGBA (optimise_raw), not the source PNG file;
      // passing an ImageData makes jSquash take the same optimise_raw path.
      return { encode: (img) => o.default(img, SQUOOSH_DEFAULTS.oxipng), decode: (b) => png.decode(b) };
    },
  },
];

// --- Helpers ----------------------------------------------------------------
const median = (xs) => {
  const s = xs.filter((x) => Number.isFinite(x)).sort((a, b) => a - b);
  if (!s.length) return null;
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const round = (x, d = 2) => (x == null ? null : Math.round(x * 10 ** d) / 10 ** d);
const toArrayBuffer = (buf) => buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength);
const cloneImage = (img) => new ImageData(new Uint8ClampedArray(img.data), img.width, img.height);
const errText = (e) => (e && (e.stack || e.message)) ? String(e.message || e).split('\n')[0] : String(e);

async function listCorpus() {
  const sums = JSON.parse(await readFile(join(corpusDir, 'checksums.json'), 'utf8')).files;
  const files = [];
  for (const [rel, hash] of Object.entries(sums)) {
    const path = join(corpusDir, rel);
    const buf = await readFile(path);
    const actual = createHash('sha256').update(buf).digest('hex');
    if (actual !== hash) throw new Error(`checksum mismatch for ${rel}; re-run corpus/fetch.mjs`);
    const name = rel.split('/').pop().replace(/\.png$/, '');
    if (ONLY && !ONLY.some((s) => name.includes(s))) continue;
    files.push({ name, path, bytes: buf.length, buf });
  }
  return files;
}

function ssimulacra2(ref, dist) {
  const out = execFileSync(SSIM_BIN, ['image', ref, dist], { encoding: 'utf8' });
  const m = out.match(/Score:\s*(-?[\d.]+)/);
  if (!m) throw new Error(`could not parse ssimulacra2 output: ${out.trim()}`);
  return Number(m[1]);
}

// Lossless check. With optimize_alpha on (as in Squoosh), oxipng may rewrite the RGB
// of fully transparent pixels, so for alpha == 0 only the alpha value is compared.
function pixelsIdentical(a, b) {
  if (a.width !== b.width || a.height !== b.height) return false;
  const x = a.data, y = b.data;
  if (x.length !== y.length) return false;
  for (let i = 0; i < x.length; i += 4) {
    if (x[i + 3] !== y[i + 3]) return false;
    if (x[i + 3] === 0) continue;
    if (x[i] !== y[i] || x[i + 1] !== y[i + 1] || x[i + 2] !== y[i + 2]) return false;
  }
  return true;
}

function hasAlpha(img) {
  const d = img.data;
  for (let i = 3; i < d.length; i += 4) if (d[i] !== 255) return true;
  return false;
}

async function loadAll(only = null) {
  const png = await loadPng();
  const codecs = [];
  for (const c of CODECS) {
    if (only && c.name !== only) continue;
    try {
      codecs.push({ ...c, impl: await c.load(png), error: null });
    } catch (e) {
      codecs.push({ ...c, impl: null, error: `init failed: ${errText(e)}` });
    }
  }
  return { png, codecs };
}

// --- Batch pass (runs in its own child process so peak RSS is its own) ------
// With --codec, only that codec is loaded and run, which gives a per-codec peak RSS.
// Without it, all codecs run in one process; wasm memory never shrinks, so that peak
// is roughly the sum of every codec's high-water mark, not the worst single codec.
async function batch() {
  if (BATCH_CODEC && !CODECS.some((c) => c.name === BATCH_CODEC)) throw new Error(`unknown codec ${BATCH_CODEC}`);
  let peakSampled = process.memoryUsage().rss;
  const sample = () => { const r = process.memoryUsage().rss; if (r > peakSampled) peakSampled = r; };
  const timer = setInterval(sample, 5);
  const t0 = performance.now();
  const files = await listCorpus();
  const { png, codecs } = await loadAll(BATCH_CODEC);
  const tReady = performance.now();
  const rssAfterInit = process.memoryUsage().rss;
  const batchOut = join(outDir, 'batch');
  await mkdir(batchOut, { recursive: true });
  const perCodecMs = {};
  const errors = [];
  let outputs = 0, outBytes = 0;
  for (const f of files) {
    const img = await png.decode(toArrayBuffer(f.buf));
    sample();
    for (const c of codecs) {
      if (!c.impl) continue;
      try {
        const input = cloneImage(img); // clone not timed, same as the per-image pass
        const s = performance.now();
        const out = await c.impl.encode(input);
        perCodecMs[c.name] = (perCodecMs[c.name] || 0) + (performance.now() - s);
        await writeFile(join(batchOut, `${f.name}.${c.name}.${c.ext}`), new Uint8Array(out));
        outputs++;
        outBytes += out.byteLength;
      } catch (e) {
        errors.push(`${c.name}/${f.name}: ${errText(e)}`);
      }
      sample();
    }
  }
  const tEnd = performance.now();
  clearInterval(timer);
  sample();
  const ru = process.resourceUsage();
  return {
    images: files.length,
    codecs: codecs.filter((c) => c.impl).map((c) => c.name),
    outputs,
    outputBytes: outBytes,
    wallMsTotal: round(tEnd - t0, 1),
    wallMsInit: round(tReady - t0, 1),
    wallMsEncodeLoop: round(tEnd - tReady, 1),
    encodeMsPerCodec: Object.fromEntries(Object.entries(perCodecMs).map(([k, v]) => [k, round(v, 1)])),
    // RSS after codec init and corpus read, before any encode (Node + buffers + wasm instances).
    rssAfterInitBytes: rssAfterInit,
    peakRssSampledBytes: peakSampled,
    // libuv's ru_maxrss (KiB); on Windows this is PeakWorkingSetSize.
    peakRssOsBytes: ru.maxRSS * 1024,
    errors: [...codecs.filter((c) => c.error).map((c) => `${c.name}: ${c.error}`), ...errors],
  };
}

// --- Main per-image pass ----------------------------------------------------
async function main() {
  const startedAt = new Date().toISOString();
  const files = await listCorpus();
  if (!files.length) throw new Error(`--only ${ONLY_ARG} matched no corpus image`);
  const { png, codecs } = await loadAll();
  const records = [];
  const failures = codecs.filter((c) => c.error).map((c) => ({ codec: c.name, image: '*', error: c.error }));

  for (const f of files) {
    const src = await png.decode(toArrayBuffer(f.buf));
    const alpha = hasAlpha(src);
    for (const c of codecs) {
      if (!c.impl) continue;
      const rec = {
        image: f.name, codec: c.name, width: src.width, height: src.height, sourceHasAlpha: alpha,
        sourceBytes: f.bytes, encodeMsRuns: [], encodeMsMedian: null, outputBytes: null,
        bpp: null, ratio: null, ssimulacra2: null, pixelIdentical: null, error: null,
      };
      try {
        let out;
        for (let r = 0; r < RUNS; r++) {
          const input = cloneImage(src); // fresh buffer each run; clone not timed
          const s = performance.now();
          out = await c.impl.encode(input);
          rec.encodeMsRuns.push(round(performance.now() - s, 2));
        }
        rec.encodeMsMedian = median(rec.encodeMsRuns);
        rec.outputBytes = out.byteLength;
        rec.bpp = round((out.byteLength * 8) / (src.width * src.height), 4);
        rec.ratio = round(f.bytes / out.byteLength, 4);
        const codecDir = join(outDir, c.name);
        await mkdir(codecDir, { recursive: true });
        await writeFile(join(codecDir, `${f.name}.${c.ext}`), new Uint8Array(out));
        const decoded = await c.impl.decode(out);
        if (c.lossless) {
          rec.pixelIdentical = pixelsIdentical(src, decoded);
        } else {
          const decPath = join(codecDir, `${f.name}.decoded.png`);
          const decPng = await png.encode(new ImageData(new Uint8ClampedArray(decoded.data), decoded.width, decoded.height));
          await writeFile(decPath, new Uint8Array(decPng));
          rec.ssimulacra2 = round(ssimulacra2(f.path, decPath), 4);
        }
      } catch (e) {
        rec.error = errText(e);
        failures.push({ codec: c.name, image: f.name, error: rec.error });
      }
      records.push(rec);
      console.log(
        `${f.name.padEnd(9)} ${c.name.padEnd(8)} ` +
          (rec.error
            ? `FAILED: ${rec.error}`
            : `${String(rec.outputBytes).padStart(9)} B  x${rec.ratio.toFixed(2).padStart(6)}  ` +
              `${rec.encodeMsMedian.toFixed(1).padStart(8)} ms  ` +
              (c.lossless ? `pixels ${rec.pixelIdentical ? 'identical' : 'DIFFER'}` : `ssimulacra2 ${rec.ssimulacra2}`)),
      );
    }
  }

  const runBatchChild = (codec) => {
    const extra = [...(ONLY ? ['--only', ONLY_ARG] : []), ...(codec ? ['--codec', codec] : [])];
    const child = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--batch', ...extra], {
      encoding: 'utf8', maxBuffer: 64 * 1024 * 1024,
    });
    try {
      return JSON.parse(child.stdout.trim().split('\n').pop());
    } catch {
      return { error: `batch child failed (status ${child.status}): ${child.stderr || child.stdout}` };
    }
  };
  console.log('\nbatch pass, all codecs in one child process...');
  const batchResult = runBatchChild(null);
  const batchPerCodec = {};
  for (const c of codecs.filter((x) => x.impl)) {
    console.log(`batch pass, ${c.name} alone in a child process...`);
    batchPerCodec[c.name] = runBatchChild(c.name);
  }

  const summary = codecs.map((c) => {
    const ok = records.filter((r) => r.codec === c.name && !r.error);
    return {
      codec: c.name,
      status: c.error ? 'failed' : ok.length === files.length ? 'ok' : ok.length ? 'partial' : 'failed',
      images: ok.length,
      medianOutputBytes: median(ok.map((r) => r.outputBytes)),
      medianBpp: round(median(ok.map((r) => r.bpp)), 3),
      medianRatio: round(median(ok.map((r) => r.ratio)), 3),
      medianSsimulacra2: c.lossless ? null : round(median(ok.map((r) => r.ssimulacra2)), 2),
      pixelIdenticalCount: c.lossless ? ok.filter((r) => r.pixelIdentical).length : null,
      medianEncodeMs: round(median(ok.map((r) => r.encodeMsMedian)), 1),
      totalOutputBytes: ok.reduce((a, r) => a + r.outputBytes, 0),
      totalSourceBytes: ok.reduce((a, r) => a + r.sourceBytes, 0),
      error: c.error,
    };
  });

  const pkgVersions = Object.fromEntries(
    ['png', 'jpeg', 'webp', 'avif', 'oxipng', 'jxl'].map((p) => [
      `@jsquash/${p}`, JSON.parse(readFileSync(join(jsq, p, 'package.json'), 'utf8')).version,
    ]),
  );
  // Hybrid CPUs (P-cores + E-cores) make unpinned timings swing by about 2x, so record
  // what the process was allowed to run on. Children spawned by this process inherit it.
  let scheduling = null;
  try {
    if (process.platform === 'win32') {
      const mask = execFileSync('powershell.exe', ['-NoProfile', '-Command',
        `[int64](Get-Process -Id ${process.pid}).ProcessorAffinity`], { encoding: 'utf8' }).trim();
      scheduling = { affinityMask: `0x${BigInt(mask).toString(16)}` };
    } else if (process.platform === 'linux') {
      scheduling = { cpusAllowedList: readFileSync('/proc/self/status', 'utf8').match(/Cpus_allowed_list:\s*(.+)/)?.[1] ?? null };
    }
    scheduling = { ...scheduling, priority: os.getPriority() };
  } catch (e) { scheduling = { error: errText(e) }; }
  let ssimVersion = null;
  try { ssimVersion = execFileSync(SSIM_BIN, ['--version'], { encoding: 'utf8' }).trim(); } catch (e) { ssimVersion = `unavailable: ${errText(e)}`; }
  const cpus = os.cpus();
  const env = {
    cpuModel: cpus[0]?.model?.trim(),
    logicalCores: cpus.length,
    availableParallelism: os.availableParallelism(),
    totalMemBytes: os.totalmem(),
    os: `${os.type()} ${os.release()} (${os.version()}) ${os.arch()}`,
    node: process.version,
    v8: process.versions.v8,
    packages: pkgVersions,
    ssimulacra2: ssimVersion,
    threading: 'single-threaded WASM for every codec (jSquash Node code paths)',
    scheduling,
  };

  const results = {
    startedAt, finishedAt: new Date().toISOString(), runsPerMeasurement: RUNS, env,
    partial: ONLY ? `--only ${ONLY_ARG}` : false,
    corpus: { images: files.map((f) => ({ name: f.name, bytes: f.bytes })) },
    options: SQUOOSH_DEFAULTS, summary, batch: batchResult, batchPerCodec, failures, records,
  };
  // A restricted run must never overwrite the committed full baseline.
  const [jsonPath, mdPath] = ONLY
    ? [join(outDir, 'results.partial.json'), join(outDir, 'RESULTS.partial.md')]
    : [join(baselineDir, 'results.json'), join(baselineDir, 'RESULTS.md')];
  await mkdir(dirname(jsonPath), { recursive: true });
  await writeFile(jsonPath, JSON.stringify(results, null, 2) + '\n');
  await writeFile(mdPath, renderMarkdown(results));
  console.log('\n' + renderSummaryTable(summary));
  console.log(renderBatch(batchResult));
  console.log(renderBatchPerCodec(batchPerCodec));
  console.log(`\nwrote ${jsonPath} and ${mdPath}`);
  if (failures.length) console.log(`\n${failures.length} failure(s); see results.json`);
}

const fmtBytes = (b) => (b == null ? 'n/a' : b >= 1048576 ? `${(b / 1048576).toFixed(2)} MiB` : `${(b / 1024).toFixed(1)} KiB`);

function renderSummaryTable(summary) {
  const rows = [
    '| Codec | Status | Images | Median bytes | Median bpp | Median ratio (src/out) | Median SSIMULACRA2 | Median encode ms |',
    '|---|---|---:|---:|---:|---:|---:|---:|',
  ];
  for (const s of summary) {
    const q = s.codec === 'oxipng'
      ? `lossless: ${s.pixelIdenticalCount}/${s.images} pixel-identical`
      : s.medianSsimulacra2 ?? 'n/a';
    rows.push(`| ${s.codec} | ${s.status}${s.error ? ` (${s.error})` : ''} | ${s.images} | ${s.medianOutputBytes ?? 'n/a'} | ${s.medianBpp ?? 'n/a'} | ${s.medianRatio ?? 'n/a'} | ${q} | ${s.medianEncodeMs ?? 'n/a'} |`);
  }
  return rows.join('\n');
}

function renderImageTable(records, image) {
  const rows = [
    '| Codec | Bytes | bpp | Ratio (src/out) | SSIMULACRA2 | Encode ms (median) |',
    '|---|---:|---:|---:|---:|---:|',
  ];
  for (const r of records.filter((x) => x.image === image)) {
    const q = r.error ? `FAILED: ${r.error}`
      : r.pixelIdentical != null ? (r.pixelIdentical ? 'lossless (pixel-identical)' : 'pixels DIFFER')
      : r.ssimulacra2;
    rows.push(`| ${r.codec} | ${r.outputBytes ?? 'n/a'} | ${r.bpp ?? 'n/a'} | ${r.ratio ?? 'n/a'} | ${q} | ${r.encodeMsMedian ?? 'n/a'} |`);
  }
  return rows.join('\n');
}

function renderBatch(b) {
  if (!b || b.error) return `Batch: FAILED ${b?.error ?? ''}`;
  const lines = [
    '| Batch metric | Value |', '|---|---:|',
    `| Images x codecs | ${b.images} x ${b.codecs.length} = ${b.outputs} outputs |`,
    `| Total wall time | ${(b.wallMsTotal / 1000).toFixed(2)} s |`,
    `| of which codec init + corpus read | ${(b.wallMsInit / 1000).toFixed(2)} s |`,
    `| of which decode/encode/write loop | ${(b.wallMsEncodeLoop / 1000).toFixed(2)} s |`,
    ...Object.entries(b.encodeMsPerCodec).map(([k, v]) => `| encode time, ${k} | ${(v / 1000).toFixed(2)} s |`),
    `| Peak RSS (sampled process.memoryUsage().rss) | ${fmtBytes(b.peakRssSampledBytes)} |`,
    `| Peak RSS (OS, process.resourceUsage().maxRSS) | ${fmtBytes(b.peakRssOsBytes)} |`,
    `| Total output bytes | ${fmtBytes(b.outputBytes)} |`,
  ];
  if (b.errors.length) lines.push(`| Errors | ${b.errors.length} |`);
  return lines.join('\n');
}

function renderBatchPerCodec(per) {
  const rows = [
    '| Codec (alone in its own process) | Wall time | Encode time | RSS after init | Peak RSS (OS) | Errors |',
    '|---|---:|---:|---:|---:|---:|',
  ];
  for (const [name, b] of Object.entries(per ?? {})) {
    if (!b || b.error) { rows.push(`| ${name} | FAILED: ${b?.error ?? ''} | | | | |`); continue; }
    rows.push(`| ${name} | ${(b.wallMsTotal / 1000).toFixed(2)} s | ${((b.encodeMsPerCodec[name] ?? 0) / 1000).toFixed(2)} s | ` +
      `${fmtBytes(b.rssAfterInitBytes)} | ${fmtBytes(b.peakRssOsBytes)} | ${b.errors.length} |`);
  }
  return rows.join('\n');
}

function renderMarkdown(r) {
  const e = r.env;
  const hasExample = r.corpus.images.some((i) => i.name === 'example');
  const corpusLine = r.partial
    ? `PARTIAL RUN (\`${r.partial}\`), ${r.corpus.images.length} image(s): ${r.corpus.images.map((i) => i.name).join(', ')}.`
    : `${r.corpus.images.length} images: Kodak kodim01 to kodim24 (768x512 / 512x768, RGB) plus Squoosh's \`codecs/example.png\` (2558x1348, stored as RGBA but fully opaque).`;
  return `# Baseline results: Squoosh codecs at Squoosh defaults

Generated by \`node baseline/run.mjs${r.partial ? ` ${r.partial}` : ''}\` (${r.startedAt} to ${r.finishedAt}). Raw data: \`${r.partial ? 'results.partial.json' : 'results.json'}\`.

## Environment

- CPU: ${e.cpuModel} (${e.logicalCores} logical cores)
- OS: ${e.os}
- Node: ${e.node} (V8 ${e.v8})
- Packages: ${Object.entries(e.packages).map(([k, v]) => `${k}@${v}`).join(', ')}
- Metric: ${e.ssimulacra2}
- Threading: ${e.threading}
- Scheduling: ${e.scheduling ? Object.entries(e.scheduling).map(([k, v]) => `${k} ${v}`).join(', ') : 'n/a'}

## Corpus

${corpusLine}
Total source size ${fmtBytes(r.corpus.images.reduce((a, i) => a + i.bytes, 0))}.

## Per-codec summary

Encode time is the per-image median of ${r.runsPerMeasurement} wall-clock runs, then the median across images.
bpp is output bits per pixel (lower means smaller output).
Ratio is source PNG bytes divided by output bytes (higher means smaller output); it depends on how well the source PNG was compressed, so prefer bpp.
SSIMULACRA2 compares the decoded output with the original PNG (100 = identical, higher is better).
${r.partial ? '' : `Medians are dominated by the 24 small Kodak photos${hasExample ? '; the one large screenshot is shown separately below' : ''}.\n`}
${renderSummaryTable(r.summary)}
${hasExample ? `
### \`example.png\` alone (2558x1348 screenshot)

${renderImageTable(r.records, 'example')}
` : ''}
## Batch run

Whole corpus through all codecs sequentially, once each, in a fresh child process
(includes WASM compile/init, PNG decode of each source, encode, and writing outputs to disk).
WASM memory never shrinks, so this peak RSS is roughly the sum of every codec's high-water
mark in one process, not the cost of any single codec. The sampled RSS peak can miss spikes
inside a synchronous WASM call; the OS figure cannot.

${renderBatch(r.batch)}

### One codec per process

The same batch, run once per codec in its own fresh child process, so each peak RSS belongs
to that codec alone (it includes Node itself, the corpus buffers and the PNG codec; see
"RSS after init"). The largest image (2558x1348) sets the peak.

${renderBatchPerCodec(r.batchPerCodec)}

## Failures

${r.failures.length ? r.failures.map((f) => `- ${f.codec} / ${f.image}: ${f.error}`).join('\n') : 'None.'}
`;
}

// CRUMPLE_BENCH_IMPORT=1 lets another script (parity.mjs) import the codec table
// and options without starting a run.
if (process.env.CRUMPLE_BENCH_IMPORT === '1') {
  // imported as a library: do nothing
} else if (flag('--batch')) {
  batch().then((r) => { process.stdout.write(JSON.stringify(r) + '\n'); process.exit(0); },
    (e) => { console.error(e); process.exit(1); });
} else {
  main().catch((e) => { console.error(e); process.exit(1); });
}
