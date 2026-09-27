# Crumple architecture (Phase 2 design)

Status: design, 2026-09-27. Scope: Phase 2 is a CLI-first MVP. The desktop app (Tauri) and the PWA come later, but this design keeps both possible.

Evidence behind the numbers:

- [`research/rust-codecs.md`](research/rust-codecs.md), especially its "Native end-to-end probe" section.
- [`research/squoosh-inventory.md`](research/squoosh-inventory.md).
- [`../bench/baseline/RESULTS.md`](../bench/baseline/RESULTS.md), the Squoosh/jSquash baseline, pinned to the P-cores.

**How measured numbers are reported.** Native numbers are single-threaded on an i7-14700KF P-core and come from kodim01 (768x512, 0.39 MP) unless another image is named.

- Numbers marked *est.* are derived, not measured.
- Numbers marked *UNMEASURED* are still open.

## 1. Decisions at a glance

| Topic | Decision |
|---|---|
| Product focus | Batch processing, a CLI and automatic perceptual targeting, all local. Upstream Squoosh is reviving its single-image web UI (draft PR #1473 adds jpegli and an SSIMULACRA2 readout). Crumple's value is what Squoosh does not do. |
| Metric | SSIMULACRA2. The default target is **80**, which the upstream README calls "very high quality", not noticeable side by side. The presets are `medium`=70, `high`=80, `excellent`=85 and `lossless-looking`=90. |
| Choice rule | For each codec, find the lowest quality whose score meets the target, then output the **smallest file across all codecs**. Keep the original file if nothing is smaller. |
| Native codecs (Phase 2) | JPEG: C `mozjpeg` 0.10.13. WebP lossy: `libwebp-sys` 0.14.4. AVIF: `ravif` 0.13.0. PNG lossless: `oxipng` 10.2.1. |
| Decoders | `png` 0.18.1, `zune-jpeg` 0.5.15 and `image-webp` 0.2.4. `avif-decode` 3.0.0 decodes AVIF candidates for scoring only. |
| Search | A sans-IO planner that brackets from a per-codec prior, then interpolates. It stops early when the score is within +0.5 of the target. Size-bound pruning skips scoring for any encode that is already too big. |
| Parallelism | Rayon over images, largest first. Codec internals are single-threaded so outputs are deterministic. A pixel-cost memory gate (`--max-memory`, default 1 GiB) bounds concurrency. |
| WASM (Phase 3) | Hybrid. The Rust core (planner, metric, pure-Rust decoders, resize) compiles to wasm32 and drives jSquash's Emscripten codecs in a worker pool. Pure-Rust `mozjpeg-rs` and `ravif` are the fallback. |
| Licenses | Permissive only. `cargo-deny` enforces an allow-list; MPL-2.0 is allowed only for `avif-parse`. No GPL, AGPL or LGPL, and no imazen `zen*` crates. |
| Build prerequisite | **nasm ≥ 2.14** on x86_64, needed by `avif-decode` (rav1d asm) and for fast `ravif`/`mozjpeg`. Without it, only the AVIF feature fails to build. |

## 2. Crate layout

```text
crates/
  crumple-metric/   NEW (T1)  SSIMULACRA2 with a cached reference. Pure Rust, wasm-clean.
  crumple-codecs/   NEW (T2)  Encoders and decoders behind features. Native only (C code inside).
  crumple-core/     (T3)      Planner, candidate choice, EXIF orientation, resize, shared types.
                              Pure Rust, wasm-clean, no I/O.
  crumple-cli/      (T4)      The `crumple` binary: discovery, scheduling, memory gate, output,
                              JSONL report. T6 adds the native pipeline driver (pipeline.rs).
  crumple-wasm/     Phase 3   wasm-bindgen facade over core + metric for the PWA.
  crumple-desktop/  Phase 4   Tauri shell over core + codecs.
```

Dependency direction: `cli → {core, codecs, metric}` (wired in T6), and later `wasm → {core, metric}`. `core`, `codecs` and `metric` do not depend on one another. The native pipeline driver lives in `crumple-cli/src/pipeline.rs`, and the PWA will have its own driver. Crates exchange pixels as plain `&[u8]` RGBA8 (sRGB, straight alpha, row-major, no padding) plus `width`/`height`. No shared "types" crate is needed.

Why three library crates:

- The metric and planner must build for `wasm32-unknown-unknown`. `libwebp-sys`, `mozjpeg-sys` and `libdeflate-sys` cannot.
- Keeping the C code in `crumple-codecs` keeps `core` and `metric` wasm-clean without feature juggling.
- The split lets T1 to T4 proceed in parallel.

## 3. Core pipeline

```text
read bytes ─► decode (sniff: PNG/JPEG/WebP) ─► apply EXIF orientation ─► [resize: fit within WxH]
      │                                                                     │
      │                         reference = Reference::new(rgba)  ◄─────────┘  (metric cache, once)
      ▼
 size_bound = original file size
 for codec in [JPEG, WebP, AVIF] ∩ allowed:        (cheapest first; tightens the bound)
     planner = Planner::new(target, prior_q(codec, target), size_bound)
     loop planner.next():
        Encode(q) ─► bytes = encode(codec, q) ─► planner.on_encoded(q, len)
        Score(q)  ─► score = reference.score(decode(bytes)) ─► planner.on_scored(q, score)
        Finished(Found{..}) ─► keep those exact bytes; size_bound = min(size_bound, len)
 if input is PNG and "png" allowed: lossless candidate = oxipng(rgba) (score 100, no search)
 choose: the smallest passing candidate, or keep the original ─► write output + JSONL record
```

Invariants. T3 and T6 must test these.

1. **The bytes written are the bytes that were scored.** Never re-encode after the search.
2. **Every output scores at or above the target**, or is the untouched original, or is lossless.
3. **Outputs are deterministic.** The same input, flags, version and CPU architecture give identical bytes regardless of `--jobs`, core count or processing order. This is why there is no cross-image warm start, why codec thread counts are fixed, and why the metric has no parallel float reductions.
4. **Inputs are never modified**, and existing outputs are not overwritten without `--overwrite`.
5. **Scoring always decodes with the same decoder per format.** Changing the decoder moved a score by 0.11 in the probe.

The following rules apply to images with an alpha channel, ICC profiles, EXIF data and large images:

- **Alpha.**
  - An image counts as having alpha if any pixel has alpha below 255.
  - For such images, JPEG is not a candidate.
  - The metric scores composites over black and over white and returns the **minimum** of the two. SSIMULACRA2 itself is RGB-only.
- **ICC profile.**
  - The profile is carried into JPEG (`write_icc_profile`), WebP (the libwebp mux `ICCP` chunk) and PNG (oxipng `add_icc_profile`).
  - AVIF is skipped when a profile is present, because `ravif` 0.13 has no ICC API.
  - The metric runs on raw sRGB-assumed values, a known approximation for wide-gamut images.
- **EXIF.** Orientation is applied to the pixels. All other metadata is stripped in Phase 2.
- **Size limit.** `--max-pixels` defaults to 24 MP. Larger images are skipped with `reason: "too-large"` until the tiled metric exists (see §5).

## 4. Quality-target search

### 4.1 Algorithm (T3 implements this exactly)

This is a per-codec search over an integer quality `q` in `[q_min=1, q_max=100]`. It assumes size rises with `q` and score roughly rises with `q`. In the probe, score was monotonic at every point tested.

State:

- a cache `q → (bytes, score?)`;
- `lo`, the highest `q` that failed (score < target), and its score `s_lo`;
- `hi`, the lowest `q` that passed, and its score `s_hi`;
- `cap`, initially `q_max`;
- `evals`, the number of encodes.

Config (defaults): `step = 8`, `max_evals = 6`, `score_slack = 0.5`.

1. **Start** at `q0 = clamp(prior_q(codec, target), q_min, cap)`.
2. **After each encode**, if `size_bound` is set and `bytes ≥ size_bound`: set `cap = min(cap, q−1)`, mark `q` as too big, and **do not score it**. Every `q' ≥ q` is at least as large, so it could not win.
3. **Pick the next `q`.** Every pick is finally clamped to `[q_min, cap]`.
   - Nothing scored yet, but the last encode was too big: `last_q − step`.
   - Only failures seen: `lo + step`.
   - Only passes seen: `hi − step`.
   - Both seen: interpolate, `lo + round((target − s_lo)/(s_hi − s_lo) · (hi − lo))`, clamped to `[lo+1, hi−1]`.
     - If the last two interpolated picks landed on the same side, take the midpoint `(lo+hi)/2` instead. This is the bisection safeguard.
     - If the chosen `q` is already cached, take the midpoint. If that is also cached, stop.
4. **Stop**, checking the rules in this order:
   - **Found** when `hi` exists and any of these holds:
     - `hi − lo ≤ 1`;
     - `hi == q_min`;
     - `s_hi < target + score_slack`.
   - When `cap < q_min` or `lo ≥ cap`:
     - **Found** if anything passed;
     - otherwise **Pruned** if anything was too big;
     - otherwise **Unreachable**.
   - When `evals == max_evals`, the same three outcomes, in the same order.
5. **Result**: the passing candidate with the **fewest bytes** (ties go to the lower `q`), not simply `hi`. This keeps the result correct if the curve is not monotonic.

Initial-q prior (`prior_q`) is linear interpolation over these points, fitted from the kodim01 probe searches:

| Target | 70 | 80 | 90 |
|---|---:|---:|---:|
| JPEG (C mozjpeg) | 73 | 87 | 98 |
| WebP (libwebp) | 69 | 83 | 96 |
| AVIF (ravif, speed 6) | 78 | 86 | 94 |

- Below 70, the prior extrapolates with the 70→80 slope. Above 90 it uses the 80→90 slope.
- The result is clamped to 1..100.
- T6 re-fits the table on the corpus.

**Caching and early exit, in summary:**

- **Caching:**
  - The planner cache stops any `q` being encoded twice.
  - The metric computes the reference once per image and shares it across codecs.
  - Only the best candidate's bytes per codec are kept in memory.
  - A cross-run content-hash cache is deferred to Phase 3.
- **Early exit:**
  - Stop when the score is within +0.5 of the target.
  - The size bound starts at the **original file size**, so already-optimized inputs are pruned cheaply.
  - The bound then tightens across codecs: JPEG → WebP → AVIF.
  - A codec is declared unreachable when `q_max` still fails.

Worked example, kodim01 at target 80:

| Codec | Probes | Evals |
|---|---|---:|
| JPEG | q87 = 80.38 (within slack) | 1 |
| WebP | q83 = 80.41 | 1 |
| AVIF | q86 = 80.76, q78 = 70.20, q85 = 79.1 → q86 | 3 |

Naive bisection took 7 to 8 evals per codec. The prior was fitted on this image, so expect 2 to 4 evals on others (*est.*). T6 measures the real mean.

### 4.2 Cost per image

One *eval* is one encode, one decode and one score. The score column uses the stock `ssimulacra2` 0.5.1 crate.

| Codec | Native encode | Decode | Score | **Native eval** | Web encode (Squoosh baseline median) | Web score (Rust metric in wasm) | **Web eval** *est.* |
|---|---:|---:|---:|---:|---:|---:|---:|
| JPEG | 44 ms (C mozjpeg, no SIMD) | 3 | 107 | **~155 ms** | 36.2 ms | 197 ms (measured) | ~240 ms |
| WebP | 26 ms | 8 | 107 | **~141 ms** | 29.8 ms | 197 | ~235 ms |
| AVIF | 443 ms (q50) to 812 ms (q75), ravif s6 no asm | 11–20 | 107 | **~0.6–0.9 s** | 346.3 ms (libaom s6) | 197 | ~560 ms |
| JXL (deferred) | — | — | — | — | 420.1 ms | 197 | ~640 ms |
| PNG lossless | 147 ms (oxipng p2), no score | — | — | **147 ms** | 232 ms | — | 232 ms |

Measured naive bisection on kodim01 took 1.47 s (JPEG), 1.19 s (WebP) and 8.41 s (AVIF), **11.1 s per image**. The planned search should take about this long, per 0.39 MP image, single-threaded:

| Codec | Evals *est.* | Time *est.* |
|---|---:|---:|
| JPEG | 3 × 155 ms | 0.47 s |
| WebP | 3 × 141 ms | 0.42 s |
| AVIF | 3 × ~770 ms | 2.3 s |
| **Total** | | **≈ 3.2 s** |

- The reference cache in T1 should trim about 40% off each score, down to ~65 ms (*est.*), which would bring the total to ~2.9 s.
- AVIF is about 75% of the cost. `ravif` with asm (nasm) should cut that, but by how much is *UNMEASURED*.

For the whole corpus (24 Kodak images plus the 3.45 MP `example.png`):

- `example.png` alone takes ≈ 3 × (130 + 10 + 1043) + 3 × (141 + 40 + 1043) + 3 × (~4000 + 85 + 1043) ≈ **23 s** of CPU inside one task (*est.*).
- The total is ≈ 24 × 3.2 + 23 ≈ **100 CPU-s** (*est.*). Across 28 threads the wall time is bounded by the `example.png` task, so expect ≈ **25 s wall** (*est.*).

Reference points:

- The Squoosh baseline batch takes 37.23 s for 5 fixed encodes per image, single-threaded, with no scoring and no search.
- Naive bisection would be ~2.7x the planned cost.
- Phase 3 web estimate per Kodak image, per worker: ≈ 3 × (240 + 235 + 560) ms ≈ **3.1 s**, or ≈ 5 s with JXL.

## 5. Parallelism and memory

- **Level 1, images:** `rayon` `par_iter` over images, sorted by pixel count, **largest first**. This is LPT scheduling, so `example.png`-sized stragglers start at t=0.
- **Level 2, codecs within an image:** sequential, JPEG → WebP → AVIF. Cheap codecs run first so their result tightens the size bound for AVIF.
- **Level 3, codec internals:** single-threaded, for determinism:
  - `ravif` uses `with_num_threads(Some(1))`. Its output **changes with the thread count**: 37,821 B against 36,604 B in the probe.
  - `oxipng` `parallel` shares the rayon pool and still returns deterministic output.
  - Known leak: `avif-decode` starts rav1d with `available_parallelism()` threads on every decode, and that is not configurable. It is acceptable because a decode takes ~16 ms. Fixing it needs an upstream API; ask the user before opening an upstream PR.
- **Memory is the binding constraint.**
  - Measured peaks:
    - `ssimulacra2` 0.5.1 peaks at **~145 B/px**: 505 MiB for one 3.45 MP score.
    - The unbounded 28-thread probe batch peaked at **1,117 MiB**, and at 568 MiB with 8 threads.
    - Squoosh's sequential baseline peaked at 829 MiB.
  - Design, the memory gate (T4):
    - Each image costs `w·h·200 B` (the metric at ~145 B/px, plus source, candidate and encoder scratch).
    - The gate is a counting semaphore of `--max-memory` bytes (default 1 GiB). An image acquires its cost before decoding and releases it when done.
    - An image whose cost exceeds the whole budget waits until nothing else is in flight, then runs alone. It never deadlocks.
    - Blocking a rayon worker is acceptable here, because the gate is taken only at the top-level per-image task and never nested.
  - The per-image constant becomes about 100 B/px once T1's lean metric lands. T6 re-measures it.
- **Very large images:** `--max-pixels` defaults to 24 MP, which is ~3.8 GB at 160 B/px. The fix is a **tiled SSIMULACRA2** in Phase 3:
  - The per-scale statistics are pixel sums, so tiles 32-px-aligned (2^5 for 6 scales) with a ~160 px halo can reproduce the full-image score exactly.
  - That bounds memory at about 1344² px × 145 B ≈ 260 MB for any image size.
  - The PWA needs this before it can handle phone photos.

## 6. WASM strategy (Phase 3, decided now so Phase 2 does not block it)

The gaps from the research:

- no permissive pure-Rust **lossy WebP** encoder;
- no permissive pure-Rust **lossy JXL** encoder;
- no **AVIF decode** on wasm32-unknown-unknown (rav1d does not compile for it);
- C `-sys` crates need a libc sysroot to build for wasm.

Measured in this review:

- A pure-Rust wasm module (`png`, `mozjpeg-rs`, `ravif`, `ssimulacra2`, `zune-jpeg`) links and runs in Node. It is 1.56 MB, not wasm-opt'ed.
- Per 0.39 MP image: mozjpeg-rs takes 78 ms, SSIMULACRA2 197 ms, and ravif q50 614 ms.
- Its outputs are the same size as native: 73,093 B and 36,604 B.
- **jSquash's Emscripten mozjpeg and libwebp outputs are byte-identical to native C `mozjpeg` and `libwebp-sys`.** The SHA-256 matches for kodim01, kodim13 and `example`, all at q75.

| Option | Lossy WebP | Lossy JXL | AVIF | Toolchain cost for one developer | Web output = native output? | Verdict |
|---|---|---|---|---|---|---|
| **A. Hybrid.** Rust core in wasm32-unknown-unknown (wasm-bindgen) plans, decodes and scores. jSquash codecs (prebuilt npm, Apache-2.0 wrappers, BSD codecs) encode in the same worker. | yes (libwebp) | yes (libjxl) | libaom encode (MT/SIMD builds); jSquash decode for scoring | **Low.** Prebuilt codecs; only our Rust goes through wasm-bindgen. | JPEG and WebP are **byte-identical** to native (verified by SHA-256). AVIF differs (libaom on web, rav1e natively). | **Chosen** |
| B. One Emscripten build of Rust core plus C codecs (`wasm32-unknown-emscripten`) | yes | yes | any | High. Tier-2 Rust target, no wasm-bindgen, every C/C++ library built by us (Squoosh's approach). | Could match exactly | Rejected for now |
| C. wasm32-unknown-unknown plus C codecs compiled with a wasi-sdk sysroot | libwebp plausible, *UNVERIFIED* | libjxl (C++, threads) impractical | rav1e only | Medium-high, unproven | Yes | Research spike only |
| D. Browser encoders (`OffscreenCanvas.convertToBlob`) | Chromium and Firefox only | no | no | Trivial | No, varies by browser | Degrade path only |
| E. Pure-Rust subset (`mozjpeg-rs`, `ravif`, `png`/`oxipng`-freestanding, `image-webp` lossless) | no | no | ravif encode (slow); no decode | Low | JPEG within ±1.5% of native C | **Fallback** inside A |

**Consequences for Phase 2** (already reflected in the tasks):

- **The planner is sans-IO.** `next()`, `on_encoded()` and `on_scored()` let a JS worker drive it asynchronously exactly as the CLI drives it synchronously.
- **`crumple-core` and `crumple-metric` must pass `cargo check --target wasm32-unknown-unknown`** in CI.
- **Codec choice stays out of `core`.** `core` knows only an opaque codec id and the prior table.

Risks for A:

- jSquash is a single-maintainer project; its last commit was 2026-01-05.
- The threaded builds need COOP/COEP headers.
- The codec wasm is large: AVIF encode 3.5 MB, JXL encode 1.4–2.0 MB, WebP encode 0.3 MB.
- AVIF will differ between CLI and PWA unless native also moves to libaom. That is an open question, to decide after measuring `ravif` with asm against libaom at equal SSIMULACRA2.

## 7. License policy

1. **Crumple is Apache-2.0.** Every linked dependency must carry an Apache-2.0-compatible permissive license from the allow-list below.
   - MPL-2.0 is allowed per crate, by explicit exception only (today just `avif-parse`), and only unmodified.
   - GPL, AGPL, LGPL, SSPL, commercial-only and unlicensed crates are rejected.
   - LGPL is rejected because static Rust linking makes its relinking requirement impractical.
2. **Check each crate's own license, never the publisher's.** imazen publishes both BSD/MIT crates (`mozjpeg-rs`, `butteraugli`, `zensim`, `zenyuv`) and AGPL ones (`zenjpeg`, `zenwebp`, `zenavif`, `zenjxl`, `zenquant`, `jxl-encoder`, `jpegli-rs`).
3. **`cargo-deny` cannot see bundled C sources**, because `libwebp-sys` declares only MIT. Maintain `THIRD_PARTY_NOTICES.md` by hand (T6) for:
   - mozjpeg (IJG, BSD-3-Clause, zlib);
   - libwebp (BSD-3-Clause plus the Google PATENTS grant);
   - libdeflate (MIT);
   - rav1e and rav1d (BSD-2-Clause plus the AOMedia Patent License 1.0);
   - avif-parse (MPL-2.0, with a source link);
   - ssimulacra2 (BSD-2-Clause; T1 ports code from it);
   - oxipng (MIT).

   Any new `-sys` crate needs a manual review of its vendored sources.
4. **Squoosh-derived code** keeps its Apache-2.0 attribution in `NOTICE`. Do not use "Squoosh" in the product name, domain or logo.
5. **The draft `deny.toml`** below was tested with `cargo-deny` 0.20.2. `check licenses bans sources` **passes** on the full native-probe graph (4 targets, all features). In a negative test it **rejects** `imagequant` (GPL-3.0-or-later) and `dssim-core` (AGPL-3.0), each by both license and ban.

```toml
[graph]
targets = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "wasm32-unknown-unknown"]
all-features = true

[advisories]
version = 2
yanked = "deny"

[licenses]
version = 2
confidence-threshold = 0.93
# Anything not listed is rejected: GPL/AGPL/LGPL/SSPL/commercial fail by default.
allow = [
  "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "MIT", "MIT-0", "BSD-2-Clause", "BSD-3-Clause",
  "ISC", "Zlib", "0BSD", "CC0-1.0", "Unlicense", "BSL-1.0", "Unicode-3.0",
  "IJG",  # mozjpeg / mozjpeg-sys / jpeg-encoder: IJG notice required in THIRD_PARTY_NOTICES
]
exceptions = [
  # File-level copyleft. Ship unmodified, list in notices, publish any patch to its files.
  { allow = ["MPL-2.0"], crate = "avif-parse" },
]

[licenses.private]
ignore = true

[bans]
multiple-versions = "warn"
wildcards = "deny"
allow-wildcard-paths = true
deny = [
  { crate = "imagequant", reason = "GPL-3.0-or-later; use quantette" },
  { crate = "jpegxl-rs", reason = "GPL-3.0-or-later wrapper" },
  { crate = "jpegxl-sys", reason = "GPL-3.0-or-later wrapper" },
  { crate = "dssim", reason = "AGPL-3.0" },
  { crate = "dssim-core", reason = "AGPL-3.0" },
  { crate = "jpegli-rs", reason = "AGPL-3.0-or-later" },
  { crate = "zenjpeg", reason = "AGPL-3.0-only OR commercial" },
  { crate = "zenpng", reason = "AGPL-3.0-only OR commercial" },
  { crate = "zenquant", reason = "AGPL-3.0-only OR commercial" },
  { crate = "zenwebp", reason = "AGPL-3.0-only OR commercial" },
  { crate = "zenavif", reason = "AGPL-3.0-only OR commercial" },
  { crate = "zenjxl", reason = "AGPL-3.0-only OR commercial" },
  { crate = "jxl-encoder", reason = "AGPL-3.0-only OR commercial" },
]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

## 8. Phase 2 MVP scope

**In scope:**

- **`crumple optimize <INPUT>... --out <DIR>`.** Inputs are files or folders (recursive).
  - Flags:
    - `--target <score|preset>` (default 80);
    - `--formats <list|same>` (default `jpeg,webp,avif,png`; `png` means the lossless candidate and applies only to PNG inputs);
    - `--max-width` / `--max-height`, which fit within the box and only downscale, using Lanczos3;
    - `--jobs`, `--max-memory` (default 1GiB) and `--max-pixels` (default 24M);
    - `--report <file.jsonl>`, `--overwrite` and `--dry-run`.
  - `same` restricts output to the input's own family: JPEG → jpeg, PNG → png lossless, WebP → webp.
- **`crumple score <reference> <distorted>`** prints SSIMULACRA2. It is useful for users and for the bench.
- **Inputs:** PNG (8 or 16 bit, reduced to 8), JPEG and WebP. Animated inputs (APNG, animated WebP) are skipped as `animated`.
- **Candidates:** JPEG, lossy WebP, AVIF, lossless PNG, and the original.
- **Fidelity:** EXIF orientation applied, the ICC profile carried (§3), alpha handled (§3), all other metadata stripped.

**Deferred:**

| Item | Reason |
|---|---|
| JXL output | No permissive lossy Rust encoder. Needs thin libjxl (BSD) bindings, or jSquash in the PWA. |
| jpegli | The `jpegli` 0.1 binding fails to link on MSVC and showed no gain. Revisit via `google/jpegli` (see Squoosh PR #1473's build). |
| AVIF, JXL, GIF, TIFF, HEIC, SVG input; animation | Scope. |
| HDR, 16-bit output | Scope. |
| Palette quantization (`quantette`) | Scope. |
| Lossless WebP/AVIF/JXL candidates | Scope. |
| Target file size (#1422) | Scope. |
| EXIF copy-through | Scope. |
| Cross-run cache | Scope. |
| Tauri, PWA | Later phases. |

**What to measure against `bench/`** (T5 builds it, T6 fills it in). For each target in {70, 80, 90}, over the 25-image corpus:

| Metric | Baseline to compare with (`bench/baseline/results.json`, Squoosh defaults) |
|---|---|
| Hit rate: share of outputs with score ≥ target, or kept original, or lossless | Squoosh defaults have no target. Their per-codec **minimum** scores are mozjpeg 67.7, webp 59.6, avif 48.7, jxl 69.0; their medians are 72.0 / 69.2 / 62.1 / 72.3. |
| Total output bytes, median bpp, codec mix | Total bytes: mozjpeg 1,413,218; webp 1,258,088; avif 834,002; jxl 1,211,649 |
| Min and median score | As above |
| Mean evals per codec; time split between encode, decode and score | Naive bisection: 7–8 evals per codec |
| Wall time for the whole corpus | 37.23 s (5 fixed encodes per image, no scoring, 1 thread) |
| Peak RSS | 829.27 MiB (OS maxRSS) |
| Determinism: `--jobs 1` and `--jobs 28` outputs hash-identical | n/a |

## 9. Phase 2 tasks

### Prerequisites (the lead does these before dispatch, not an executor)

- **P1.** In the root `Cargo.toml`, change `members = ["crates/crumple-core", "crates/crumple-cli"]` to `members = ["crates/*"]`. New crates then join the workspace without anyone editing the root file.
- **P2.** Put nasm ≥ 2.14 on the dev box's PATH. Installing a tool needs the user's OK. Check with `nasm -v`. T2's `avif` feature cannot build without it on x86_64.
- **P3.** Make sure `bench/corpus/kodak/*.png` exists, via `node bench/corpus/fetch.mjs`. Tests that need it skip with a printed notice when it is absent.

Rules for every task:

- Touch **only** the files listed for that task.
- Pin the exact dependency versions given.
- No `unsafe` except inside `crumple-codecs`' FFI module, and in T4's single peak-RSS FFI call (`GetProcessMemoryInfo` on Windows; `getrusage` via `libc` elsewhere).
- `cargo fmt`, and `cargo clippy -p <crate> --all-targets -- -D warnings` clean.
- No new root files, except those listed for T5 and T6.
- Do not commit.
- Report the exact commands you ran and their output.
- Resolve any `Cargo.lock` conflict by keeping both sides and running `cargo check --workspace`.

T1 to T5 run in parallel. T6 runs after all five are merged.

---

### T1. `crumple-metric`: SSIMULACRA2 with a cached reference

**Files:** `crates/crumple-metric/**` only (new crate).

**Dependencies:**

- `ssimulacra2 = { version = "=0.5.1", default-features = false }`, for `Blur` and the `yuvxyb` re-exports.
- dev: `png = "=0.18.1"`, to load the corpus.

**License:** set `license = "Apache-2.0 AND BSD-2-Clause"`. Copy ssimulacra2's BSD-2-Clause text to `crates/crumple-metric/LICENSE-ssimulacra2`, and put a header comment on every ported file.

**API (exact):**

```rust
pub struct Reference { /* private */ }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricError { BadLength, SizeMismatch, TooSmall }
impl Reference {
    /// `rgba`: sRGB, straight alpha, row-major, len == width*height*4. Errors: BadLength, TooSmall (<8x8).
    pub fn new(rgba: &[u8], width: u32, height: u32) -> Result<Reference, MetricError>;
    /// Same layout and size as the reference. Returns SSIMULACRA2 (100 = identical).
    pub fn score(&self, distorted_rgba: &[u8]) -> Result<f64, MetricError>;
    pub fn width(&self) -> u32;
    pub fn height(&self) -> u32;
    pub fn has_alpha(&self) -> bool; // any reference alpha < 255
}
/// Convenience: Reference::new(reference, w, h)?.score(distorted)
pub fn ssimulacra2(reference: &[u8], distorted: &[u8], width: u32, height: u32) -> Result<f64, MetricError>;
```

**Behaviour:**

- Port the scoring loop of `ssimulacra2` 0.5.1 (`src/lib.rs`: `downscale_by_2`, `image_multiply`, `ssim_map`, `edge_diff_map`, the Msssim scoring) so that everything that depends only on the reference is computed once in `new`. That means, per scale:
  - the XYB planes;
  - the reference's blurred `mu1`;
  - the reference's blurred `sigma11`.
- Pixel conversion must match the crate exactly: `v/255.0` as f32, `TransferCharacteristic::SRGB`, `ColorPrimaries::BT709`, then `LinearRgb`, then `Xyb`.
- **Alpha:** if `has_alpha()`, keep two cached references, composited over black (0,0,0) and over white (255,255,255) (`c·a/255 + bg·(1−a/255)`, in f32 then rounded to u8). In `score`, composite the distorted image the same way **with its own alpha**, and return the minimum of the two scores. If the reference has no alpha, ignore the distorted image's alpha bytes.
- **No parallel reductions and no rayon.** Results must be bit-for-bit reproducible.

**Acceptance** (run each command and paste its output):

1. `cargo test -p crumple-metric` passes, and the tests include:
   - a) **Parity.** Apply these four distortions to the RGB channels, leaving alpha at 255:
     - D1: a 3x3 box blur, with edges clamped;
     - D2: posterize, `v & 0xF8`;
     - D3: add noise `(((i as u64 * 1103515245 + 12345) >> 16) % 13) as i32 − 6` to the byte at index `i`, clamped to 0..=255;
     - D4: a 2x nearest-neighbour downscale followed by a 2x nearest-neighbour upscale.

     For **every** `bench/corpus/kodak/*.png` and every distortion, `|Reference::score − ssimulacra2::compute_frame_ssimulacra2| ≤ 1e-4`. The crate path uses `Rgb::new(v/255.0 …, SRGB, BT709)`. Skip with `eprintln!` if the corpus is missing.
   - b) `score(reference itself)` ≥ 99.999.
   - c) **Alpha.** A 64x64 image whose left half has alpha=0 scores ≥ 99.999 against a copy whose left-half RGB was changed to random values. Changing any right-half pixel lowers the score.
   - d) **Errors.** Wrong length gives `BadLength`, 7x7 gives `TooSmall`, and a different size in `score` gives `BadLength`.
2. `cargo run -p crumple-metric --release --example bench_metric -- bench/corpus/kodak/kodim01.png` prints the median of 10 for the crate time and for `Reference::score` time. **The cached score must be ≤ 0.75 × the crate time.** Also print the peak working set in B/px (report only, no pass/fail).
3. `cargo check -p crumple-metric --target wasm32-unknown-unknown` passes.

---

### T2. `crumple-codecs`: encoders and decoders

**Files:** `crates/crumple-codecs/**` only (new crate).

**Features:** `default = ["jpeg","webp","avif","png"]`. Each feature gates its dependencies.

**Dependencies (exact):**

- `mozjpeg = "=0.10.13"` (default features: SIMD when nasm is present);
- `libwebp-sys = "=0.14.4"`;
- `ravif = { version = "=0.13.0", default-features = false, features = ["asm","threading"] }`;
- `avif-decode = { version = "=3.0.0", default-features = false }`;
- `oxipng = { version = "=10.2.1", default-features = false, features = ["parallel"] }`;
- `png = "=0.18.1"`, `zune-jpeg = "=0.5.15"`, `image-webp = "=0.2.4"`;
- `kamadak-exif = "=0.6.1"`;
- `imgref = "1"`, `rgb = "0.8"`;
- dev: `ssimulacra2 = { version = "=0.5.1", default-features = false }`.

**API (exact):**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec { Jpeg, WebpLossy, Avif, PngLossless }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format { Png, Jpeg, Webp, Avif }
pub struct Decoded { pub rgba: Vec<u8>, pub width: u32, pub height: u32,
                     pub icc: Option<Vec<u8>>, pub exif_orientation: u8 /* 1..=8, 1 if absent */,
                     pub format: Format, pub animated: bool }
#[derive(Debug)]
pub enum CodecError { UnknownFormat, Decode(String), Encode(String), Unsupported(&'static str) }
/// quality 1..=100 (ignored for PngLossless). `icc`: embed if supported, else Err(Unsupported("icc")).
pub fn encode(codec: Codec, rgba: &[u8], width: u32, height: u32, quality: u8, icc: Option<&[u8]>) -> Result<Vec<u8>, CodecError>;
/// Sniffs magic bytes. Always returns 8-bit RGBA (16-bit PNG → 8-bit; gray/palette expanded).
pub fn decode(bytes: &[u8]) -> Result<Decoded, CodecError>;
pub fn supports_alpha(codec: Codec) -> bool;   // Jpeg → false
pub fn extension(codec: Codec) -> &'static str; // "jpg","webp","avif","png"
```

**Frozen encoder settings.** "Opaque" means every alpha byte is 255. Opaque images are passed to the encoders as RGB.

| Codec | Settings |
|---|---|
| Jpeg | If the image is not opaque, return `Err(Unsupported("alpha"))`. Otherwise convert to RGB and call `mozjpeg::Compress::new(ColorSpace::JCS_RGB)`, then `set_size(w,h)`, `set_quality(q as f32)`, `set_progressive_mode()` and `set_optimize_scans(true)`. Call `start_compress(Vec::new())`, then `write_icc_profile(icc)` if `icc` is set, then `write_scanlines(rgb)` and `finish()`. Keep the library default for everything else (4:2:0, trellis on). |
| WebpLossy | `WebPEncodeRGB` or `WebPEncodeRGBA` (preset default, method 4) with `quality = q as f32`. With `icc`, wrap the result with the libwebp mux API: `WebPNewInternal(WEBP_MUX_ABI_VERSION)`, `WebPMuxSetImage`, `WebPMuxSetChunk(b"ICCP")`, `WebPMuxAssemble`. |
| Avif | `ravif::Encoder::new().with_quality(q as f32).with_alpha_quality(q as f32).with_speed(6).with_num_threads(Some(1))`. `icc` gives `Unsupported("icc")`. |
| PngLossless | `oxipng::RawImage::new(w, h, ColorType::RGBA or RGB, BitDepth::Eight, data)`, then `add_icc_profile` if set, then `create_optimized_png(&Options::from_preset(2))`. |

**Decoders.** Format by magic bytes:

- PNG `\x89PNG`: `png` with `EXPAND | STRIP_16`.
- JPEG `FF D8 FF`: `zune-jpeg`, output RGB.
- WebP `RIFF....WEBP`: `image-webp`.
- AVIF (`ftyp` with brand `avif`/`avis`): `avif-decode`. Convert `Rgb16`/`Rgba16` to 8-bit with `(v as u32*255+32767)/65535`. When built without the `avif` feature, return `Err(Unsupported("avif"))`.

Extract the ICC profile from PNG `iCCP`, JPEG APP2 (`icc_profile()`) and WebP `ICCP`. Parse the orientation tag 0x0112 with `kamadak-exif` from PNG `eXIf`, JPEG `exif()` and WebP `exif_metadata()`. Set `animated = true` for APNG (an `acTL` chunk) or animated WebP.

**Acceptance.** Tests use `bench/corpus/kodak/kodim01.png`, skipping if missing. Scores come from the dev-dep `ssimulacra2` after `decode`.

1. `cargo test -p crumple-codecs` passes, with these tests:

   | Test | Expected |
   |---|---|
   | a) Jpeg q75 | bytes within ±0.5% of **72,721**; score within ±0.2 of **71.74** |
   | b) WebpLossy q75 | bytes within ±0.5% of **77,422**; score within ±0.2 of **72.72** |
   | c) Avif q50 | two encodes byte-identical; bytes within ±3% of **36,604** |
   | d) PngLossless | decodes pixel-identical to the input RGBA; bytes ≤ 700,000 |
   | e) ICC | the blob `(0..3144).map(\|i\| (i * 7 % 251) as u8)` round-trips through Jpeg, WebpLossy and PngLossless: `decode(..).icc == Some(blob)`. Avif with ICC returns `Unsupported("icc")`. |
   | f) Orientation | a JPEG built in the test by `mozjpeg` with a hand-made APP1 Exif segment (orientation=6) decodes with `exif_orientation == 6` |
   | g) Alpha | an RGBA image with alpha < 255 round-trips alpha through WebpLossy (lossy, alpha within ±8), Avif (±8) and PngLossless (exact). `encode(Jpeg, …alpha…)` returns `Unsupported("alpha")`. |
   | h) Errors | `decode(b"garbage")` returns `UnknownFormat` |

2. `cargo build -p crumple-codecs --release` and `cargo build -p crumple-codecs --no-default-features --features jpeg,webp,png` both pass. The second must work **without nasm**.

---

### T3. `crumple-core`: planner, choice, orientation, resize

**Files:** `crates/crumple-core/**` only. This is the existing placeholder; replace `src/lib.rs`.

**Dependencies:** `fast_image_resize = "=6.1.0"` only. No codecs and no metric.

**API (exact):**

```rust
// planner.rs
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchConfig { pub target: f64, pub q_min: u8, pub q_max: u8, pub initial_q: u8,
                          pub max_evals: u8, pub step: u8, pub score_slack: f64 }
impl SearchConfig { pub fn new(target: f64, initial_q: u8) -> Self; } // q_min 1, q_max 100, max_evals 6, step 8, slack 0.5
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Found { q: u8, bytes: u64, score: f64, evals: u8 },
    Unreachable { best_q: u8, best_score: f64, evals: u8 },
    Pruned { evals: u8 },
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step { Encode(u8), Score(u8), Finished(Outcome) }
pub struct Planner { /* private */ }
impl Planner {
    pub fn new(cfg: SearchConfig, size_bound: Option<u64>) -> Planner;
    pub fn next(&mut self) -> Step;                 // idempotent until the matching on_* call
    pub fn on_encoded(&mut self, q: u8, bytes: u64); // panics if q != pending Encode
    pub fn on_scored(&mut self, q: u8, score: f64);  // panics if q != pending Score
}
/// Synchronous driver over Planner. The encode closure returns the byte length; the caller keeps the bytes.
pub fn search<E>(cfg: SearchConfig, size_bound: Option<u64>,
                 encode: impl FnMut(u8) -> Result<u64, E>, score: impl FnMut(u8) -> Result<f64, E>) -> Result<Outcome, E>;
// prior.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecKind { Jpeg, Webp, Avif }
pub fn prior_q(kind: CodecKind, target: f64) -> u8; // the table and extrapolation in ARCHITECTURE §4.1
// choose.rs
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate { pub id: u8 /* opaque codec id */, pub bytes: u64, pub passes: bool }
/// Smallest passing candidate strictly smaller than `original_bytes`; ties go to the lower id. None = keep the original.
pub fn choose(original_bytes: u64, candidates: &[Candidate]) -> Option<usize>;
// orient.rs
/// Applies EXIF orientation 1..=8 to RGBA8; returns (pixels, new_w, new_h). Values outside 1..=8 are treated as 1.
pub fn apply_orientation(rgba: &[u8], w: u32, h: u32, orientation: u8) -> (Vec<u8>, u32, u32);
// resize.rs
/// Fit within (max_w, max_h), never upscale, keep aspect ratio (round to nearest, min 1 px).
/// Lanczos3 with premultiplied alpha (fast_image_resize MulDiv). Returns the input unchanged if it already fits.
pub fn fit_within(rgba: &[u8], w: u32, h: u32, max_w: Option<u32>, max_h: Option<u32>) -> (Vec<u8>, u32, u32);
```

The planner must implement §4.1 **exactly**, including the order of the stop rules, the same-side bisection safeguard and "smallest passing bytes wins".

**Acceptance:**

1. `cargo test -p crumple-core` passes. Synthetic curves use `bytes(q) = 1000·q` unless stated.

   | Case | Curve and setup | Expected |
   |---|---|---|
   | a) linear | `score = q − 10`, target 80, initial 60 | Found q = 90, evals = 6 (60, 68, 76, 84, 92, 90) |
   | b) sigmoid | `100/(1+e^{−(q−70)/6})`, target 80, initial 50 | Found q = 79 (the true minimal passing q), evals ≤ 6 |
   | c) slack | `score = 0.9q`, target 80, initial 90 (score 81.0) | Found q = 89 (via 90, 82, 89) |
   | d) unreachable | `score = q/2`, target 80, initial 80 | Unreachable, best_q = 100, evals = 4 |
   | e) pruned | `score = q + 20`, target 80, initial 60, `size_bound = 50_000` | Pruned, evals ≤ 6; no Score step issued for any q with `bytes ≥ 50_000` |
   | f) non-monotone | `score = q − 10 + 3·sin(q as f64)`, target 80, initial 85 | Found; the returned candidate has score ≥ 80 and the fewest bytes among the evaluated passing ones |
   | g) determinism | two runs of each case | identical Step sequences |
   | h) driver | `search()` | same outcome as manual stepping in every case |
   | i) `prior_q` | | returns exactly the table values at 70/80/90 and clamps to 1..=100 |
   | j) `choose` | | smallest wins; ties go to the lower id; all ≥ original gives None; failing candidates are ignored |
   | k) `apply_orientation` | all 8 values on a 3x2 image with distinct pixels | hard-coded expected arrays |
   | l) `fit_within` | 768x512 at max 400x400; 100x50 at max 400; a 2x1 image with one transparent pixel | 400x267; unchanged; the transparent pixel's RGB does not bleed into the opaque one (premultiplied) |

2. `cargo check -p crumple-core --target wasm32-unknown-unknown` passes.

---

### T4. `crumple-cli`: command line, discovery, scheduler, memory gate, report (stubbed pipeline)

**Files:** `crates/crumple-cli/**` only. Keep the binary name `crumple`.

**Dependencies:**

- `clap = { version = "=4.6.7", features = ["derive"] }`, `rayon = "=1.12.0"`, `imagesize = "=0.15.0"`;
- `serde = { version = "=1.0.229", features = ["derive"] }`, `serde_json = "=1.0.151"`;
- `windows-sys = { version = "=0.61.2", features = ["Win32_System_ProcessStatus","Win32_System_Threading"] }` (Windows only), `libc = "=0.2.189"` (Unix only);
- dev: `tempfile = "=3.27.0"`.

Do **not** depend on `crumple-codecs` or `crumple-metric`. T6 wires those in.

**Behaviour:**

- **Subcommands:** `crumple optimize` with exactly the flags in §8, and `crumple --version`. Leave `score` to T6.
  - `--target` accepts a number in (0, 100] or `medium`/`high`/`excellent`/`lossless-looking` (70/80/85/90).
  - `--formats` accepts a comma list of `jpeg,webp,avif,png`, or `same`.
  - `--max-memory` accepts `512MiB`, `1GiB` or plain bytes. `--max-pixels` accepts `24M` or a plain number.
- **Discovery:**
  - Recurse into folders.
  - Accept `png`, `jpg`, `jpeg` and `webp`, case-insensitive. Anything else becomes `status: "skipped", reason: "unsupported"`.
  - Follow no symlinks.
  - Sort by path for stable output.
- **Output path:**
  - The path is `<out>/<path relative to the input root>` with the extension replaced by the chosen codec's (`jpg`, `webp`, `avif`, `png`). A kept original keeps its extension.
  - If two inputs map to the same output, both use `<stem>.<orig-ext>.<new-ext>`.
  - Refuse (as an error record) if the output path equals the input path.
  - An existing output file is an error unless `--overwrite` is given.
  - `--dry-run` writes nothing but still reports.
- **Scheduler:**
  - Read dimensions with `imagesize`. If that fails, use cost 0 and let the stub report an error later.
  - Sort by `w*h` descending.
  - Run on a rayon pool of `--jobs` threads (default `available_parallelism`).
  - Each task acquires `MemoryBudget` with cost `w*h*200` before calling `optimize_one` and releases it afterwards.
  - Images above `--max-pixels` become `skipped` / `too-large` without acquiring.
- **MemoryBudget** (`src/budget.rs`, public within the crate):
  - Methods: `new(capacity)` and `acquire(cost) -> Guard` (released on drop).
  - An item whose cost exceeds capacity waits until `in_use == 0`, then takes the whole budget.
  - Implemented with `Mutex` + `Condvar`, with no busy-waiting.
- **Stub:** `fn optimize_one(input: &Path, bytes: &[u8], settings: &Settings) -> ImageOutcome` returns `kept-original` and copies the file. Put it in `src/pipeline_stub.rs` so T6 can replace it.
- **Report:**
  - One JSON object per line, in completion order, to `--report`. If `--report` is absent, nothing goes to a file and a summary goes to stderr.
  - The field names and types are frozen:

    ```json
    {"input":"a/b.png","output":"out/a/b.avif","status":"optimized","reason":null,"codec":"avif","quality":86,
     "bytes_in":736501,"bytes_out":100585,"score":80.76,"target":80.0,"evals":{"jpeg":1,"webp":1,"avif":3},"ms":4210}
    ```

    - `status` is one of `optimized`, `kept-original`, `skipped` or `error`.
    - `codec` is one of `jpeg`, `webp`, `avif`, `png`, `original` or `null`.
    - Missing values are `null`.
  - The last line is `{"summary":true,"images":N,"optimized":N,"kept":N,"skipped":N,"errors":N,"bytes_in":N,"bytes_out":N,"wall_ms":N,"peak_rss_bytes":N}`.
  - `peak_rss_bytes` comes from `GetProcessMemoryInfo(...).PeakWorkingSetSize` on Windows, and `getrusage(RUSAGE_SELF).ru_maxrss` elsewhere (KiB × 1024 on Linux, bytes on macOS).
- **Exit codes:** 0 when there are no errors, 1 when any image errored, 2 on a usage error.

**Acceptance:**

1. `cargo test -p crumple-cli` passes, with these tests:
   - a) Path mapping, including the collision rule and the "output equals input" refusal.
   - b) Discovery filters and sorting.
   - c) **MemoryBudget stress.** 16 threads × 200 random costs (seeded LCG) against capacity 1000, including costs of 1500. A shared atomic `in_use` never exceeds 1000, except while an oversize item runs alone. The test finishes in under 10 s.
   - d) `--target`, `--max-memory` and `--max-pixels` parsing.
   - e) End to end with `env!("CARGO_BIN_EXE_crumple")` on a temp folder of 3 small PNGs written by the test: `optimize <dir> --out <out> --report r.jsonl` exits 0, writes 3 files and writes 4 JSONL lines, the last with `"summary":true`. A second run without `--overwrite` exits 1 with 3 error records.
2. `cargo run -p crumple-cli -- --version` prints `crumple 0.0.0`.

---

### T5. Tooling: `deny.toml`, CI and the Crumple bench harness

**Files:** `deny.toml` (new, root), `.github/workflows/ci.yml` (new), and `bench/crumple/**` (new). Do not touch `bench/baseline/**` or `bench/corpus/**`.

**`deny.toml`:** copy it byte-for-byte from §7.

**CI (`ci.yml`)**, on `push` and `pull_request`:

- A `test` matrix over `windows-latest`, `ubuntu-latest` and `macos-latest`:
  - install nasm (`ilammy/setup-nasm@v1`);
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets -- -D warnings`;
  - `cargo test --workspace`.
- A `deny` job using `EmbarkStudios/cargo-deny-action@v2`.
- A `wasm` job on ubuntu:
  - `rustup target add wasm32-unknown-unknown`;
  - `cargo check -p crumple-core -p crumple-metric --target wasm32-unknown-unknown`.
- Pin actions by major version.
- No secrets, and no publishing steps.

**Bench (`bench/crumple/run.mjs`, Node 24, no npm dependencies):**

- Arguments: `--crumple <path-to-binary>` (default `target/release/crumple(.exe)`), `--targets 70,80,90`, `--runs 1`.
- For each target it runs `<crumple> optimize bench/corpus/kodak --out bench/out/crumple/t<T> --target <T> --report bench/out/crumple/t<T>.jsonl --overwrite`.
  - On Windows, pin to the P-cores the same way `bench/baseline/run.mjs` does (affinity mask `0xffff`). Read that file and reuse its method.
- It parses the JSONL and writes `bench/crumple/results.json` and `bench/crumple/RESULTS.md`, with one row per target and these columns:
  - images, hit rate, total bytes and median bpp;
  - min and median score;
  - codec mix;
  - mean evals per codec;
  - wall seconds and peak RSS in MiB.
- Under that, a fixed comparison table reads `bench/baseline/results.json`: per codec, total bytes and min/median score.
- A determinism check (`--determinism`) runs target 80 twice, with `--jobs 1` and with `--jobs <all>`, and compares SHA-256 of every output file.
- Also write `bench/crumple/fake-crumple.mjs`, a stand-in that honours the §8 flags and the T4 report schema by copying inputs. Add `bench/crumple/README.md` with the usage.

**Acceptance:**

1. `node bench/crumple/run.mjs --crumple "node bench/crumple/fake-crumple.mjs" --targets 80` produces both results files with 25 images, 100% "kept" and no crash. (Accept a command string for `--crumple`.)
2. `cargo deny check licenses bans sources` passes on the current workspace. Install it with `cargo install cargo-deny --version 0.20.2 --locked --root <scratch dir>` if it is missing; do not install it globally.
3. Paste the full `ci.yml` into your report for the lead to review. No offline YAML or Actions validator is available on the dev box, so do not install one.

---

### T6. Integration and measurement (sequential, after T1–T5 are merged)

**Files:**

- `crates/crumple-cli/src/pipeline.rs` (new);
- edits to `crates/crumple-cli/src/main.rs` and `crates/crumple-cli/Cargo.toml` (add `crumple-core`, `crumple-codecs`, `crumple-metric`);
- delete `pipeline_stub.rs`;
- `crates/crumple-core/src/prior.rs` (the re-fitted table);
- `THIRD_PARTY_NOTICES.md` (new, per §7);
- `README.md` ("Building": nasm);
- `bench/crumple/RESULTS.md` and `results.json`, regenerated;
- `docs/ARCHITECTURE.md`, §4.1 prior table only.

**Work:**

- Implement §3 exactly, using `Planner` with real `encode`/`decode`/`Reference::score`, `choose`, `apply_orientation` and `fit_within`.
- Add `crumple score <ref> <dist>`.
- Run the bench at 70, 80 and 90, plus `--determinism`.
- Re-fit the `prior_q` table on the corpus (median solved q per codec and target) and update §4.1 of this document with the new values.

**Acceptance:**

1. `cargo test --workspace` passes.
2. In the bench:
   - hit rate is 100%;
   - the determinism check is identical;
   - peak RSS ≤ `--max-memory` + 128 MiB at both the default 1 GiB and at 512 MiB;
   - mean evals ≤ 4 per codec at target 80;
   - corpus wall time at target 80 ≤ 60 s on the dev box.
3. Report the total bytes at target 70 against Squoosh-default mozjpeg (1,413,218 B, min score 67.7).

## 10. Open questions and risks

1. **AVIF encoder.**
   - Without asm, `ravif` is ~1.5–2x slower than jSquash libaom-wasm at similar SSIMULACRA2 (*est.* from kodim01).
   - Measure `ravif` with asm in T6. If it is still more than 2x slower than native libaom would be, evaluate a libaom binding in Phase 3. That would also make CLI and PWA AVIF identical.
2. **Upstream fixes worth offering.** Each is a public contribution and needs the user's OK first.
   - `avif-decode`: an `asm` feature, and a thread-count API.
   - `jpegli-sys`: the MSVC `Release/` link path.
   - `ravif`: ICC support.
3. **Tiled metric**, before the PWA and for images above 24 MP (§5).
4. **Metric trust.** SSIMULACRA2 is tuned on photographic content. Before promising "visually lossless" for screenshots and line art, validate targets on a non-photo corpus (only `example.png` today).
5. **Squoosh PR #1473.** If it merges, reuse its jpegli Emscripten build for the PWA, and keep Crumple's messaging on batch, CLI and automation.
