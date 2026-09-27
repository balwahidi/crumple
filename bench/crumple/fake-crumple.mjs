#!/usr/bin/env node
// Stand-in for the `crumple` binary, used to test bench/crumple/run.mjs before the real
// pipeline exists. It accepts the §8 `optimize` flags, copies every supported input to
// its output path unchanged, and writes a T4-schema JSONL report ("kept-original").
//
//   node bench/crumple/fake-crumple.mjs optimize <INPUT>... --out <DIR> [--report r.jsonl] ...
//   node bench/crumple/fake-crumple.mjs --version

import { copyFileSync, existsSync, lstatSync, mkdirSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { dirname, extname, join, relative, resolve } from 'node:path';

const t0 = performance.now();
const argv = process.argv.slice(2);
const usage = (msg) => { process.stderr.write(`error: ${msg}\n`); process.exit(2); };

if (argv[0] === '--version') { process.stdout.write('crumple 0.0.0 (fake)\n'); process.exit(0); }
if (argv[0] !== 'optimize') usage('expected subcommand `optimize`');

const VALUE_FLAGS = new Set(['--out', '--target', '--formats', '--max-width', '--max-height', '--jobs',
  '--max-memory', '--max-pixels', '--report']);
const BOOL_FLAGS = new Set(['--overwrite', '--dry-run']);
const opts = {};
const inputs = [];
for (let i = 1; i < argv.length; i++) {
  const a = argv[i];
  if (VALUE_FLAGS.has(a)) {
    if (i + 1 >= argv.length) usage(`${a} needs a value`);
    opts[a] = argv[++i];
  } else if (BOOL_FLAGS.has(a)) opts[a] = true;
  else if (a.startsWith('--')) usage(`unknown flag ${a}`);
  else inputs.push(a);
}
if (!inputs.length) usage('no inputs');
if (!opts['--out']) usage('--out is required');

const PRESETS = { medium: 70, high: 80, excellent: 85, 'lossless-looking': 90 };
const tRaw = opts['--target'] ?? '80';
const target = PRESETS[tRaw] ?? Number(tRaw);
if (!(target > 0 && target <= 100)) usage(`bad --target ${tRaw}`);
if (opts['--formats'] && opts['--formats'] !== 'same'
  && !opts['--formats'].split(',').every((f) => ['jpeg', 'webp', 'avif', 'png'].includes(f))) usage('bad --formats');
for (const f of ['--jobs', '--max-width', '--max-height']) {
  if (opts[f] !== undefined && !(Number.isInteger(Number(opts[f])) && Number(opts[f]) > 0)) usage(`bad ${f}`);
}

const SUPPORTED = new Set(['.png', '.jpg', '.jpeg', '.webp']);
const items = []; // { abs, root }
for (const inp of inputs) {
  const abs = resolve(inp);
  if (!existsSync(abs)) usage(`input not found: ${inp}`);
  if (statSync(abs).isDirectory()) {
    const walk = (d) => {
      for (const e of readdirSync(d, { withFileTypes: true })) {
        const p = join(d, e.name);
        if (e.isSymbolicLink()) continue;
        if (e.isDirectory()) walk(p); else if (e.isFile()) items.push({ abs: p, root: abs });
      }
    };
    walk(abs);
  } else items.push({ abs, root: dirname(abs) });
}
items.sort((a, b) => (a.abs < b.abs ? -1 : a.abs > b.abs ? 1 : 0));

const outDir = resolve(opts['--out']);
const records = [];
let bytesIn = 0, bytesOut = 0;
for (const { abs, root } of items) {
  const rel = relative(root, abs);
  const ext = extname(abs).toLowerCase();
  const bytes = lstatSync(abs).size;
  const rec = { input: abs, output: null, status: 'kept-original', reason: null, codec: 'original', quality: null,
    bytes_in: bytes, bytes_out: null, score: null, target, evals: null, ms: 0 };
  const t = performance.now();
  if (!SUPPORTED.has(ext)) {
    Object.assign(rec, { status: 'skipped', reason: 'unsupported', codec: null, bytes_in: bytes });
  } else {
    const out = join(outDir, rel);
    rec.output = out;
    bytesIn += bytes;
    if (resolve(out) === abs) Object.assign(rec, { status: 'error', reason: 'output equals input', codec: null });
    else if (existsSync(out) && !opts['--overwrite']) Object.assign(rec, { status: 'error', reason: 'output exists', codec: null });
    else {
      if (!opts['--dry-run']) { mkdirSync(dirname(out), { recursive: true }); copyFileSync(abs, out); }
      rec.bytes_out = bytes;
      bytesOut += bytes;
    }
  }
  rec.ms = Math.round(performance.now() - t);
  records.push(rec);
}

const count = (s) => records.filter((r) => r.status === s).length;
const summary = { summary: true, images: records.length, optimized: count('optimized'), kept: count('kept-original'),
  skipped: count('skipped'), errors: count('error'), bytes_in: bytesIn, bytes_out: bytesOut,
  wall_ms: Math.round(performance.now() - t0), peak_rss_bytes: process.resourceUsage().maxRSS * 1024 };

if (opts['--report']) {
  const p = resolve(opts['--report']);
  mkdirSync(dirname(p), { recursive: true });
  writeFileSync(p, [...records, summary].map((r) => JSON.stringify(r)).join('\n') + '\n');
} else {
  process.stderr.write(`${summary.images} images: ${summary.optimized} optimized, ${summary.kept} kept, `
    + `${summary.skipped} skipped, ${summary.errors} errors\n`);
}
process.exit(summary.errors ? 1 : 0);
