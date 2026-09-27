#!/usr/bin/env node
// Crumple bench harness: runs `crumple optimize` over the Kodak corpus at several targets,
// parses the T4 JSONL reports and writes bench/crumple/results.json and RESULTS.md,
// with a fixed comparison against bench/baseline/results.json (Squoosh defaults).
//
//   node bench/crumple/run.mjs [--crumple <path or command string>] [--targets 70,80,90] [--runs 1]
//                              [--determinism] [--no-pin]
//
// --crumple defaults to target/release/crumple(.exe). A command string such as
// "node bench/crumple/fake-crumple.mjs" is split on whitespace (double quotes group words).
// --determinism additionally runs target 80 with --jobs 1 and --jobs <all usable CPUs>
// and compares the SHA-256 of every output file.
// On Windows the harness pins itself to P-cores (affinity mask 0xffff, as in
// bench/README.md for the baseline); the crumple child processes inherit it.

import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, openSync, readFileSync, readSync, closeSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const benchDir = join(here, '..');
const repoDir = join(benchDir, '..');
const corpusDir = join(benchDir, 'corpus', 'kodak');
const outDir = join(benchDir, 'out', 'crumple');

const args = process.argv.slice(2);
const flag = (n) => args.includes(n);
const opt = (n, d) => { const i = args.indexOf(n); return i > -1 ? args[i + 1] : d; };

const defaultBin = join(repoDir, 'target', 'release', process.platform === 'win32' ? 'crumple.exe' : 'crumple');
const CRUMPLE = splitCommand(opt('--crumple', `"${defaultBin}"`));
// CPUs this process may run on. Rust's available_parallelism() ignores the Windows affinity
// mask (it reports 28 under the 0xFFFF pin), so crumple is always told --jobs explicitly.
const TARGETS = opt('--targets', '70,80,90').split(',').map((s) => Number(s.trim()));
const RUNS = Number(opt('--runs', 1));
if (TARGETS.some((t) => !(t > 0 && t <= 100))) throw new Error(`bad --targets ${opt('--targets')}`);
if (!Number.isInteger(RUNS) || RUNS < 1) throw new Error(`--runs must be a positive integer`);
if (!existsSync(corpusDir)) throw new Error(`corpus missing: ${corpusDir} (run node bench/corpus/fetch.mjs)`);

function splitCommand(s) {
  const parts = [];
  for (const m of s.matchAll(/"([^"]*)"|(\S+)/g)) parts.push(m[1] ?? m[2]);
  if (!parts.length) throw new Error('empty --crumple');
  return parts;
}

// --- P-core pinning (same method as bench/README.md / baseline) -------------
function pinAndRecord() {
  let scheduling = {};
  try {
    if (process.platform === 'win32') {
      if (!flag('--no-pin') && os.cpus().length > 16) {
        execFileSync('powershell.exe', ['-NoProfile', '-Command',
          `(Get-Process -Id ${process.pid}).ProcessorAffinity = 0xFFFF`], { encoding: 'utf8' });
      }
      const mask = execFileSync('powershell.exe', ['-NoProfile', '-Command',
        `[int64](Get-Process -Id ${process.pid}).ProcessorAffinity`], { encoding: 'utf8' }).trim();
      scheduling = { affinityMask: `0x${BigInt(mask).toString(16)}` };
    } else if (process.platform === 'linux') {
      scheduling = { cpusAllowedList: readFileSync('/proc/self/status', 'utf8').match(/Cpus_allowed_list:\s*(.+)/)?.[1] ?? null };
    }
  } catch (e) { scheduling = { error: String(e?.message ?? e) }; }
  return scheduling;
}

// --- helpers ------------------------------------------------------------------
const median = (xs) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b), m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const round = (x, d = 2) => (x == null ? null : Math.round(x * 10 ** d) / 10 ** d);

function pngSize(path) {
  try {
    const fd = openSync(path, 'r');
    const b = Buffer.alloc(24);
    readSync(fd, b, 0, 24, 0);
    closeSync(fd);
    if (b.readUInt32BE(0) !== 0x89504e47) return null;
    return { w: b.readUInt32BE(16), h: b.readUInt32BE(20) };
  } catch { return null; }
}

function runCrumple(extra) {
  const [cmd, ...pre] = CRUMPLE;
  const t = performance.now();
  const r = spawnSync(cmd, [...pre, ...extra], { cwd: repoDir, encoding: 'utf8', stdio: ['ignore', 'inherit', 'pipe'] });
  const wallMs = performance.now() - t;
  if (r.error) throw new Error(`failed to start ${CRUMPLE.join(' ')}: ${r.error.message}`);
  if (r.status !== 0) process.stderr.write(r.stderr ?? '');
  return { status: r.status, wallMs };
}

function readReport(path) {
  const lines = readFileSync(path, 'utf8').split('\n').filter((l) => l.trim());
  const objs = lines.map((l) => JSON.parse(l));
  const summary = objs.find((o) => o.summary === true) ?? null;
  return { records: objs.filter((o) => o.summary !== true), summary };
}

// A hit is an output that meets the target, a kept original, or a lossless PNG. Every image
// the run should have handled counts, so errors and too-large skips are misses; only files that
// are not images at all (skipped as unsupported) leave the denominator.
function isHit(r, target) {
  if (r.status === 'kept-original') return true;
  if (r.status !== 'optimized') return false;
  return r.codec === 'png' || (typeof r.score === 'number' && r.score >= target);
}

function analyse(target, records, summary, wallMsRuns, exitCodes) {
  const n = records.length;
  const images = records.filter((r) => !(r.status === 'skipped' && r.reason === 'unsupported'));
  const hit = images.filter((r) => isHit(r, target)).length;
  const codecMix = {};
  const evalSum = {}, evalRuns = {};
  for (const r of records) {
    if (r.codec && (r.status === 'optimized' || r.status === 'kept-original')) codecMix[r.codec] = (codecMix[r.codec] ?? 0) + 1;
    // Mean over the searches that ran: a codec with 0 evals was not searched for that image
    // (JPEG for alpha, AVIF with ICC, --formats), and averaging those zeros in would flatter it.
    for (const [c, k] of Object.entries(r.evals ?? {})) {
      if (k > 0) { evalSum[c] = (evalSum[c] ?? 0) + k; evalRuns[c] = (evalRuns[c] ?? 0) + 1; }
    }
  }
  const meanEvals = Object.fromEntries(Object.entries(evalSum).map(([c, s]) => [c, round(s / evalRuns[c])]));
  const bpps = [];
  for (const r of records) {
    const d = r.input ? pngSize(resolve(repoDir, r.input)) : null;
    if (d && r.bytes_out != null) bpps.push((r.bytes_out * 8) / (d.w * d.h));
  }
  const scores = records.map((r) => r.score).filter((s) => typeof s === 'number');
  const totalBytes = records.reduce((a, r) => a + (r.bytes_out ?? 0), 0);
  return {
    target,
    images: n,
    statuses: Object.fromEntries(['optimized', 'kept-original', 'skipped', 'error']
      .map((s) => [s, records.filter((r) => r.status === s).length])),
    hitRate: images.length ? round(hit / images.length, 4) : null,
    totalBytes,
    medianBpp: round(median(bpps), 3),
    minScore: scores.length ? round(Math.min(...scores)) : null,
    medianScore: round(median(scores)),
    codecMix,
    meanEvals,
    wallSeconds: round(median(wallMsRuns) / 1000, 3),
    wallSecondsRuns: wallMsRuns.map((m) => round(m / 1000, 3)),
    reportWallMs: summary?.wall_ms ?? null,
    peakRssMiB: summary?.peak_rss_bytes != null ? round(summary.peak_rss_bytes / 2 ** 20) : null,
    exitCodes,
  };
}

function hashTree(dir) {
  const out = {};
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else out[relative(dir, p).split('\\').join('/')] = createHash('sha256').update(readFileSync(p)).digest('hex');
    }
  };
  if (existsSync(dir)) walk(dir);
  return out;
}

function baselineComparison() {
  const p = join(benchDir, 'baseline', 'results.json');
  if (!existsSync(p)) return null;
  const b = JSON.parse(readFileSync(p, 'utf8'));
  return b.summary.filter((s) => s.status === 'ok').map((s) => {
    const sc = b.records.filter((r) => r.codec === s.codec && typeof r.ssimulacra2 === 'number').map((r) => r.ssimulacra2);
    return { codec: s.codec, totalBytes: s.totalOutputBytes, minScore: sc.length ? round(Math.min(...sc), 1) : null,
      medianScore: s.medianSsimulacra2 };
  });
}

// --- main -----------------------------------------------------------------------
const startedAt = new Date().toISOString();
const scheduling = pinAndRecord();
const JOBS = os.availableParallelism(); // after pinning, so it counts only the allowed CPUs
scheduling.jobs = JOBS;
mkdirSync(outDir, { recursive: true });

const rows = [];
for (const T of TARGETS) {
  const out = join(outDir, `t${T}`);
  const report = join(outDir, `t${T}.jsonl`);
  const wall = [], codes = [];
  for (let i = 0; i < RUNS; i++) {
    // Start clean: a failed run must not leave the previous report or outputs to be read back.
    rmSync(out, { recursive: true, force: true });
    rmSync(report, { force: true });
    const r = runCrumple(['optimize', relative(repoDir, corpusDir), '--out', relative(repoDir, out),
      '--target', String(T), '--jobs', String(JOBS), '--report', relative(repoDir, report), '--overwrite']);
    wall.push(r.wallMs);
    codes.push(r.status);
  }
  if (!existsSync(report)) throw new Error(`target ${T}: crumple wrote no report (exit ${codes.at(-1)})`);
  const { records, summary } = readReport(report);
  const row = analyse(T, records, summary, wall, codes);
  rows.push(row);
  console.log(`target ${T}: ${row.images} images, hit ${(row.hitRate * 100).toFixed(1)}%, ${row.totalBytes} bytes, ${row.wallSeconds} s`);
}

let determinism = null;
if (flag('--determinism')) {
  const all = Math.max(2, JOBS);
  const res = {}, codes = {};
  for (const j of [1, all]) {
    const out = join(outDir, `det-j${j}`);
    rmSync(out, { recursive: true, force: true });
    codes[j] = runCrumple(['optimize', relative(repoDir, corpusDir), '--out', relative(repoDir, out), '--target', '80',
      '--jobs', String(j), '--report', relative(repoDir, join(outDir, `det-j${j}.jsonl`)), '--overwrite']).status;
    res[j] = hashTree(out);
  }
  const files = [...new Set([...Object.keys(res[1]), ...Object.keys(res[all])])].sort();
  const mismatches = files.filter((f) => res[1][f] !== res[all][f]);
  const identical = mismatches.length === 0 && files.length > 0 && codes[1] === 0 && codes[all] === 0;
  determinism = { target: 80, jobs: [1, all], exitCodes: [codes[1], codes[all]], files: files.length, identical, mismatches };
  console.log(`determinism: ${files.length} files, ${mismatches.length} mismatches`);
}

const baseline = baselineComparison();
const results = {
  startedAt, finishedAt: new Date().toISOString(),
  // Repo-relative when possible, so results.json does not record local absolute paths.
  crumple: CRUMPLE.map((p) => (p.startsWith(repoDir) ? relative(repoDir, p).split(sep).join('/') : p)).join(' '),
  runsPerTarget: RUNS,
  env: { cpuModel: os.cpus()[0]?.model?.trim(), logicalCores: os.cpus().length, os: `${os.type()} ${os.release()} ${os.arch()}`,
    node: process.version, scheduling },
  targets: rows,
  determinism,
  baseline,
};
writeFileSync(join(here, 'results.json'), JSON.stringify(results, null, 2) + '\n');

const fmtObj = (o) => Object.entries(o).map(([k, v]) => `${k} ${v}`).join(', ') || '-';
const na = (x) => (x == null ? '-' : x);
let md = `# Crumple bench results\n\n`
  + `Generated by \`bench/crumple/run.mjs\` on ${results.finishedAt}. Binary: \`${results.crumple}\`. `
  + `Runs per target: ${RUNS}. Scheduling: \`${JSON.stringify(scheduling)}\`.\n\n`
  + `| Target | Images | Hit rate | Total bytes | Median bpp | Min score | Median score | Codec mix | Mean evals per codec | Wall s | Peak RSS MiB |\n`
  + `|---:|---:|---:|---:|---:|---:|---:|---|---|---:|---:|\n`;
for (const r of rows) {
  md += `| ${r.target} | ${r.images} | ${r.hitRate == null ? '-' : (r.hitRate * 100).toFixed(1) + '%'} | ${r.totalBytes.toLocaleString('en-US')} `
    + `| ${na(r.medianBpp)} | ${na(r.minScore)} | ${na(r.medianScore)} | ${fmtObj(r.codecMix)} | ${fmtObj(r.meanEvals)} `
    + `| ${na(r.wallSeconds)} | ${na(r.peakRssMiB)} |\n`;
}
if (baseline) {
  md += `\n## Baseline (Squoosh defaults, \`bench/baseline/results.json\`)\n\n`
    + `| Codec | Total bytes | Min score | Median score |\n|---|---:|---:|---:|\n`;
  for (const b of baseline) md += `| ${b.codec} | ${b.totalBytes.toLocaleString('en-US')} | ${na(b.minScore)} | ${na(b.medianScore)} |\n`;
}
if (determinism) {
  md += `\n## Determinism\n\nTarget 80, \`--jobs 1\` vs \`--jobs ${determinism.jobs[1]}\`: ${determinism.files} files, `
    + `${determinism.identical ? 'all SHA-256 identical' : `${determinism.mismatches.length} mismatches: ${determinism.mismatches.join(', ')}`}.\n`;
}
writeFileSync(join(here, 'RESULTS.md'), md);
console.log(`wrote ${relative(repoDir, join(here, 'results.json'))} and ${relative(repoDir, join(here, 'RESULTS.md'))}`);
if (determinism && !determinism.identical) process.exitCode = 1;
