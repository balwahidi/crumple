# Crumple bench

A reproducible baseline: Squoosh's own codecs, at Squoosh's own default settings,
measured on a fixed corpus. It gives Crumple numbers to beat. Results are in
[`baseline/RESULTS.md`](baseline/RESULTS.md), with raw data in `baseline/results.json`.

## Reproduce

Requirements: Node 24+, and a Rust toolchain for the metric.

```sh
cd bench

# 1. Metric: SSIMULACRA2 (Rust port). The default features pull in
#    VapourSynth for video and fail to link on Windows without it, so turn them off.
cargo install ssimulacra2_rs --no-default-features

# 2. Codecs: exact pinned jSquash versions from package.json
npm ci            # or: npm install

# 3. Corpus: 24 Kodak PNGs + Squoosh's codecs/example.png
#    (the Squoosh clone defaults to $SQUOOSH_DIR, then ../../squoosh next to this repo)
node corpus/fetch.mjs --squoosh /path/to/squoosh

# 4. Run (about 4 minutes pinned to P-cores; writes baseline/results.json and baseline/RESULTS.md)
node baseline/run.mjs
```

Options for `run.mjs`:

- `--runs N`: encode repetitions per image and codec (default 3).
- `--only kodim01,kodim13`: restrict to images whose name contains any of the
  comma-separated strings, for quick checks. A restricted run writes
  `out/results.partial.json` and `out/RESULTS.partial.md`, and never touches
  the committed baseline.
- Set `SSIMULACRA2_BIN` if the binary is not on `PATH`.

### Stable timings on hybrid CPUs

On CPUs with performance and efficiency cores, the OS can move the benchmark to
an efficiency core mid-run, and encode times then roughly double. On the i7-14700KF
used for the baseline, logical CPUs 0-15 are P-cores and 16-27 are E-cores; an
unpinned run measured the same AVIF encode at 440 ms and at 1,500 ms. Pin the run
to P-cores. Child processes inherit the affinity, and `results.json` records it
under `env.scheduling`.

```powershell
# Windows (PowerShell): P-cores 0-15
$p = Start-Process node -ArgumentList baseline/run.mjs -NoNewWindow -PassThru
$p.ProcessorAffinity = 0xFFFF; $p.WaitForExit()
```

```sh
# Linux
taskset -c 0-15 node baseline/run.mjs
```

Other load on the machine still adds noise. Compare timings only between runs
made on the same machine under the same conditions.

### Parity with Squoosh's shipped binaries

`baseline/parity.mjs` encodes images through jSquash (exactly as `run.mjs` does)
and through the prebuilt single-threaded `.wasm` files committed in a Squoosh
clone (`codecs/*/enc/`, `codecs/oxipng/pkg/`), then compares the bytes:

```sh
node baseline/parity.mjs --squoosh /path/to/squoosh --only kodim01,kodim13,example
```

At Squoosh commit `e8d35e0f` all 15 outputs (3 images x 5 codecs) are
byte-identical, although the `.wasm` files themselves differ. So the file sizes
and quality scores in the baseline are what Squoosh itself produces with its
single-threaded builds.

### Corpus integrity

`corpus/fetch.mjs` downloads `https://r0k.us/graphics/kodak/kodak/kodimNN.png`
(NN = 01..24), rejects anything that is not a PNG, and copies
`<squoosh>/codecs/example.png`. On the first run it wrote the SHA-256 of all 25
files to `corpus/checksums.json` (trust on first use: there is no official
checksum list for the Kodak set). Later runs, and `run.mjs` itself, verify every
file against that list and stop on a mismatch. The committed `example.png`
checksum matches the file at Squoosh commit `e8d35e0f`.
`corpus/checksums.json` is meant to be committed. The images themselves
(`corpus/kodak/`) and all outputs (`out/`) are git-ignored and never redistributed.

## What is measured

For each of the 25 images and each codec (mozjpeg, webp, avif, jxl, oxipng):

| Metric | How |
|---|---|
| Encode time | `performance.now()` wall clock around the jSquash `encode()` call, 3 runs, median. Single process, sequential. The source RGBA is decoded once (with `@jsquash/png`) and cloned before each run; the clone is not timed. The first run of the first image includes lazy WASM compilation; the median drops it. |
| Output size | Bytes, and bpp (output bits per pixel). Also `ratio = source PNG file bytes / output bytes`, which depends on how well each source PNG happened to be compressed, so bpp is the better number to compare. |
| Quality (lossy codecs) | Output decoded back to RGBA with the matching jSquash decoder, written as PNG to `out/<codec>/<image>.decoded.png` with `@jsquash/png`, then `ssimulacra2_rs image <original.png> <decoded.png>` (reference first; the metric is not symmetric). |
| Quality (oxipng) | Output decoded with `@jsquash/png` and compared with the source RGBA (`pixelIdentical`). Where alpha is 0 only alpha is compared, because oxipng's `optimize_alpha` (on in Squoosh) may rewrite the colour of fully transparent pixels. |
| Batch, all codecs | A fresh child process (`run.mjs --batch`) pushes the whole corpus through all five codecs sequentially, once each: compile/init the WASM, read and decode each PNG, encode, write to `out/batch/`. It reports wall time, peak RSS sampled from `process.memoryUsage().rss` (every 5 ms and after every encode), and the OS peak from `process.resourceUsage().maxRSS`. WASM memory never shrinks, so this peak is roughly the sum of every codec's high-water mark. |
| Batch, one codec per process | The same batch once per codec, each in its own child process (`run.mjs --batch --codec <name>`), so each peak RSS belongs to one codec. It includes Node, the corpus buffers and the PNG codec, shown as "RSS after init". |

Per-codec summary rows are medians across images. The 24 Kodak photos
dominate the medians, so `RESULTS.md` also lists `example.png` on its own.

### Checks behind the method

- **Options.** Every value in `SQUOOSH_DEFAULTS` was compared with
  `squoosh/src/features/encoders/*/shared/meta.ts`, and the enum values with
  `codecs/*/enc/*.d.ts` (`MozJpegColorSpace.YCbCr = 3`, `AVIFTune.auto = 0`).
- **Encoders.** Byte-identical to Squoosh's shipped binaries (see parity above).
- **Decode path.** For kodim01, kodim13 and example, the jSquash-decoded JPEG and
  WebP pixels are identical to Pillow's (libjpeg-turbo, libwebp). AVIF differs
  from Pillow's libavif by at most 2 levels (YUV to RGB rounding), and JXL from
  ffmpeg's libjxl by at most 7 levels (mean 0.25). SSIMULACRA2 on the ffmpeg-decoded
  JXL differs by at most 0.11. The PNG written for the metric is lossless.

### Caveats

- **Threading.** Everything runs single-threaded. In Node, jSquash selects the
  single-threaded builds of avif, jxl and oxipng. The Squoosh web app uses the
  multi-threaded builds when the browser allows it (for jxl, the multi-threaded
  SIMD build), so browser-Squoosh can have lower wall-clock times for those three
  codecs. Those builds were not tested here; their output is expected to match,
  but that is not verified.
- **SIMD.** webp uses the `webp_enc_simd` build, which is what both Squoosh and
  jSquash pick when WASM SIMD is available.
- **Alpha.** `ssimulacra2_rs` loads images with `to_rgb32f()`, so it scores RGB
  only and ignores alpha. No image in this corpus uses transparency.
  `example.png` is stored as RGBA, but every pixel is opaque (`sourceHasAlpha: false`
  in `results.json`). Any future corpus with transparency needs a different
  comparison.
- **Colour.** `example.png` embeds a macOS "Display" ICC profile with roughly
  Display P3 primaries. `@jsquash/png` and `ssimulacra2_rs` both ignore it and
  treat the pixels as sRGB, so the comparison is consistent. A browser running
  Squoosh would convert the image to sRGB first, so real Squoosh would encode
  slightly different pixels for this one image. The Kodak PNGs are tagged sRGB.
- **Corpus.** 24 of the 25 images are 768x512 scans of film photos, and one is
  a screenshot. Screenshots, graphics, line art, alpha and high-resolution
  photos are barely or not at all represented, so do not generalise these
  numbers to them.
- **RSS sampling.** The sampled RSS can miss spikes inside a synchronous WASM
  call because the event loop is blocked. The `maxRSS` figure has no such gap.
  On Windows it is the peak working set, which the OS can trim under memory
  pressure.

## Node setup details

jSquash's default `init()` tries to `fetch()` its `.wasm` from a `file://` URL,
which Node's `fetch` does not support ("not implemented... yet"). `run.mjs`
therefore compiles each `.wasm` from `node_modules` with `WebAssembly.compile`
and passes the `WebAssembly.Module` to each package's `init()`. The `.wasm`
variant has to match the JS glue that jSquash picks in Node:

| Package | Encoder wasm | Decoder wasm |
|---|---|---|
| `@jsquash/jpeg` | `codec/enc/mozjpeg_enc.wasm` | `codec/dec/mozjpeg_dec.wasm` |
| `@jsquash/webp` | `codec/enc/webp_enc_simd.wasm` | `codec/dec/webp_dec.wasm` |
| `@jsquash/avif` | `codec/enc/avif_enc.wasm` (single-threaded) | `codec/dec/avif_dec.wasm` |
| `@jsquash/jxl` | `codec/enc/jxl_enc.wasm` (single-threaded) | `codec/dec/jxl_dec.wasm` |
| `@jsquash/oxipng` | `codec/pkg/squoosh_oxipng_bg.wasm` (single-threaded) | via `@jsquash/png` |
| `@jsquash/png` | `codec/pkg/squoosh_png_bg.wasm` (shared by encode and decode) | |

Node also has no `ImageData` global, and the jSquash decoders and
`@jsquash/oxipng` need one, so `run.mjs` installs a minimal polyfill.

## Squoosh defaults mapped to jSquash options

The source of truth is `squoosh/src/features/encoders/<codec>/shared/meta.ts`
(`defaultOptions`). jSquash uses **the same option names** as Squoosh, because
both pass the object straight to the same embind structs. So the mapping is
one-to-one, and `run.mjs` passes every field explicitly rather than relying on
jSquash's defaults.

| Codec | Squoosh `defaultOptions` (meta.ts) | jSquash option | Notes |
|---|---|---|---|
| mozjpeg | `quality: 75`, `baseline: false`, `arithmetic: false`, `progressive: true`, `optimize_coding: true`, `smoothing: 0`, `color_space: YCbCr (3)`, `quant_table: 3`, `trellis_multipass: false`, `trellis_opt_zero: false`, `trellis_opt_table: false`, `trellis_loops: 1`, `auto_subsample: true`, `chroma_subsample: 2`, `separate_chroma_quality: false`, `chroma_quality: 75` | identical names and values | `@jsquash/jpeg` defaults are identical. |
| webp | `quality: 75`, `target_size: 0`, `target_PSNR: 0`, `method: 4`, `sns_strength: 50`, `filter_strength: 60`, `filter_sharpness: 0`, `filter_type: 1`, `partitions: 0`, `segments: 4`, `pass: 1`, `show_compressed: 0`, `preprocessing: 0`, `autofilter: 0`, `partition_limit: 0`, `alpha_compression: 1`, `alpha_filtering: 1`, `alpha_quality: 100`, `lossless: 0`, `exact: 0`, `image_hint: 0`, `emulate_jpeg_size: 0`, `thread_level: 0`, `low_memory: 0`, `near_lossless: 100`, `use_delta_palette: 0`, `use_sharp_yuv: 0` | identical names and values | `@jsquash/webp` defaults are identical. |
| avif | `quality: 50`, `qualityAlpha: -1`, `denoiseLevel: 0`, `tileColsLog2: 0`, `tileRowsLog2: 0`, `speed: 6`, `subsample: 1` (4:2:0), `chromaDeltaQ: false`, `sharpness: 0`, `tune: auto (0)`, `enableSharpYUV: false` | identical names and values, plus `bitDepth: 8`, `lossless: false` | `bitDepth` and `lossless` are jSquash-only. Their values reproduce Squoosh, which always encodes 8-bit and is lossless only at quality 100 with 4:4:4. Quality uses libavif's 0 to 100 scale in both. |
| jxl | `effort: 7`, `quality: 75`, `progressive: false`, `epf: -1`, `lossyPalette: false`, `decodingSpeedTier: 0`, `photonNoiseIso: 0`, `lossyModular: false` | identical names and values, plus `lossless: false` | `lossless` is jSquash-only. |
| oxipng | `level: 2`, `interlace: false` | `level: 2`, `interlace: false`, `optimiseAlpha: true` | Squoosh has no alpha option: its `codecs/oxipng/src/lib.rs` hard-codes `optimize_alpha = true`, so the jSquash switch is set to match. Squoosh optimises the raw RGBA pixels (`RawImage`), not the source file; `run.mjs` passes an `ImageData`, so jSquash takes the same raw path (`optimise_raw`). This matters once a corpus has transparency: on a synthetic 256x256 image whose transparent half held random colours, `optimiseAlpha: false` gave 25,971 bytes and Squoosh gave 774 (`true` matches Squoosh byte for byte). |

Squoosh's UI starts every image with these values ("Squoosh defaults"), and the
baseline uses exactly these.

## Layout

```
bench/
  package.json            pinned jSquash versions
  corpus/fetch.mjs        corpus download + SHA-256 verification
  corpus/checksums.json   SHA-256 of all 25 corpus files
  corpus/kodak/           images (git-ignored)
  baseline/run.mjs        the benchmark
  baseline/parity.mjs     byte-for-byte check against Squoosh's shipped wasm
  baseline/results.json   raw per-image data + environment
  baseline/RESULTS.md     summary tables
  out/                    encoded + decoded outputs, partial runs (git-ignored)
```
