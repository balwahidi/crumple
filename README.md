# Crumple

> **Status: pre-alpha.** The `crumple` command-line tool works on PNG, JPEG and WebP
> files, but there is no release or installer yet, and its flags and report format may
> still change. The desktop app and the PWA do not exist yet.

Crumple is an open-source, local-first batch image optimizer.

## Vision

- Drop in a folder of images and set a quality target, for example
  "visually lossless" or "SSIMULACRA2 ≥ 80".
- Crumple picks the codec and settings for each image to meet that target.
- Everything runs on your machine. Crumple never uploads your images.

One Rust core, shared by three front ends:

- `crumple`, a command-line tool (works today)
- a desktop app (Tauri, planned)
- a PWA (the core compiled to WebAssembly, planned)

## What the CLI does

For each image, Crumple:

1. decodes it, applies its EXIF orientation and, if asked, downscales it to fit
   `--max-width`/`--max-height`;
2. searches JPEG (mozjpeg), WebP (libwebp) and AVIF (rav1e) for the lowest quality whose
   [SSIMULACRA2](https://github.com/cloudinary/ssimulacra2) score against those pixels meets
   the target (usually 1 to 6 encodes per codec). PNG inputs also get a lossless PNG (oxipng);
3. writes the smallest candidate, or copies the original if no candidate is smaller. The
   bytes written are exactly the bytes that were scored.

Limits you should know about:

- **Inputs:** PNG, JPEG and WebP. AVIF input, animated images, CMYK JPEGs and images above
  24 MP (`--max-pixels`) are skipped and reported, not converted.
- **Metadata:** EXIF orientation is applied to the pixels. An ICC profile is carried into
  JPEG, WebP and PNG output, but an image with any ICC profile (even plain sRGB) gets no AVIF
  candidate yet. All other metadata (EXIF, XMP, GPS) is removed.
- **Colour:** the metric treats pixel values as sRGB, which is an approximation for
  wide-gamut images.
- **Speed:** the AVIF search dominates. Expect a few CPU-seconds per 0.4 MP photo, and far
  more for large images.
- **Memory:** `--max-memory` (default 1 GiB) limits how many images run at once. It is not a
  hard cap: an image too big for the budget runs alone and can exceed it (a 12 MP image with
  transparency peaked at 2.2 GB).

## Usage

```sh
# Optimize a folder (recursively) into another folder, with a per-image JSONL report.
crumple optimize photos/ --out optimized/ --target 80 --report report.jsonl

# Presets: medium = 70, high = 80, excellent = 85, lossless-looking = 90.
crumple optimize a.png b.jpg --out web/ --target high --formats webp,avif --max-width 1600

# Score any image against a reference (PNG, JPEG, WebP or AVIF; 100 = identical).
crumple score original.png optimized/original.avif
```

`crumple optimize --help` lists every flag: `--formats` (default `jpeg,webp,avif,png`, or
`same` to keep each file's format family), `--jobs`, `--max-memory`, `--max-pixels`,
`--overwrite` and `--dry-run`. Existing outputs are never replaced without `--overwrite`,
and inputs are never modified. The exit code is 1 if any image failed.

`crumple score` decodes AVIF with rav1d, which can abort the process on a corrupt AVIF file
([memorysafety/rav1d#1497](https://github.com/memorysafety/rav1d/issues/1497)). That ends
only that one command; `optimize` never decodes AVIF input for this reason.

## Results

Measured with [`bench/crumple`](bench/crumple/RESULTS.md) on 25 images, on one machine
(i7-14700KF, 16 threads pinned to the performance cores, default 1 GiB memory budget).
Scores are SSIMULACRA2.

| Target | Total output bytes | Images meeting the target | Min score | Median score | Wall time | Peak memory |
|---:|---:|---:|---:|---:|---:|---:|
| 70 | 1,138,078 | 25 of 25 | 70.00 | 70.45 | 22.8 s | 498 MiB |
| 80 | 1,765,791 | 25 of 25 | 80.03 | 80.63 | 22.9 s | 501 MiB |
| 90 | 3,890,160 | 25 of 25 | 90.18 | 90.65 | 17.1 s | 511 MiB |

The scores in this table were measured without Crumple's own code
([`bench/crumple/rescore.mjs`](bench/crumple/rescore.mjs)): every output was decoded with
Squoosh's decoders (jSquash) and scored with `ssimulacra2_rs`, the same path as the Squoosh
baseline below. Crumple's own scores differ by up to 0.22 because it decodes AVIF and JPEG
with other libraries: lowest 70.00, 80.00 and 90.11, medians 70.34, 80.60 and 90.54. The source files total
18,804,994 bytes. The codec chosen was AVIF for 23 or 24 of the 25 images at every target.

For comparison, Squoosh's default settings on the same images (a fixed quality per codec,
not a target) give:

| Squoosh default | Total bytes | Min score | Median score |
|---|---:|---:|---:|
| MozJPEG q75 | 1,413,218 | 67.7 | 72.0 |
| WebP q75 | 1,258,088 | 59.6 | 69.2 |
| AVIF q50 | 834,002 | 48.7 | 62.1 |
| JPEG XL q75 | 1,211,649 | 69.0 | 72.3 |

How to read this, and what it does not show:

- At target 70, Crumple's output is 19.5% smaller than Squoosh's default MozJPEG and every
  image scores at least 70, where MozJPEG's worst image scores 67.7. But Crumple's median
  score (70.5) is below MozJPEG's (72.0): Crumple lifts the worst images to the target and
  spends few bytes beyond it on the others. Squoosh's default AVIF is smaller still, at much
  lower quality (median 62.1).
- **The corpus is small and narrow**: 24 Kodak film scans at 768x512 or 512x768 (0.4 MP)
  plus one 2558x1348 screenshot. It has no phone photos, no high-resolution photos, no
  graphics or line art and no transparency. Do not generalise these numbers to other images;
  see the corpus notes in [`bench/README.md`](bench/README.md).
- SSIMULACRA2 is tuned on photographs, and a score target is not a guarantee of how every
  viewer will see every image.
- The wall times are medians of 3 runs and moved by 2 to 3 s between sessions on this
  machine (an earlier session measured 21.4, 20.1 and 15.9 s). They are not a speed
  comparison with Squoosh: Crumple used 16 threads and about 99 CPU-seconds at target 80,
  while the Squoosh baseline encodes each image once per codec, on one thread, without
  scoring (37.2 s for five codecs).
- Output is deterministic: the same files hash the same with `--jobs 1` and `--jobs 16`.

## Layout

- `crates/crumple-cli`: the `crumple` binary (discovery, scheduling, memory budget, report)
  and the per-image pipeline.
- `crates/crumple-core`: the quality search, the final choice, orientation and resizing.
  Pure Rust, no I/O; it builds for WebAssembly.
- `crates/crumple-codecs`: encoders and decoders (wraps C mozjpeg and libwebp, rav1e, rav1d,
  oxipng).
- `crates/crumple-metric`: SSIMULACRA2 with the reference computed once per image.
- `bench/`: the Squoosh baseline and the Crumple benchmark. `docs/ARCHITECTURE.md`: the design.

## Building

Prerequisites:

- A stable Rust toolchain and a C compiler (the codecs bundle C code: mozjpeg, libwebp, libdeflate).
- **nasm 2.14 or newer on `PATH`** on x86_64. The AVIF encoder and decoder (rav1e, rav1d) need it,
  and mozjpeg uses it for its SIMD code. Check with `nasm -v`. Without it, only the AVIF feature
  of `crumple-codecs` fails to build (`--no-default-features --features jpeg,webp,png` works).

```sh
cargo build --release -p crumple-cli
cargo test --workspace
./target/release/crumple --help
```

Third-party licenses for everything the binary is built from are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). It is generated: after changing
dependencies, run `node tools/gen-notices.mjs` (Node 24; `--check` only verifies).

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

## Credits

- [Squoosh](https://github.com/GoogleChromeLabs/squoosh) by Google Chrome Labs
  (Apache-2.0), unmaintained since August 2024. Crumple starts from its
  per-codec default settings and uses them as the benchmark baseline. Code
  ported from Squoosh keeps its original copyright header, and [NOTICE](NOTICE)
  carries the attribution. Crumple is not affiliated with or endorsed by Google.
- The benchmark in [`bench/`](bench/) measures the codec builds Squoosh shipped
  (MozJPEG, libwebp, libavif with libaom, libjxl and OxiPNG), run through
  [jSquash](https://github.com/jamsinclair/jSquash) (Apache-2.0). They are
  benchmark tools only and are not part of Crumple.
- The `crumple` binary is built on mozjpeg (IJG, libjpeg-turbo), libwebp, rav1e and
  [ravif](https://github.com/kornelski/cavif-rs), rav1d and avif-decode, oxipng and
  libdeflate, among others. Every one is listed, with its license text, in
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
- [SSIMULACRA2](https://github.com/cloudinary/ssimulacra2) by Jon Sneyers (Cloudinary), the
  perceptual metric behind the quality targets. `crumple-metric` ports code from the Rust
  [`ssimulacra2`](https://github.com/rust-av/ssimulacra2) crate (BSD-2-Clause), and the
  benchmark uses [`ssimulacra2_rs`](https://github.com/rust-av/ssimulacra2_bin) (BSD-2-Clause).
