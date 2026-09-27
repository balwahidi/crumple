#!/usr/bin/env node
// Downloads the Kodak Lossless True Color Image Suite (kodim01..kodim24.png)
// into bench/corpus/kodak/ and copies Squoosh's codecs/example.png alongside.
//
// Integrity: on the first run every file's SHA-256 is written to
// bench/corpus/checksums.json (trust-on-first-use). On later runs every file,
// whether freshly downloaded or already on disk, is verified against that list
// and the script exits non-zero on any mismatch.
//
// Usage: node corpus/fetch.mjs [--squoosh <path to squoosh clone>]
// The clone defaults to $SQUOOSH_DIR, then a `squoosh` folder next to this repo.
// checksums.json pins example.png as of Squoosh commit e8d35e0f (2024-08-19).

import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, writeFile, access } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const kodakDir = join(here, 'kodak');
const checksumsPath = join(here, 'checksums.json');
const BASE_URL = 'https://r0k.us/graphics/kodak/kodak/';

const argIdx = process.argv.indexOf('--squoosh');
const squooshRoot =
  argIdx > -1 ? process.argv[argIdx + 1] : process.env.SQUOOSH_DIR || join(here, '..', '..', '..', 'squoosh');
const examplePng = join(squooshRoot, 'codecs', 'example.png');

const names = Array.from({ length: 24 }, (_, i) => `kodim${String(i + 1).padStart(2, '0')}.png`);

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');
const exists = (p) => access(p).then(() => true, () => false);

async function download(url, attempts = 3) {
  let lastErr;
  for (let i = 1; i <= attempts; i++) {
    try {
      const res = await fetch(url, { headers: { 'user-agent': 'crumple-bench/0.0 (corpus fetch)' } });
      if (!res.ok) throw new Error(`HTTP ${res.status} ${res.statusText}`);
      const buf = Buffer.from(await res.arrayBuffer());
      // PNG signature check so an HTML error page is never stored as an image.
      if (!buf.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) {
        throw new Error(`response is not a PNG (content-type ${res.headers.get('content-type')})`);
      }
      return buf;
    } catch (err) {
      lastErr = err;
      if (i < attempts) await new Promise((r) => setTimeout(r, 1000 * i));
    }
  }
  throw new Error(`download failed for ${url}: ${lastErr?.message ?? lastErr}`);
}

async function main() {
  await mkdir(kodakDir, { recursive: true });
  const known = (await exists(checksumsPath)) ? JSON.parse(await readFile(checksumsPath, 'utf8')) : null;
  const sums = {};
  const failures = [];

  for (const name of names) {
    const dest = join(kodakDir, name);
    let buf;
    if (await exists(dest)) {
      buf = await readFile(dest);
    } else {
      try {
        buf = await download(BASE_URL + name);
        await writeFile(dest, buf);
        console.log(`downloaded ${name} (${buf.length} bytes)`);
      } catch (err) {
        failures.push(`${name}: ${err.message}`);
        console.error(`FAILED ${name}: ${err.message}`);
        continue;
      }
    }
    sums[`kodak/${name}`] = sha256(buf);
  }

  const exampleDest = join(kodakDir, 'example.png');
  try {
    await copyFile(examplePng, exampleDest);
    sums['kodak/example.png'] = sha256(await readFile(exampleDest));
    console.log(`copied ${examplePng}`);
  } catch (err) {
    failures.push(`example.png: ${err.message}`);
    console.error(`FAILED to copy example.png from ${examplePng}: ${err.message}`);
  }

  if (failures.length) {
    console.error(`\n${failures.length} file(s) could not be obtained. Checksums NOT written.`);
    process.exit(1);
  }

  if (!known) {
    await writeFile(checksumsPath, JSON.stringify({ algorithm: 'sha256', source: BASE_URL, files: sums }, null, 2) + '\n');
    console.log(`\nfirst run: wrote ${Object.keys(sums).length} checksums to ${checksumsPath}`);
    return;
  }

  let bad = 0;
  for (const [file, hash] of Object.entries(sums)) {
    const expected = known.files[file];
    if (!expected) {
      console.error(`NO CHECKSUM for ${file}`);
      bad++;
    } else if (expected !== hash) {
      console.error(`CHECKSUM MISMATCH ${file}\n  expected ${expected}\n  actual   ${hash}`);
      bad++;
    }
  }
  if (bad) {
    console.error(`\n${bad} file(s) failed verification`);
    process.exit(1);
  }
  console.log(`verified ${Object.keys(sums).length} files against ${checksumsPath}: all OK`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
