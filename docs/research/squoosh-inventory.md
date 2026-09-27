# Squoosh inventory: what it does, what Crumple can reuse, and what users want

- Source: local read-only clone `D:\contrubution\squoosh`, HEAD `e8d35e0f` (2024-08-19, "Replace deprecated terser plugin (#1423)"), branch `dev`.
- GitHub data pulled on 2026-09-27 with `gh api` from `GoogleChromeLabs/squoosh`.
- Repo status: the brief calls Squoosh "archived", but the GitHub API returns `archived: false`. The last commit on `dev` is still `e8d35e0f` from 2024-08-19. There are 153 open issues (not counting PRs) and 25,958 stars. The licence is Apache-2.0.
- **[Reviewer] Upstream is being revived. This note missed it.** Jake Archibald has had a **draft PR [#1473](https://github.com/GoogleChromeLabs/squoosh/pull/1473) "Updating AVIF & JXL"** open since 2026-05-29. It is branch `ja/update-codecs`, with 41 commits and 229 files, and was last pushed on 2026-09-23 (checked 2026-09-27). Its commits:
  - update every codec build to current Emscripten;
  - **add jpegli and make it the default JPEG encoder** (commit "Make JPEGLI the default", 2026-08-27);
  - **add an SSIMULACRA2 metric** ("Add SSIMULACRA 2 metric 'easter egg'", plus `codecs/ssimulacra2` and `src/features/metrics`);
  - add progressive AVIF, JPEG→JXL transcoding, and a zopfli effort level;
  - update the quantizer and resizers, and drop the wasm AVIF decoder.

  It is still a single-image web UI, with no batch mode, CLI or quality-target search. Until it merges, `dev` is unchanged. See §5 for what this means for Crumple.
- The CLI and libSquoosh (`cli/`, `libsquoosh/`) were deleted in commit `13a185d`, "Remove CLI / libsquoosh (#1321)" (2023-01-03). There is no Node or batch path left in the repo.
- **[Reviewer] Spot-check, 2026-09-27.** These all match:
  - HEAD `e8d35e0` and its date;
  - commit `13a185d` and its date;
  - `archived: false`, 25,958 stars and 153 open issues;
  - the reaction counts for #1408 (55), #280 (55), #736 (25), #1406 (20) and #1259 (15);
  - the #1472 quote;
  - the imagequant and hqx licence findings.

  One addition, the upstream revival in draft PR #1473, is below.

---

## 1. Codecs and processors (`src/features/`)

Defaults are taken from each `shared/meta.ts`. The UI ranges come from each `client/index.tsx`. By default the left side of the compare view shows the **Original Image** (`encoderState: undefined`) and the right side shows **MozJPEG** with default options (`src/client/lazy-app/Compress/index.tsx` L285-315). All processors default to `enabled: false` (generated in `lib/feature-plugin.js` L234).

### 1.1 Encoders

#### MozJPEG (`encoders/mozJPEG`, label "MozJPEG", `image/jpeg`, `.jpg`)
| UI label | Option | Default | UI range / choices |
|---|---|---|---|
| Quality | `quality` | 75 | 0-100 |
| Channels (adv.) | `color_space` | YCbCr | Grayscale / RGB / YCbCr |
| Auto subsample chroma (YCbCr only) | `auto_subsample` | true | checkbox |
| Subsample chroma by (if auto off) | `chroma_subsample` | 2 | 1-4 |
| Separate chroma quality | `separate_chroma_quality` | false | checkbox |
| Chroma quality | `chroma_quality` | 75 | 0-100 |
| "Pointless spec compliance" | `baseline` | false | checkbox |
| Progressive rendering (if not baseline) | `progressive` | true | checkbox |
| Optimize Huffman table (if baseline) | `optimize_coding` | true | checkbox |
| Smoothing | `smoothing` | 0 | 0-100 |
| Quantization | `quant_table` | 3 (ImageMagick) | 0 Annex K, 1 Flat, 2 MSSIM Kodak, 3 ImageMagick, 4 PSNR-HVS-M Kodak, 5 Klein, 6 Watson, 7 Ahumada, 8 Peterson |
| Trellis multipass | `trellis_multipass` | false | checkbox |
| Optimize zero block runs | `trellis_opt_zero` | false | checkbox (if multipass) |
| Optimize after trellis quantization | `trellis_opt_table` | false | checkbox |
| Trellis quantization passes | `trellis_loops` | 1 | 1-50 |
| (not in UI) | `arithmetic` | false | - |

#### WebP (`encoders/webP`, label "WebP", `image/webp`)
The defaults mirror libwebp's `WebPConfig` (see the comment in meta.ts).

| UI label | Option | Default | UI range |
|---|---|---|---|
| Lossless | `lossless` | 0 | checkbox |
| Effort (lossless) | preset index to `[method, quality]` from `kLosslessPresets` | preset 6 = `[4,75]` | 0-9 |
| Slight loss (lossless) | `near_lossless` | 100 | 0-100 (UI shows `100 - value`) |
| Discrete tone image (lossless) | `image_hint` | 0 (DEFAULT); checkbox sets GRAPH (3) | checkbox |
| Effort (lossy) | `method` | 4 | 0-6 |
| Quality (lossy) | `quality` | 75 | 0-100, step 0.1 |
| Compress alpha (adv.) | `alpha_compression` | 1 | checkbox |
| Alpha quality | `alpha_quality` | 100 | 0-100 |
| Alpha filter quality | `alpha_filtering` | 1 | 0-2 |
| Auto adjust filter strength | `autofilter` | 0 | checkbox |
| Filter strength | `filter_strength` | 60 | 0-100 |
| Strong filter | `filter_type` | 1 | checkbox |
| Filter sharpness | `filter_sharpness` | 0 | 0-7 |
| Sharp RGB→YUV conversion | `use_sharp_yuv` | 0 | checkbox |
| Passes | `pass` | 1 | 1-10 |
| Spatial noise shaping | `sns_strength` | 50 | 0-100 |
| Preprocess | `preprocessing` | 0 | None / Segment smooth / Pseudo-random dithering |
| Segments | `segments` | 4 | 1-4 |
| Partitions | `partitions` | 0 | 0-3 |
| Preserve transparent data | `exact` | 0 | checkbox |
| (not in UI) | `target_size` 0, `target_PSNR` 0, `show_compressed` 0, `partition_limit` 0, `emulate_jpeg_size` 0, `thread_level` 0, `low_memory` 0, `use_delta_palette` 0 | | |

#### AVIF (`encoders/avif`, label "AVIF", `image/avif`)
| UI label | Option | Default | UI range |
|---|---|---|---|
| Lossless | derived: `quality==100 && qualityAlpha in {-1,100} && subsample==3` | false | checkbox |
| Quality | `quality` | 50 | 0-99 (100 is reserved for lossless) |
| Effort | `speed = 10 - effort` | speed 6 (effort 4) | 0-10 |
| Subsample chroma (adv.) | `subsample` | 1 (4:2:0) | 0 = 4:0:0, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4 |
| Sharp YUV Downsampling (4:2:0 only) | `enableSharpYUV` | false | checkbox |
| Separate alpha quality | `qualityAlpha` | -1 (same as colour) | 0-99 |
| Extra chroma compression | `chromaDeltaQ` | false | checkbox |
| Sharpness | `sharpness` | 0 | 0-7 |
| Noise synthesis | `denoiseLevel` | 0 | 0-50 |
| Tuning | `tune` | auto | Auto / PSNR / SSIM |
| Log2 of tile rows / cols | `tileRowsLog2` / `tileColsLog2` | 0 / 0 | 0-6 |

#### JPEG XL (`encoders/jxl`, label "JPEG XL (beta)", `image/jxl`)
| UI label | Option | Default | UI range |
|---|---|---|---|
| Lossless | derived | false | checkbox |
| Slight loss (lossless) | `lossyPalette` (+ modular) | false | checkbox |
| Quality | `quality` | 75 | 0-99.9, step 0.1 |
| Alternative lossy mode | `lossyModular` | false | checkbox |
| Auto edge filter / Edge preserving filter | `epf` | -1 (auto) | auto, or 0-3 |
| Optimise for decoding speed | `decodingSpeedTier` | 0 | 0-4 |
| Noise equivalent to ISO | `photonNoiseIso` | 0 | 0-50000, step 100 |
| Progressive rendering | `progressive` | false | checkbox |
| Effort | `effort` | 7 | 1-9 |

#### WebP v2 (`encoders/wp2`, label "WebP v2 (unstable)", `image/webp2`, `.wp2`)
| UI label | Option | Default | UI range |
|---|---|---|---|
| Lossless | derived | false | checkbox |
| Slight loss | derived from quality | - | 0-5, step 0.1 |
| Quality | `quality` | 75 | 0-95, step 0.1 |
| Separate alpha quality / Alpha Quality | `alpha_quality` | 75 | 0-100 |
| Passes (adv.) | `pass` | 1 | 1-10 |
| Spatial noise shaping | `sns` | 50 | 0-100 |
| Error diffusion | `error_diffusion` | 0 | 0-100 |
| Subsample chroma | `uv_mode` | Auto | Auto / Vary / Half / Off |
| Color space | `csp_type` | YCoCg | YCoCg / YCbCr / YIQ |
| Random matrix | `use_random_matrix` | false | checkbox |
| Effort | `effort` | 5 | 0-9 |

#### OxiPNG (`encoders/oxiPNG`, label "OxiPNG", `image/png`)
| UI label | Option | Default | UI range |
|---|---|---|---|
| Interlace | `interlace` | false | checkbox |
| Effort | `level` | 2 | 0-6 |

#### QOI (`encoders/qoi`, label "QOI", `image/qoi`)
It has no options (`defaultOptions = {}`); the client UI is 11 lines long.

#### Browser-native encoders (canvas `toBlob`; no wasm)
| Encoder | Options | Default |
|---|---|---|
| Browser JPEG (`image/jpeg`) | `quality` | 0.75 (range 0-1, step 0.01) |
| Browser PNG (`image/png`) | none | - |
| Browser GIF (`image/gif`) | none | - (only works where the browser can encode GIF) |

The UI does not explain the difference between "Browser JPEG" and MozJPEG. Issue #332 raises exactly this.

### 1.2 Decoders (`src/features/decoders/*`, wasm fallback)
Squoosh sniffs the MIME type first. If `canDecodeImageType()` (the browser) can decode the file, it uses `builtinDecode` (createImageBitmap/`<img>`). If not, it falls back to a wasm decoder (`src/client/lazy-app/Compress/index.tsx` L91-120). The wasm decoders are **avif, jxl, qoi, webp, wp2**. JPEG, PNG and GIF are always decoded by the browser. SVG is rasterised in the browser, and the resize method is then forced to `vector`.

### 1.3 Preprocessor: rotate (`preprocessors/rotate`)
- `rotate`: 0 | 90 | 180 | 270, default **0**. It is applied once for both sides and driven by the rotate button in the output toolbar.
- It uses a ~500-byte hand-written Rust wasm module (`codecs/rotate/rotate.wasm`) through raw `WebAssembly.instantiate`. It does not use wasm-bindgen.
- It does **not** read EXIF orientation (see issue #299).

### 1.4 Processors

#### Resize (`processors/resize`)
| UI label | Option | Default |
|---|---|---|
| Method | `method` | `lanczos3` (or `vector` for SVG input) |
| Preset | width/height presets | 25%, 33.33%, 50%, 100%, 200%, 300%, 400%, Custom |
| Width / Height | `width`, `height` | the source dimensions |
| Premultiply alpha channel (worker methods only) | `premultiply` | true |
| Linear RGB (worker methods only) | `linearRGB` | true |
| Maintain aspect ratio | UI state | true |
| Fit method (when aspect is not maintained) | `fitMethod` | `stretch` (the other choice is `contain`, which centre-crops) |

Methods:
- **Worker (wasm, `codecs/resize`)**:
  - `lanczos3`
  - `mitchell`
  - `catrom` (Catmull-Rom)
  - `triangle` (bilinear)
- **`hqx` (pixel art)**:
  1. Upscales by an integer factor `clamp(ceil(max(w/W, h/H)), 1..4)` with `codecs/hqx`.
  2. Resizes to the exact target with `catrom` (`features/processors/resize/worker/resize.ts` L66-105).
- **Browser (canvas `imageSmoothingQuality`)**:
  - `browser-pixelated`
  - `browser-low`
  - `browser-medium`
  - `browser-high`
- **`vector`**: re-rasterises the SVG at the target size. It is only offered when the input is SVG.

#### Quantize, UI label "Reduce palette" (`processors/quantize`, libimagequant)
| UI label | Option | Default | Range |
|---|---|---|---|
| Type (hidden "extended settings") | `zx` | 0 (Standard) | Standard / ZX (a ZX Spectrum-style palette; `zx_quantize` is custom code in `codecs/imagequant/imagequant.cpp`) |
| Colors | `maxNumColors` | 256 | 2-256 |
| Dithering | `dither` | 1.0 | 0-1, step 0.01 |

---

## 2. Codec builds (`codecs/`)

There are two build paths:
- **C++**: `build-cpp.sh` runs Docker with `cpp.Dockerfile`, which is `emscripten/emsdk:2.0.34`, `-O3 -flto`, Embind, `ALLOW_MEMORY_GROWTH`, `PTHREAD_POOL_SIZE=navigator.hardwareConcurrency`, then `emmake make`.
- **Rust**: `build-rust.sh` runs Docker with `rust.Dockerfile`. That image is `rust:1.47` by default, uses wasm-pack v0.12.1 and wasm-opt/clang taken from `emsdk:2.0.8`, and runs `wasm-pack build --target web`.

Build variants are chosen at runtime with `wasm-feature-detect` (simd) and `worker-shared/supports-wasm-threads`:
- threads: `_mt`
- simd: `_simd`
- both: `_mt_simd`

The threaded builds need COOP/COEP headers (set in `src/static-build/index.tsx` `_headers`).

| Dir | Upstream | Pinned version / commit | Build | Upstream licence | Permissive? |
|---|---|---|---|---|---|
| `avif` | libavif + libaom (encode and decode via aom; dav1d not used) + libsharpyuv from libwebp | libavif **v1.0.1**; aom **v3.7.0**; libwebp `e2c85878f6a3` for sharpyuv (`codecs/avif/Makefile` L1-17) | Emscripten C++ | libavif BSD-2-Clause; aom BSD-2-Clause **plus the AOM Patent License 1.0**; libwebp BSD-3-Clause | Yes (note the AOM patent licence) |
| `jxl` | libjxl | commit `9f544641ec83f6abd9da598bdd08178ee8a003e0` (2022-01-18), fetched with git plus submodules | Emscripten C++ | BSD-3-Clause at that commit (verified at the ref) | Yes |
| `mozjpeg` | mozilla/mozjpeg | **v3.3.1** (latest upstream is v4.1.1) | Emscripten C++ | IJG + BSD-3-Clause + zlib (libjpeg-turbo terms; `codecs/mozjpeg/LICENSE.codec.md`) | Yes (IJG needs an attribution notice) |
| `webp` | webmproject/libwebp | commit `d2e245ea9e959a5a79e1db0ed2085206947e98f2` (2020-11-24) | Emscripten C++ | BSD-3-Clause (plus the Google PATENTS grant) | Yes |
| `wp2` | libwebp2 (chromium.googlesource) | commit `413df7caeca5013fa9a51401660f7efd8572e0ae` | Emscripten C++ | Apache-2.0 (LICENSE fetched at that commit) | Yes. The format itself is experimental and unstable. |
| `qoi` | phoboslab/qoi | commit `8d35d93cdca85d2868246c2a8a80a1e2c16ba2a8` (2023-09-10) | Emscripten C++ | MIT | Yes |
| `imagequant` | ImageOptim/libimagequant | **2.12.1** (`codecs/imagequant/Makefile` L1) | Emscripten C++; the prebuilt `imagequant.wasm` is committed | **GPL-3.0-or-later** (pngquant-original parts are BSD-like). Current upstream (4.x, Rust) is still `GPL-3.0-or-later`, with commercial licences sold separately. **[Reviewer] Verified**: the `COPYRIGHT` at tag 2.12.1 and on `main` both say the Kornel Lesiński changes are "licensed under GPL v3 or later", and crates.io `imagequant` 4.4.1 declares `GPL-3.0-or-later`. Draft PR #1473 moves this codec to a Rust build (`codecs/imagequant/Cargo.toml`), which is still libimagequant and still GPL. | **NO: copyleft (GPLv3).** It cannot be shipped in an Apache-2.0 app without the whole distributed work falling under GPLv3. |
| `hqx` | `hqx` crate from CryZe/wasmboy-rs (git tag **v0.1.3**, lock `d7cbae67`). The local README says v0.1.2. | Cargo.toml `hqx = {git=..., tag="v0.1.3"}` | Rust wasm-pack | The repo declares Apache-2.0 and MIT. The `hqx/` subdirectory has no licence file or headers. | **FLAG (provenance).** The code is a Rust translation of Maxim Stepin's hqx (same `interp*` helpers, YUV thresholds and per-pixel case tables). The original C hqx is **LGPL-2.1-or-later**. The Apache/MIT labelling on the translation is doubtful. Treat it as LGPL-derived until clean-room replaced. **[Reviewer] Verified** at tag v0.1.3. The repo root has `LICENSE-APACHE` and `LICENSE-MIT`, and `hqx/` holds only `Cargo.toml` (no `license` field) and `src/`. `common.rs` has the hqx YUV thresholds `0x00300000` / `0x00000700` / `0x00000006` and the `interp1/2/6/7/9/10` helpers of the LGPL reference. |
| `oxipng` | shssoichiro/oxipng | crate **9.0.0** (lock), with libdeflater 1.19.0 and rayon 1.8.0. Built on nightly Rust, pinned by image digest in package.json. | Rust wasm-pack; two outputs, `pkg` and `pkg-parallel` (wasm-bindgen-rayon, atomics) | MIT | Yes |
| `resize` | PistonDevelopers/resize | crate **0.5.5** | Rust wasm-pack | MIT | Yes |
| `rotate` | Squoosh's own `rotate.rs` (no deps) | n/a | Rust `cargo build --target wasm32-unknown-unknown` plus `wasm-opt -Os` | Apache-2.0 (Squoosh) | Yes |
| `png` | image-rs/image-png | crate **0.16.7** | Rust wasm-pack | MIT OR Apache-2.0 | Yes. The web app does not use it; it was only used by the removed libSquoosh. |
| `visdif` | google/butteraugli | commit `71b18b636b9c7d1ae0c1d3730b85b3c127eb4511` | Emscripten C++ | Apache-2.0 | Yes. The web app does not use it (it was a libSquoosh auto-quality helper). |

Licence risk summary:
- **imagequant (GPLv3) is the only clearly non-permissive codec.**
- **hqx has an unclear provenance (probably LGPL-2.1-derived).**
- Everything else is BSD, MIT, Apache or IJG/zlib.
- The AOM patent licence and the Google PATENTS files are defensive-termination grants, not copyleft.
- Squoosh's `LICENSE` is Apache-2.0, yet it ships a GPL wasm module. Crumple should not repeat that.

The pins are stale:
- mozjpeg 3.3.1 dates from 2018.
- libjxl is from January 2022; the latest is v0.12.0.
- libavif is 1.0.1; the latest is v1.4.2.
- libwebp is from November 2020.
- Emscripten 2.0.34 and Rust 1.47 are both from 2020-2021.

---

## 3. App architecture

- **Build**: Rollup 2 with custom plugins in `lib/` (see below). Preact 10 for the UI and PostCSS modules for CSS. TypeScript 4.4. `static-build` prerenders `index.html`, the manifest and `_headers` with preact-render-to-string.
- **Two-stage loading**:
  - `src/client/initial-app` is small. It holds the prerendered intro (`src/shared/prerendered-app/Intro`), the `<file-drop>` element (`file-drop-element`), paste via `navigator.clipboard.read`, and the demo images.
  - The heavy `src/client/lazy-app/Compress` is dynamically imported only once the user picks an image.
- **Workers via Comlink**:
  - `lazy-app/worker-bridge/index.ts` wraps a single `Worker` (`omt:../../../features-worker`) with `comlink.wrap`.
  - Calls are serialised through a promise queue.
  - Each call takes an `AbortSignal`, and aborting **terminates the worker**. That is how in-flight encodes are cancelled when a slider moves.
  - An idle worker is terminated after 10 s.
  - Two WorkerBridges are used, one per side.
- **Feature registry by code generation**: `lib/feature-plugin.js` globs `src/features/{encoders,decoders,processors,preprocessors}/*`. It generates:
  - `encoderMap`
  - `defaultProcessorState` and `defaultPreprocessorState`
  - the features-worker entry that exposes every `worker/*.ts` default export as a Comlink method
  - `worker-bridge/meta` method names

  Adding a codec means adding a folder with `shared/meta.ts`, `client/index.tsx` and `worker/*.ts`.
- **Lazy codec loading inside the worker**: each worker module does `await import('codecs/...')` on first use and picks the `_mt`, `_simd` or `_mt_simd` build by feature detection. Nothing heavy loads until that codec is chosen.
- **Service worker and offline** (`src/sw/`):
  - On install it caches the "basics", which are the initial app, the Compress chunk and the sw-bridge (`to-cache.ts`).
  - It caches **all codecs only once the user has interacted**. This relies on an `idb-keyval` flag `user-interacted` and a `cache-all` message from `lazy-app/sw-bridge`.
  - Only the codec variants the browser can use are cached (threads/simd detection, and whether AVIF/WebP decode natively via `tiny.avif`/`tiny.webp`).
  - The versioned cache is `static-<VERSION>`. The `dynamic` cache is for `/c/demo-*`.
  - Update flow: `skip-waiting` message with a snack-bar prompt.
  - The SW is registered only in production.
- **Web Share Target**:
  - The manifest `share_target` POSTs `multipart/form-data` field `file` (`image/*`) to `/?share-target` (`src/static-build/index.tsx` L83-96).
  - The SW intercepts the POST (`sw/index.ts` L62-69) and redirects to `/?share-target`. It then waits for a `share-ready` message from the page and `postMessage`s the `File` to the client (`sw/util.ts` `serveShareTarget`).
  - `initial-app/App` removes the query and loads the image.
  - Open issue #1503 reports that shared files arrive empty on Chrome 153 on Android and suspects a Chrome regression.
- **Side-by-side compare**:
  - `Compress/Output` renders a `<two-up>` custom element (`Output/custom-els/TwoUp`), which is a draggable split handle built on `pointer-tracker`. Its orientation is horizontal on desktop and vertical on mobile, it has a `legacy-clip-compat` attribute, and the split is kept as a relative position.
  - It contains two `<pinch-zoom>` elements (`Output/custom-els/PinchZoom`), one per side, kept in sync. Events on the right-hand one are redirected to the left so a two-finger pinch across the split works.
  - The toolbar has zoom in/out, a numeric zoom %, a toggle between a checkerboard and a solid background, and rotate.
  - Each side has its own processor and encoder pipeline and its own `ResultCache` (`Compress/result-cache.ts`), so returning to earlier settings is instant.
- **Per-side options UI** (`Compress/Options`):
  - Resize and Reduce palette toggles, then an encoder select and that encoder's options component.
  - Buttons: "Copy settings to other side", "Save side settings" and "Import saved side settings", stored in `localStorage` keys `leftSideSettings`/`rightSideSettings`.
  - Shared form widgets live in `Options/{Range,Checkbox,Select,Toggle,Expander,Revealer}`.
- **Results/download**: `Compress/Results` shows the output size with a percentage delta and a download link, which is an `<a download>` of the blob.
- **Single-image model**: all state is tied to one image, and there is no queue. Batch processing was PR #1428 (sequential downloads) and was never merged. **[Reviewer]** #1428 is still *open* and unmerged (created 2024-10-02).
- **Privacy**: images never leave the device, but the README states that Google Analytics collects visitor data and before/after sizes.
- **`lib/` contents**: it holds only **Rollup build plugins and scripts** (1,401 lines in total). There is no runtime library.
  - `feature-plugin.js` (feature codegen)
  - `client-bundle-plugin.js`
  - `entry-data-plugin.js`
  - `sw-plugin.js`
  - `css-plugin.js`
  - `initial-css-plugin.js`
  - `data-url-plugin.js`
  - `url-plugin.js`
  - `emit-files-plugin.js`
  - `resolve-dirs-plugin.js`
  - `node-external-plugin.js`
  - `simple-ts.js`
  - `run-script.js`
  - `move-output.js`
  - `omt.ejs` (the off-main-thread worker loader template)

  The former libSquoosh Node API lived in `libsquoosh/` and was deleted in #1321.

### Reuse vs rewrite

| Area | Verdict | Reason |
|---|---|---|
| `TwoUp` split slider and synced `PinchZoom` UX | **Reuse the concept or port the code** (Apache-2.0, framework-free custom elements) | Users repeatedly call side-by-side compare Squoosh's unique value (#1472 body). It is small and self-contained. |
| Per-codec option sets, defaults and ranges (tables in §1) | **Reuse as a spec** | These are tuned, user-tested defaults, for example the AVIF effort inversion and the WebP lossless presets. |
| Embind C++ wrappers (`codecs/*/enc/*.cpp`, `dec/*.cpp`) and Makefiles | **Reuse as a starting point, then re-pin** | The libraries are 2-6 years old and the toolchain is Emscripten 2.0.34. Rebuild them against current libavif, libjxl, mozjpeg 4.x and libwebp. |
| Worker bridge (Comlink, AbortSignal, terminate-on-abort) | **Reuse the pattern** | It is simple and effective for cancellation. Batch processing needs a worker **pool**, not one queue per side. |
| Feature folder codegen in `lib/feature-plugin.js` | **Rewrite** | It is tied to Rollup 2 and its custom plugins. A modern bundler (Vite) plus a plain registry module can do the same. |
| Rollup plugin suite in `lib/` | **Rewrite (drop)** | It is bespoke and dated (Rollup 2, PostCSS 7, TS 4.4). |
| Preact single-image `Compress` state machine | **Rewrite** | It hard-codes exactly two sides and one image. Crumple needs N images with shared or per-image settings. |
| Service worker caching of codec variants | **Reuse the idea** | Precaching only the variants the device can use is a good offline strategy. Re-implement it with a modern SW tool. |
| Share target | **Reuse**, extend to `files` with multiple entries | Today it takes a single `file`. |
| imagequant | **Replace** | GPLv3. Options are a permissive quantizer, a clean-room implementation, or making it a separately licensed optional plugin. |
| hqx | **Replace or drop** | Provenance is unclear (LGPL origin). |
| Google Analytics | **Drop** | It conflicts with a local-first, privacy-focused positioning. |

---

## 4. User demand (open issues, ranked by total reactions)

Method:
- `gh api --paginate repos/GoogleChromeLabs/squoosh/issues?state=open&per_page=100`, excluding PRs, gave **153 open issues**.
- They were sorted by `reactions.total_count`.
- Ranks 21-29 are all tied at 3 reactions. The list below keeps the first five in API sort order; #1503, #299, #622 and #85 also have 3.

| # | Issue | Reactions | Summary |
|---|---|---|---|
| 1 | [#1408 Support for jpegli](https://github.com/GoogleChromeLabs/squoosh/issues/1408) | 55 | Add the jpegli JPEG encoder from libjxl. |
| 2 | [#280 Release codecs on NPM](https://github.com/GoogleChromeLabs/squoosh/issues/280) | 55 | Publish the wasm codecs as npm packages for build-time use instead of imagemin binaries. |
| 3 | [#736 Transparency removal](https://github.com/GoogleChromeLabs/squoosh/issues/736) | 25 | Control how alpha is flattened (background colour) for formats without transparency. |
| 4 | [#1406 Bulk image upload & processing](https://github.com/GoogleChromeLabs/squoosh/issues/1406) | 20 | Apply the same settings to many files without dragging them in one by one. Community forks exist (squoosh-multiple-export, "Squiish"). |
| 5 | [#1259 Batch / Bulk process](https://github.com/GoogleChromeLabs/squoosh/issues/1259) | 15 | Bulk-compress a folder, for example phone photos. |
| 6 | [#1020 Contribution Pairing](https://github.com/GoogleChromeLabs/squoosh/issues/1020) | 12 | A meta issue inviting contributors to pair with the team. It is not a feature request. |
| 7 | [#1240 Squoosh as a VS Code extension](https://github.com/GoogleChromeLabs/squoosh/issues/1240) | 11 | Compress images from inside the editor. |
| 8 | [#1377 Support Progressive AVIF](https://github.com/GoogleChromeLabs/squoosh/issues/1377) | 10 | Expose the progressive (layered) AVIF encoding that libavif supports. |
| 9 | [#314 Crop with frame by mouse](https://github.com/GoogleChromeLabs/squoosh/issues/314) | 10 | Interactive crop, plus copying the result back to the clipboard. |
| 10 | [#1405 Batch compression support](https://github.com/GoogleChromeLabs/squoosh/issues/1405) | 7 | Promotes a third-party desktop app built on Squoosh that does batch compression. Counts as a demand signal. |
| 11 | [#1417 Options to crop after resizing](https://github.com/GoogleChromeLabs/squoosh/issues/1417) | 6 | Fit modes beyond Stretch/Contain, with a choice of which region to keep (cover and anchor). |
| 12 | [#1392 Dark mode](https://github.com/GoogleChromeLabs/squoosh/issues/1392) | 5 | Avoid the white flash with a dark theme. |
| 13 | [#1092 Add TIFF support](https://github.com/GoogleChromeLabs/squoosh/issues/1092) | 4 | Decode TIFF input. |
| 14 | [#1247 Preserving Metadata](https://github.com/GoogleChromeLabs/squoosh/issues/1247) | 4 | Keep EXIF (a photographer currently re-copies it with exiftool). |
| 15 | [#1449 FAQ links to non-existent /cli](https://github.com/GoogleChromeLabs/squoosh/issues/1449) | 4 | The wiki still points to the deleted CLI. |
| 16 | [#1471 Can we use the app with an API?](https://github.com/GoogleChromeLabs/squoosh/issues/1471) | 4 | Wants a local API for programmatic use. |
| 17 | [#332 Unclear options](https://github.com/GoogleChromeLabs/squoosh/issues/332) | 4 | Users don't understand codec names such as "Browser WebP" vs "WebP" or the option meanings. |
| 18 | [#348 Better color management](https://github.com/GoogleChromeLabs/squoosh/issues/348) | 4 | ICC profiles are dropped and sRGB is not tagged; colour handling is inconsistent. |
| 19 | [#741 Control the crop process](https://github.com/GoogleChromeLabs/squoosh/issues/741) | 4 | Contain always crops from the centre; wants an anchor or offset. |
| 20 | [#802 HDR / wide colour gamut support](https://github.com/GoogleChromeLabs/squoosh/issues/802) | 4 | Canvas is 8-bit sRGB, so HDR/WCG data is lost for AVIF/JXL. |
| 21 | [#1040 Don't download all codecs](https://github.com/GoogleChromeLabs/squoosh/issues/1040) | 3 | Let users choose which codecs to download or cache (size concern). |
| 22 | [#1122 Integrate with WordPress media upload](https://github.com/GoogleChromeLabs/squoosh/issues/1122) | 3 | Compress inside the WordPress upload flow. |
| 23 | [#1237 Cmd-V paste listener](https://github.com/GoogleChromeLabs/squoosh/issues/1237) | 3 | Support a `paste` event so pasting doesn't trigger a clipboard permission prompt. |
| 24 | [#1410 Tooltips explaining settings](https://github.com/GoogleChromeLabs/squoosh/issues/1410) | 3 | Inline help for each codec option. |
| 25 | [#1419 Chinese language support](https://github.com/GoogleChromeLabs/squoosh/issues/1419) | 3 | i18n. |
| (tie) | [#1503](https://github.com/GoogleChromeLabs/squoosh/issues/1503), [#299](https://github.com/GoogleChromeLabs/squoosh/issues/299), [#622](https://github.com/GoogleChromeLabs/squoosh/issues/622), [#85](https://github.com/GoogleChromeLabs/squoosh/issues/85) | 3 each | Share Target files arrive empty on Chrome/Android; EXIF orientation ignored; "memory access out of bounds" on large (7952x5304) images; MozJPEG lossless transforms. |

Below the top 25 there is more demand on the same themes:
- #1422: set quality by target file size.
- #1070: trim transparent pixels.
- #1302: QOI, now shipped.
- #1279: HDR.
- #1331: Squoosh as an invisible-iframe service.
- #1248 and #1390: keep or choose output filenames.
- #960 and #1367: AVIF and WebP colour shifts.
- #1191: PNG brightness varies across browsers.

### Themes (reactions summed over the top 25 plus the ties)
- **Batch / bulk processing: 42.** #1406 (20), #1259 (15) and #1405 (7). Also the unmerged PR #1428 and closed #301.
- **CLI / npm / API / integrations: 77.** #280 (55), #1240 (11), #1471 (4), #1122 (3) and #1449 (4, dead CLI link). Also #1331.
- **New or updated codecs and formats: 76.** #1408 jpegli (55), #1377 progressive AVIF (10), #1092 TIFF (4), #85 MozJPEG lossless (3) and #802 HDR (4).
- **Editing: crop, fit, alpha: 45.** #736 transparency (25), #314 crop (10), #1417 (6) and #741 (4). Also #1070.
- **Fidelity (metadata, colour, orientation): 11.** #1247 (4), #348 (4) and #299 (3). Also #960, #1191 and #1367.
- **UI, UX and help: 21.** #332 (4), #1410 (3), #1392 dark mode (5) and #1419 i18n (3). Also #1237 paste (3) and #1040 codec download size (3).
- **Bugs and robustness: 6.** #622 OOM on large images (3) and #1503 share target (3).
- **Meta: 12.** #1020 contributor pairing.

### Issue #1472 "Abandoned?" (opened 2026-05-18 by Ciancy28)
- **The reporter** notes that there has been no activity for about two years and that jpegli is still missing. They say they value Squoosh mainly for its many encoders plus the side-by-side comparison, and they ask for an alternative.
- **Maintainer reply**: Jake Archibald (COLLABORATOR), 2026-05-18, [comment](https://github.com/GoogleChromeLabs/squoosh/issues/1472#issuecomment-4478977526). In summary:
  - The people who built Squoosh left Google.
  - Google refused either to hand the project over or to maintain it.
  - He still has commit and publish access and plans to update the JPEG XL and AVIF codecs "at some point soon".

  Key quote: "Google refused to hand the project over, or maintain it." **[Reviewer]** Verified verbatim against comment 4478977526 (jakearchibald, COLLABORATOR, 2026-05-18T15:05:16Z). The reporter's summary and the two follow-ups (Ciancy28 asking for jpegli, juliobbv-p for AVIF) also match.
- **[Reviewer] Follow-through**: eleven days later Jake opened draft PR #1473, which updates the AVIF, JXL and other codecs and adds jpegli and SSIMULACRA2. See the repo-status note at the top.
- **Follow-ups** from two users thank him and ask for jpegli (Ciancy28) and for AVIF updates (juliobbv-p).
- **Forking or the name**: #1472 **does not mention** forking, renaming or trademarks, and no maintainer statement on those topics was found anywhere.
  - A search of the issues for "fork", "trademark", "rename" and "name squoosh" found no maintainer comments on them.
  - The only trademark question is from a user in PR #1252 (self-hosting via Docker, closed): "Is the name squoosh subject to trademarks mentioned in the license?" It got no maintainer answer.
- **Related maintainer positions on batch processing**:
  - #301: Surma (2019) said the point of Squoosh is visual inspection and suggested using a CLI for batches. Jake (2020) said "the hard part is UI" and listed open UX problems. The issue was closed when the (now deleted) CLI shipped.
  - PR #1428 (2024-10): Surma welcomed the bulk-processing PR, preferred sequential downloads over zipping for now, and suggested File System Access API progressive enhancement later. The PR was never merged.

---

## 5. Implications for Crumple

### Table stakes (Squoosh already does these; users will expect parity)
- All processing happens locally in the browser via wasm, with no uploads, and works offline as an installable PWA with a service-worker codec cache.
- The encoders:
  - MozJPEG
  - WebP (lossy and lossless)
  - AVIF
  - JPEG XL
  - OxiPNG
  - QOI
  - browser-native JPEG/PNG
- Each encoder needs Squoosh's advanced options and sensible defaults (§1).
- Wasm decoders for formats the browser can't decode (AVIF, JXL, WebP, QOI).
- Resize with Lanczos3, Mitchell, Catmull-Rom, triangle and browser methods, including premultiply and linear-RGB, presets, aspect lock, stretch/contain and SVG vector resize.
- Palette reduction with colour count and dithering.
- Rotate.
- A **side-by-side split compare with synced pinch-zoom**, plus a live size delta. Users single this out as Squoosh's differentiator.
- Instant re-encode with cancellation, and per-setting result caching.
- Input by drag-and-drop, file picker, paste, demo images and the Web Share Target.
- Save and import settings, and copy settings between sides.
- Threaded and SIMD codec variants, which need COOP/COEP hosting.

### Unmet requests (open, with high demand, that Crumple could own)
1. **Batch / bulk processing**: many files, one settings profile, original filenames kept, and zip or File System Access output. This is the headline "batch optimizer" gap. There were 42 reactions across the top issues and several community forks.
2. **CLI / npm codecs / local API**: 55 reactions on #280 plus the related issues. The CLI was deleted in 2023.
3. **New codecs**: jpegli (55, the top request), progressive AVIF, current libavif/libjxl/mozjpeg 4.x, MozJPEG lossless transforms, and TIFF input.
4. **Alpha handling**: flatten onto a chosen colour (#736, 25 reactions) and trim transparent borders (#1070).
5. **Crop and fit**: an interactive crop, plus cover/anchor fit modes (#314, #1417, #741).
6. **Fidelity**: keep EXIF/ICC metadata, honour EXIF orientation, correct colour management and sRGB tagging, and eventually HDR/WCG.
7. **Target file size or quality search** (#1422).
8. **Usability**: tooltips and explanations for options, dark mode, i18n, a Cmd-V paste listener, and codec download on demand.
9. **Robustness**: handle very large images (OOM, #622) and batch memory pressure. The Squiish fork added a batch-size limit for this.

### [Reviewer] Upstream revival changes the positioning

Draft PR #1473 (see the top of this note) would give Squoosh's web app four things:

- current codecs;
- **jpegli as the default JPEG encoder** (the #1 request, #1408);
- **an on-screen SSIMULACRA2 score**;
- progressive AVIF (#1377).

If it merges, item 3 of the list above (new codecs) is largely met upstream *for single images in the browser*. Item 7 is only touched: Squoosh would show a score, but it would not search for settings that meet one. Three things are still unmet there and are the gaps Crumple can own:

- batch processing (#1406, #1259, #1405);
- a CLI and local API (#280, #1471);
- automatic codec and quality selection against a perceptual target (#1422 asks for the file-size variant).

So:

- **Position Crumple as "batch + CLI + automatic quality targeting, local-first"**, not as a replacement for an abandoned Squoosh.
- Track #1473. Its jpegli build (`codecs/jpegli/Makefile`, Emscripten, pinned `google/jpegli` commit `031a0077`) is a ready reference for a later Crumple PWA jpegli codec.
- Its quantizer still uses GPL `imagequant` 4.4 (`codecs/imagequant/Cargo.toml` in the PR), so the licence constraints below are unchanged.

### Licensing constraints for Crumple (Apache-2.0)
- **Do not ship libimagequant.** It is GPL-3.0-or-later in both 2.12.1 and current 4.x. Use a permissive quantizer, or isolate it as an optional, separately licensed add-on.
- **Do not ship the `hqx` crate as-is.** It appears to be a translation of LGPL-2.1 hqx. Drop pixel-art upscaling or use a clean-room or permissively licensed scaler.
- Bundle a licence and notice file covering:
  - IJG (mozjpeg)
  - BSD notices (libwebp, libavif, aom, libjxl)
  - the AOM Patent License 1.0
  - Squoosh's Apache-2.0 NOTICE and attribution for any ported code
- Drop Google Analytics to stay consistent with a local-first privacy claim.

### Trademark and naming constraints
- **The maintainers have made no stated position** on forking or reusing the name. #1472 and every search above turned up nothing. The one user question about the name (PR #1252) went unanswered.
- **Apache-2.0 §6 (Trademarks) applies anyway.** The licence does not grant permission to use the licensor's trade names or marks, except to describe the origin of the work.
  - Crumple should not use "Squoosh" in its product name, domain or logo, and should not imply endorsement by Google.
  - Factual statements such as "successor to / inspired by Squoosh (Apache-2.0, © Google)" in the README and NOTICE are fine.
- Squoosh itself is Google-owned: copyright is "Google Inc." and contributions require Google's CLA. Jake Archibald said Google refused to hand the project over, so a sanctioned transfer of the name or domain should not be expected.
