# Rust codec layer survey

Status: research note. Data gathered on **2026-09-27**.

> **Reviewer verification (2026-09-27).** A reviewer re-checked this note against crates.io and GitHub and ran real end-to-end probes. Corrections are marked **[Reviewer]** in the text below. The main points:
>
> - **Every license claim checks out** against the crates.io API and the GitHub license files. A transitive scan (`cargo tree`) of the native recommended stack found no GPL, AGPL or LGPL crate.
> - **The `mozjpeg` no-nasm explanation was wrong** (see the probe table).
> - **`mozjpeg-rs` is younger than stated.** Its first release was on 2025-12-29, not in 2024. Its README carries an "AI-generated code, not all human-reviewed" notice. In Crumple's configuration its output is **not** byte-identical to C mozjpeg.
> - **`jpegli` compiles but does not link** on MSVC.
> - **`avif-decode` can be built without nasm** by patching one dependency.
> - **The native stack works end to end on Windows MSVC.** See [Native end-to-end probe](#native-end-to-end-probe-reviewer).
> - **The native JPEG pick changes** to C `mozjpeg`. `mozjpeg-rs` stays the WASM pick.

This note surveys the Rust crates that could form Crumple's codec layer. Crumple has one Rust core, built natively (CLI and Tauri on Windows, macOS and Linux) and for `wasm32-unknown-unknown` (the browser PWA). Crumple is Apache-2.0, so every dependency it ships must be compatible with distributing an Apache-2.0 app.

## Method

- **Version, release date and license** come from the crates.io API (`https://crates.io/api/v1/crates/NAME`, sent with a User-Agent header). The version is the highest stable version, and the license is the one declared for that version.
- **Last commit** is the committer date of the HEAD commit on the repo's default branch, taken from `gh api repos/OWNER/REPO/commits/BRANCH`.
- **Build probes.** For each probed crate I made a one-dependency scratch crate (empty `lib.rs`) outside the repo and ran:
  - `cargo check --target x86_64-pc-windows-msvc` for the native build, and
  - `cargo check --target wasm32-unknown-unknown` for the WASM build.

  The toolchain was rustc 1.98.1, with VS 2022 Build Tools and CMake present. Neither `nasm` nor `pkg-config` was on PATH. LLVM was installed at `C:\Program Files\LLVM` but was **not** on PATH.

  `cargo check` runs build scripts, so bundled C code is really compiled. It does **not** link, so a PASS proves compilation only, not a working wasm module.
- **"UNVERIFIED"** marks a claim I did not check directly, whether inferred or taken from a README.
- The **WASM column** shows my own probe result where one exists, marked "probe". Otherwise it is an inference, marked UNVERIFIED.

## Probe results (raw)

| Probe (dependency spec) | Native (Windows MSVC) | wasm32-unknown-unknown | First error / note |
|---|---|---|---|
| `mozjpeg = "0.10.13"` | PASS | FAIL | wasm: `error: failed to run custom build command for mozjpeg-sys v2.2.3`. Cause: `failed to find tool "clang"`. When given LLVM clang, it fails with `fatal error: 'stdlib.h' file not found` because there is no libc sysroot. ~~Native PASS works without nasm because `mozjpeg` turns off `mozjpeg-sys` default features, so there is **no SIMD**.~~ **[Reviewer] Wrong reason.** `mozjpeg`'s own `default` feature re-enables `mozjpeg-sys/default`, which includes `nasm_simd`. The build passes because `mozjpeg-sys`'s `build.rs` runs `nasm -v`. When that fails it prints `cargo:warning=NASM not installed. Mozjpeg's SIMD won't be enabled` and quietly compiles `jsimd_none.c` instead. So the same build is SIMD on a machine with nasm and non-SIMD without it. Only the speed differs, not the output. Linked and run end to end: see the native probe. |
| `mozjpeg-rs = "0.9.2"` | PASS | PASS | |
| `jpegli = "0.1.0"` | FAIL | FAIL | `error: failed to run custom build command for jpegli-sys v0.1.0+0.10.2`. Natively this was a CMake/MSBuild **MAX_PATH (260 chars) error caused by the long scratch path**, which is an environment issue and **not conclusive**. **[Reviewer] Re-run with a short `CARGO_TARGET_DIR`: it compiles (34 s) but does not link.** The error is `LINK : fatal error LNK1181: cannot open input file 'jpegli-static.lib'`. `jpegli-sys`'s `build.rs` adds `out/build/lib` to the search path, but the MSVC multi-config CMake generator writes to `out/build/lib/Release/`. Adding `-L native=…/lib/Release -L native=…/third_party/highway/Release` makes it link and run. On kodim01 it is no better than mozjpeg: q80 gives 98,432 B at SSIMULACRA2 77.41, while mozjpeg q85 gives 100,040 B at 78.52. It is also ~15x slower (~700 ms against ~45 ms), for reasons not investigated. Verdict: not usable as is. |
| `jpeg-encoder = "0.7"` | PASS | PASS | |
| `zune-jpeg = "0.5.15"` | PASS | PASS | |
| `png = "0.18.1"` | PASS | PASS | |
| `oxipng = { 10.2.1, default-features = false }` | PASS | FAIL | `error: failed to run custom build command for libdeflate-sys v1.26.1`. Cause: no clang. With LLVM clang it fails on `'string.h' file not found`. |
| `oxipng = { 10.2.1, default-features = false, features = ["freestanding"] }` with `CC_wasm32_unknown_unknown` set to LLVM clang | n/a | **PASS** | Without clang it still fails: `libdeflate-sys` needs clang. |
| `quantette = "0.6.0"` | PASS | PASS | |
| `imagequant = "4.4.1"` | PASS | PASS | Builds, but the **license is GPL-3.0** (see below). |
| `webp = { 0.3.1, default-features = false }` | PASS | FAIL | `error: failed to run custom build command for libwebp-sys v0.9.6`. With clang it fails on `'assert.h' file not found`. Note that `webp` 0.3.1 pins `libwebp-sys ^0.9.3`, while the latest is 0.14.4. |
| `image-webp = "0.2.4"` | PASS | PASS | |
| `ravif = { 0.13.0, default-features = false }` | PASS | PASS | No `asm` and no `threading`. **[Reviewer]** Linked and run both natively and as wasm32-unknown-unknown in Node. Three findings. (a) The output **depends on the thread count**: q50 at speed 6 gave 37,821 B with the rayon default and 36,604 B with `RAYON_NUM_THREADS=1`. The single-threaded native and wasm outputs are the same size. (b) The default output is 10-bit, so `avif-decode` returns `Image::Rgb16`. (c) Without asm it is slow; see the native probe. |
| `avif-decode = { 3.0.0, default-features = false }` | FAIL | FAIL | Native: `failed to run custom build command for rav1d v1.1.0`, because nasm was not found. `avif-decode` does not let you turn off rav1d's `asm`. wasm: `error[E0432]: unresolved import libc::ptrdiff_t` (39 errors). **[Reviewer]** The asm dependency is target-specific. Only `cfg(any(target_arch = "x86", target_arch = "x86_64"))` pulls `rav1d` with default features (asm); other targets get `default-features = false`. A vendored copy with the x86 entry changed to `default-features = false, features = ["bitdepth_8","bitdepth_16"]`, applied through `[patch.crates-io]`, **builds, links and decodes correctly on Windows MSVC without nasm**. Decoding a 768x512 image takes 16 ms single-threaded. Also, `Decoder::from_reader` always starts rav1d with `available_parallelism()` threads (capped at 32), and there is no API to change that. |
| `rav1d = { 1.1.0, default-features = false, features = ["bitdepth_8"] }` | PASS | FAIL | wasm: `error[E0432]: unresolved import libc::ptrdiff_t`. rav1d does not compile for wasm32-unknown-unknown. |
| `dav1d = "0.11.1"` | FAIL | FAIL | `failed to run custom build command for dav1d-sys v0.8.3`: pkg-config not found. It needs a system libdav1d. |
| `libavif = "0.14.0"` | FAIL | FAIL | `failed to run custom build command for libdav1d-sys v0.7.1+libdav1d.1.4.3`: meson and ninja are required. |
| `jpegxl-rs = "0.15.0"` | FAIL | FAIL | `failed to run custom build command for jpegxl-sys v0.13.0+libjxl-0.12.0`: it looks for a system libjxl via pkg-config (the `vendored` feature was not tried). The crate is GPL-3.0 anyway. |
| `jxl-oxide = "0.12.6"` | PASS | PASS | |
| `jxl = "0.7.4"` (libjxl/jxl-rs) | PASS | PASS | |
| `zune-jpegxl = "0.5.2"` | PASS | PASS | |
| `fast_image_resize = "6.1.0"` | PASS | PASS | |
| `ssimulacra2 = { 0.5.1, default-features = false }` | PASS | PASS | |
| `butteraugli = { 0.9.3, default-features = false }` | PASS | PASS | |
| `dssim-core = "3.5.1"` | PASS | PASS | Builds, but the **license is AGPL-3.0**. |
| `image = { 0.25.10, default-features = false, features = ["jpeg","png","webp","avif","gif"] }` | PASS | PASS | |
| `zune-image = { 0.5.0, default-features = false, features = ["image_formats"] }` | PASS | PASS | |

## JPEG encode

| Crate | Latest (date) | License | Apache-2.0 app OK? | Pure Rust / binds | Last commit | wasm32-unknown-unknown |
|---|---|---|---|---|---|---|
| [`mozjpeg`](https://crates.io/crates/mozjpeg) ([repo](https://github.com/ImageOptim/mozjpeg-rust)) | 0.10.13 (2025-02-18) | IJG | Yes, with the IJG attribution notice | Binds C mozjpeg (libjpeg-turbo fork) through `mozjpeg-sys` | 2026-02-04 | FAIL (probe): needs a C toolchain and a libc sysroot |
| [`mozjpeg-sys`](https://crates.io/crates/mozjpeg-sys) ([repo](https://github.com/kornelski/mozjpeg-sys)) | 2.2.3 (2025-01-21) | IJG AND Zlib AND BSD-3-Clause | Yes, with notices | C mozjpeg, optional nasm SIMD (default `nasm_simd`) | 2026-05-31 | FAIL (via the `mozjpeg` probe) |
| [`mozjpeg-rs`](https://crates.io/crates/mozjpeg-rs) ([repo](https://github.com/imazen/mozjpeg-rs)) | 0.9.2 (2026-04-17) | BSD-3-Clause (the LICENSE also carries the IJG "based in part" notice) | Yes | **Pure Rust** port of mozjpeg (`forbid(unsafe_code)`). Encoder only. ~~The README claims byte-identical output to C mozjpeg (UNVERIFIED).~~ **[Reviewer] Measured on the 25-image corpus** at the mozjpeg max-compression settings Squoosh uses (progressive, trellis, optimize_scans, 4:2:0): **0 of 25 files are byte-identical.** Total bytes are −0.52% / +0.03% / +0.20% against C at q60 / q75 / q90, and individual images range from −1.5% to +0.9%. Mean SSIMULACRA2 is −0.22 / −0.14 / −0.02 lower. Encoding is **~1.5x slower than C mozjpeg even without C SIMD**, and 2.1x slower on the 3.4 MP `example.png`. The README only claims byte parity for its non-trellis modes. **Maturity:** first release 2025-12-29, 17 releases in 4 months, 3 GitHub stars, and a README "AI-Generated Code Notice" saying not all code has been human-reviewed. The default feature `fast-yuv` pulls in `zenyuv`, whose `repository` is the AGPL `imazen/zenjpeg` repo. The crate itself declares `MIT OR Apache-2.0` and ships both license files (checked in the downloaded package). | 2026-08-29 | **PASS** (probe); **[Reviewer]** also linked and run in Node: output size is identical to native (73,093 B at q75), and encoding takes 78 ms against 51 ms native |
| [`jpeg-encoder`](https://crates.io/crates/jpeg-encoder) ([repo](https://github.com/vstroebel/jpeg-encoder)) | 0.7.1 (2026-07-27) | (MIT OR Apache-2.0) AND IJG | Yes, with the IJG notice | Pure Rust, baseline and progressive; no trellis (UNVERIFIED) | 2026-07-27 | PASS (probe) |
| [`jpegli`](https://crates.io/crates/jpegli) ([repo](https://github.com/Szpadel/jpegli-rs)) | 0.1.0 (2024-04-09) | BSD-3-Clause | Yes | Binds C++ jpegli from libjxl 0.10.2 (`jpegli-sys`, CMake build) | 2024-04-09 (**stale**) | FAIL (probe; the native failure was a MAX_PATH issue, so it is inconclusive) |
| [`jpegli-rs`](https://crates.io/crates/jpegli-rs) ([repo](https://github.com/imazen/jpegli-rs)) | 0.12.0 (2026-01-24) | **AGPL-3.0-or-later** | **No** | Pure-Rust port of jpegli. **[Reviewer]** The GitHub repo `imazen/jpegli-rs` now redirects to `imazen/zenjpeg`: it was renamed, and `zenjpeg` is its successor. | 2026-09-27 (the zenjpeg repo) | not probed |
| [`zenjpeg`](https://crates.io/crates/zenjpeg) ([repo](https://github.com/imazen/zenjpeg)) | 0.8.4 (2026-06-01) | **AGPL-3.0-only OR commercial** | **No** (unless a commercial license is bought) | Pure Rust | 2026-09-27 | not probed |

The `mozjpeg-rs` README recommends `zenjpeg` "for new projects". That crate is AGPL, so do not follow that suggestion.

## JPEG decode

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`zune-jpeg`](https://crates.io/crates/zune-jpeg) ([repo](https://github.com/etemesi254/zune-image)) | 0.5.15 (2026-03-26); 0.5.16-rc2 prerelease on 2026-09-08 | MIT OR Apache-2.0 OR Zlib | Yes | Pure Rust | 2026-09-24 (monorepo `dev`) | PASS (probe) |
| [`jpeg-decoder`](https://crates.io/crates/jpeg-decoder) ([repo](https://github.com/image-rs/jpeg-decoder)) | 0.3.2 (2025-06-21) | MIT OR Apache-2.0 | Yes | Pure Rust | 2025-06-21 (quiet; `image` now uses zune-jpeg) | UNVERIFIED (pure Rust, likely PASS) |

## PNG

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`png`](https://crates.io/crates/png) ([repo](https://github.com/image-rs/image-png)) | 0.18.1 (2026-02-14) | MIT OR Apache-2.0 | Yes | Pure Rust | 2026-09-23 | PASS (probe) |
| [`oxipng`](https://crates.io/crates/oxipng) ([repo](https://github.com/oxipng/oxipng)) | 10.2.1 (2026-09-02) | MIT | Yes | Rust, but depends on `libdeflater` → **C libdeflate**. Optional `zopfli` is pure Rust. | 2026-09-02 | FAIL by default. **PASS** with `features = ["freestanding"]` and LLVM clang as `CC_wasm32_unknown_unknown` (probe; compile only, not linked) |
| [`zune-png`](https://crates.io/crates/zune-png) ([repo](https://github.com/etemesi254/zune-image)) | 0.5.2 (2026-03-11) | MIT OR Apache-2.0 OR Zlib | Yes | Pure Rust (fast decoder) | 2026-09-24 | PASS via `zune-image` (probe) |

## Palette quantization

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`imagequant`](https://crates.io/crates/imagequant) ([repo](https://github.com/ImageOptim/libimagequant)) | 4.4.1 (2025-07-13) | **GPL-3.0-or-later** (or a paid commercial license) | **No** | Pure Rust since v4 (it is libimagequant itself) | 2026-06-21 | PASS (probe) |
| [`color_quant`](https://crates.io/crates/color_quant) ([repo](https://github.com/image-rs/color_quant)) | 2.0.0 (2026-05-09) | MIT | Yes | Pure Rust (NeuQuant) | 2026-05-09 | UNVERIFIED (pure Rust; `image` with `gif` pulls it in and PASSed) |
| [`quantette`](https://crates.io/crates/quantette) ([repo](https://github.com/IanManske/quantette)) | 0.6.0 (2026-05-15) | MIT OR Apache-2.0 | Yes | Pure Rust (Wu and k-means, optional dithering; UNVERIFIED feature list) | 2026-08-02 | PASS (probe) |
| [`exoquant`](https://crates.io/crates/exoquant) ([repo](https://github.com/exoticorn/exoquant-rs)) | 0.2.0 (2016-09-04) | MIT | Yes | Pure Rust | 2016-09-04 (**abandoned**) | not probed |
| [`zenquant`](https://crates.io/crates/zenquant) ([repo](https://github.com/imazen/zenquant)) | 0.1.3 (2026-04-10) | **AGPL-3.0-only OR commercial** | **No** | Pure Rust | not checked | not probed |

The `imagequant` license history on crates.io: BSD-2-Clause up to 2.3.x, GPL-3.0 from 2.7.0 (2016), and GPL-3.0-or-later from 4.0.0. The README states it is dual-licensed, GPLv3+ or commercial.

## WebP

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`webp`](https://crates.io/crates/webp) ([repo](https://github.com/jaredforth/webp)) | 0.3.1 (2025-08-29) | MIT OR Apache-2.0 | Yes | Wraps `libwebp-sys ^0.9.3` (an **old** libwebp-sys) | 2025-08-29 | FAIL (probe): no clang; with clang, no libc headers |
| [`libwebp-sys`](https://crates.io/crates/libwebp-sys) ([repo](https://github.com/NoXF/libwebp-sys)) | 0.14.4 (2026-04-29) | MIT (bundled libwebp is BSD-3-Clause plus a patent grant) | Yes | Vendored C libwebp built with `cc` | 2026-04-30 | FAIL (0.9.6 probed through `webp`; 0.14 not probed). **[Reviewer]** 0.14.4 builds and links natively on MSVC. `WebPEncodeRGB` at q75 gives **77,422 B for kodim01, the same size as the jSquash/Squoosh baseline output**, with the same SSIMULACRA2 of 72.72. Across the corpus the median is 46,574 B at 69.18, also identical to the baseline. |
| [`image-webp`](https://crates.io/crates/image-webp) ([repo](https://github.com/image-rs/image-webp)) | 0.2.4 (2025-08-27) | MIT OR Apache-2.0 | Yes | Pure Rust. Full decoder (lossy, lossless, alpha, animation); **encoder is lossless only** (per README) | 2026-04-08 | PASS (probe) |
| [`zenwebp`](https://crates.io/crates/zenwebp) ([repo](https://github.com/imazen/zenwebp)) | 0.4.4 (2026-04-17) | **AGPL-3.0-only OR commercial** | **No** | Pure Rust (lossy encoder, UNVERIFIED) | not checked | not probed |

## AVIF encode

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`ravif`](https://crates.io/crates/ravif) ([repo](https://github.com/kornelski/cavif-rs)) | 0.13.0 (2026-01-19) | BSD-3-Clause | Yes | Rust over `rav1e`. The default `asm` feature needs nasm (rav1e `asm` = nasm-rs + cc). | 2026-09-05 | PASS with `default-features = false` (probe) |
| [`rav1e`](https://crates.io/crates/rav1e) ([repo](https://github.com/xiph/rav1e/)) | 0.8.1 (2025-06-16) | BSD-2-Clause (plus the AOM patent license, UNVERIFIED) | Yes | Rust, with optional nasm/C asm. Has a `wasm` feature. | 2026-08-31 | PASS via `ravif` (probe) |
| [`libavif`](https://crates.io/crates/libavif) / [`libavif-sys`](https://crates.io/crates/libavif-sys) ([repo](https://github.com/njaard/libavif-rs)) | 0.14.0 / 0.17.0+libavif.1.0.4 (2024-07-05) | BSD-2-Clause | Yes | Binds C libavif plus libdav1d/libaom/rav1e; build needs meson and ninja | 2025-03-16 (bundles old libavif 1.0.4) | FAIL (probe) |

## AVIF decode

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`avif-decode`](https://crates.io/crates/avif-decode) ([repo](https://github.com/kornelski/avif-decode)) | 3.0.0 (2026-09-20) | BSD-3-Clause (its dependency `avif-parse` is MPL-2.0) | Yes | Rust over **`rav1d`** (Rust port of dav1d) with nasm asm always on **[Reviewer] on x86/x86_64 only**; other architectures build without asm | 2026-09-20 | FAIL (probe). Native fails without nasm; wasm has `libc::ptrdiff_t` errors. **[Reviewer]** Native PASS with a one-line `[patch]` (see the probe table) |
| [`rav1d`](https://crates.io/crates/rav1d) ([repo](https://github.com/memorysafety/rav1d)) | 1.1.0 (2025-05-07) | BSD-2-Clause | Yes | Rust port of dav1d, with optional nasm asm | 2026-08-14 | FAIL even with no asm (probe) |
| [`dav1d`](https://crates.io/crates/dav1d) / [`dav1d-sys`](https://crates.io/crates/dav1d-sys) ([repo](https://github.com/rust-av/dav1d-rs)) | 0.11.1 (2025-11-25) / 0.8.3 | MIT (libdav1d is BSD-2-Clause) | Yes | Binds a **system** libdav1d via pkg-config | 2025-11-25 | FAIL (probe; no pkg-config/libdav1d) |

`image` offers the `avif-native` feature (mp4parse plus dav1d) for AVIF decode. Its plain `avif` feature only **encodes**, through ravif.

## JPEG XL

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Enc / Dec | Last commit | wasm32 |
|---|---|---|---|---|---|---|---|
| [`jpegxl-rs`](https://crates.io/crates/jpegxl-rs) / [`jpegxl-sys`](https://crates.io/crates/jpegxl-sys) ([repo](https://github.com/inflation/jpegxl-rs)) | 0.15.0+libjxl-0.12.0 / 0.13.0 (2026-07-03) | **GPL-3.0-or-later** (every version since 0.1.0) | **No** | Binds C++ libjxl (system via pkg-config, or the `vendored` feature) | both | 2026-09-22 | FAIL (probe) |
| [`jxl-oxide`](https://crates.io/crates/jxl-oxide) ([repo](https://github.com/tirr-c/jxl-oxide)) | 0.12.6 (2026-05-29) | MIT OR Apache-2.0 | Yes | Pure Rust | decode only | 2026-08-16 | PASS (probe) |
| [`jxl`](https://crates.io/crates/jxl) ([repo](https://github.com/libjxl/jxl-rs), official libjxl org) | 0.7.4 (2026-09-17) | BSD-3-Clause | Yes | Pure Rust | decode only | 2026-09-27 | PASS (probe) |
| [`zune-jpegxl`](https://crates.io/crates/zune-jpegxl) ([repo](https://github.com/etemesi254/zune-image)) | 0.5.2 (2026-01-24) | MIT OR Apache-2.0 OR Zlib | Yes | Pure Rust | **lossless encode only** (per README) | 2026-09-24 | PASS (probe) |
| [`jxl-encoder`](https://crates.io/crates/jxl-encoder) ([repo](https://github.com/imazen/jxl-encoder)) | 0.3.1 (2026-05-02) | **AGPL-3.0-only OR commercial** | **No** | Pure Rust, **lossy (VarDCT) and lossless** | encode | 2026-09-27 | not probed |
| [`zenjxl`](https://crates.io/crates/zenjxl) ([repo](https://github.com/imazen/zenjxl)) | 0.2.1 (2026-05-02) | **AGPL-3.0-only OR commercial** | **No** | Pure Rust | both | not checked | not probed |
| [`gamut-jxl`](https://crates.io/crates/gamut-jxl) / [`gamut-jxl-sys`](https://crates.io/crates/gamut-jxl-sys) ([repo](https://github.com/justin13888/gamut)) | 0.4.0 (2026-07-21) / 0.1.0 | MIT OR Apache-2.0 | Yes | Rust decoder. The optional `encode` feature statically builds C++ libjxl (CMake), per README | both | pushed 2026-09-25 (6 stars, very new) | not probed |

The search found **no permissively licensed, pure-Rust, lossy JPEG XL encoder**. The only pure-Rust lossy encoder found is `jxl-encoder`, which is AGPL.

## Resize

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`fast_image_resize`](https://crates.io/crates/fast_image_resize) ([repo](https://github.com/cykooz/fast_image_resize)) | 6.1.0 (2026-07-21) | MIT OR Apache-2.0 | Yes | Pure Rust (SIMD; has a wasm32 simd128 path, UNVERIFIED) | 2026-08-20 | PASS (probe) |
| [`resize`](https://crates.io/crates/resize) ([repo](https://github.com/PistonDevelopers/resize)) | 0.8.9 (2026-02-07) | MIT | Yes | Pure Rust | 2026-02-07 | UNVERIFIED (pure Rust, likely PASS) |

## Metrics

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`ssimulacra2`](https://crates.io/crates/ssimulacra2) ([repo](https://github.com/rust-av/ssimulacra2)) | 0.5.1 (2024-12-29) | BSD-2-Clause (its dependency `yuvxyb` is MIT) | Yes | Pure Rust. **[Reviewer] Cost measured on a P-core, single-threaded.** A 768x512 image takes 107 ms and a 3.45 MP image 1.04 s, so one score costs **~2x a mozjpeg encode and ~4x a libwebp encode**. The `rayon` feature barely helps (~100 ms). **Memory is ~145 B per pixel** at peak: a 3.45 MP score peaks at 505 MiB, so a 24 MP photo would need ~3.5 GB. The API converts and blurs the reference again on every call; nothing is cached. In wasm32 (Node, no simd128) it takes 197 ms, 1.8x native. | 2026-07-05 | PASS (probe); **[Reviewer]** linked and run natively and in wasm |
| [`dssim`](https://crates.io/crates/dssim) / [`dssim-core`](https://crates.io/crates/dssim-core) ([repo](https://github.com/kornelski/dssim)) | 3.5.1 (2026-08-29) | **AGPL-3.0** (or a paid commercial license) | **No** | Pure Rust | 2026-09-24 | PASS (probe) |
| [`butteraugli`](https://crates.io/crates/butteraugli) ([repo](https://github.com/imazen/butteraugli)) | 0.9.3 (2026-05-28) | BSD-3-Clause | Yes | Pure-Rust port of Google's butteraugli | 2026-09-27 | PASS (probe) |
| [`butteraugli-sys`](https://crates.io/crates/butteraugli-sys) ([repo](https://github.com/rust-av/butteraugli-rs)) | 0.1.0 (2021-05-18) | Apache-2.0 | Yes | Binds C++ butteraugli | stale (2021 release) | not probed |
| [`zensim`](https://crates.io/crates/zensim) ([repo](https://github.com/imazen/zensim)) | 0.2.7 (2026-04-27) | MIT OR Apache-2.0 | Yes | Pure Rust (new psychovisual metric) | 2026-09-26 | not probed |

## Umbrella

| Crate | Latest (date) | License | OK? | Pure Rust / binds | Last commit | wasm32 |
|---|---|---|---|---|---|---|
| [`image`](https://crates.io/crates/image) ([repo](https://github.com/image-rs/image)) | 0.25.10 (2026-03-10) | MIT OR Apache-2.0 | Yes | Pure Rust by default. `avif-native` pulls in C dav1d; `nasm` enables ravif asm. | 2026-09-15 | PASS with `default-features = false`, features `jpeg,png,webp,avif,gif` (probe) |
| [`zune-image`](https://crates.io/crates/zune-image) ([repo](https://github.com/etemesi254/zune-image)) | 0.5.0 (2026-01-24) | MIT OR Apache-2.0 OR Zlib | Yes | Pure Rust | 2026-09-24 | PASS with `default-features = false`, `image_formats` (probe) |

**`image` 0.25.10 feature flags**, from the crates.io API:

- `default = ["rayon", "default-formats"]`.
- `default-formats` = avif, bmp, dds, exr, ff, gif, hdr, ico, jpeg, png, pnm, qoi, tga, tiff, webp.
- What each codec feature pulls in:
  - `jpeg` → zune-jpeg (decode). `image` also has its own baseline JPEG encoder (UNVERIFIED that it is still in-tree).
  - `png` → png.
  - `webp` → image-webp, whose encoder is lossless only.
  - `avif` → ravif (encode only).
  - `avif-native` → mp4parse + dav1d (decode).
  - `gif` → gif + color_quant.
- Other flags: `nasm` → `ravif?/asm`, and `rayon` also enables ravif and exr threading.

**`zune-image` 0.5.0** feature `image_formats` covers jpeg (zune-jpeg + jpeg-encoder), png, ppm, psd, farbfeld, qoi, jpeg-xl (zune-jpegxl + jxl-oxide), hdr, bmp and webp (image-webp).

## Recommended stack

One rule shapes these picks: prefer pure-Rust crates, so the native and WASM builds share one code path. C-backed crates should be native-only accelerators.

| Area | Native (CLI / Tauri) primary | Native fallback | WASM primary | WASM fallback |
|---|---|---|---|---|
| JPEG encode | ~~`mozjpeg-rs`~~ **[Reviewer] `mozjpeg` (C, IJG)**. It matches the Squoosh baseline to the byte count (72,721 B for kodim01 at q75), is ~1.5x faster than `mozjpeg-rs` even without nasm SIMD, and is mature. | ~~`mozjpeg` (C; enable `mozjpeg-sys/nasm_simd` for speed)~~ **[Reviewer] `mozjpeg-rs`**, behind a feature. Its output is within ±1.5% in size and 0.25 in SSIMULACRA2, but it is young and AI-generated. | `mozjpeg-rs` (proven to run in wasm) | `jpeg-encoder` |
| JPEG decode | `zune-jpeg` | `jpeg-decoder` | `zune-jpeg` | `jpeg-decoder` |
| PNG decode / encode | `png` (plus `zune-png` for fast decode) | none needed | `png` | none needed |
| PNG optimize | `oxipng` (libdeflate C, builds with MSVC) | `oxipng` with only `zopfli`, or `png` at max compression | `oxipng` + `freestanding`, with clang for wasm32 in CI (compile-checked only) | `png` at max compression (no oxipng) |
| Palette quantization | `quantette` | `color_quant` | `quantette` | `color_quant` |
| WebP decode | `image-webp` | `libwebp-sys` | `image-webp` | browser `createImageBitmap` |
| WebP lossy encode | `libwebp-sys` 0.14 directly (skip `webp`, which pins libwebp-sys 0.9). **[Reviewer] Verified end to end.** | `webp` 0.3 | **No working option found.** Needs libwebp built with a wasm libc sysroot (wasi-sdk, UNVERIFIED) | browser `canvas.toBlob('image/webp')` (Chromium only, UNVERIFIED) |
| WebP lossless encode | `image-webp` | `libwebp-sys` | `image-webp` | none |
| AVIF encode | `ravif` with `asm` (needs nasm in CI) + `threading`. **[Reviewer]** Pin the thread count, because output varies with it. The asm speed is UNMEASURED (no nasm on the probe box). | `ravif` without asm (**[Reviewer]** measured at 442 ms for q50 and 805 ms for q75, speed 6, single-threaded, 0.39 MP) | `ravif` with `default-features = false` (single-threaded, slow; **[Reviewer]** 614 ms for q50 in Node) | none |
| AVIF decode | `avif-decode` (rav1d; **requires nasm** on every CI runner). **[Reviewer]** Or use a `[patch]` copy without asm, verified on MSVC. Better still, upstream an `asm` feature. | `image` `avif-native` / `dav1d` (system libdav1d + pkg-config) | **No crate passed.** Use the browser's native AVIF decode (`createImageBitmap`) | none |
| JPEG XL decode | `jxl-oxide` | `jxl` (libjxl/jxl-rs) | `jxl-oxide` | `jxl` |
| JPEG XL encode | lossless: `zune-jpegxl`. Lossy: **defer**, or write thin in-house BSD bindings to libjxl (native only) | `gamut-jxl` `encode` feature (young, unprobed) | lossless: `zune-jpegxl`. Lossy: none | none |
| Resize | `fast_image_resize` | `resize` | `fast_image_resize` | `resize` |
| Metrics | `ssimulacra2`. **[Reviewer]** It is the dominant cost and memory hog; a reference-caching, memory-lean wrapper is planned in `docs/ARCHITECTURE.md`. | `butteraugli` (imazen, BSD-3) | `ssimulacra2` | `butteraugli` |
| Umbrella / glue | `image` with `default-features = false` and only the needed features, used for I/O glue | `zune-image` | same | same |

## License risks

Crates marked **Exclude** must not be linked into Crumple unless a commercial license is bought.

- **GPL-3.0-or-later: exclude.**
  - `imagequant` / libimagequant v4, which is GPLv3+ or paid commercial. This is the pngquant engine, the obvious palette-quantization choice, and it is off-limits. Use `quantette`.
  - `jpegxl-rs` and `jpegxl-sys`, GPL-3.0-or-later since 0.1.0. This is so even though libjxl itself is BSD-3-Clause, because the Rust wrapper is what is GPL.
- **AGPL-3.0: exclude.**
  - `dssim` and `dssim-core`, which are AGPL or paid commercial.
  - The whole imazen `zen*` family, all "AGPL-3.0-only OR LicenseRef-Imazen-Commercial": `zenjpeg`, `zenpng`, `zenquant`, `zenwebp`, `zenavif`, `zenjxl`, plus `jxl-encoder`, the only pure-Rust lossy JXL encoder.
  - `jpegli-rs` (imazen), AGPL-3.0-or-later.

  Watch out: other imazen crates are permissive (`mozjpeg-rs` BSD-3, `butteraugli` BSD-3, `zensim` MIT/Apache), and their READMEs point to the AGPL siblings. Check each crate's own license, never the publisher's.
- **LGPL:** none found among the surveyed crates.
- **MPL-2.0:** `avif-parse`, a dependency of `avif-decode`. MPL is file-level copyleft: fine to ship unmodified with an Apache-2.0 app, but it must be listed in NOTICE, and any modified MPL source files must be published.
- **Attribution-only obligations** (must be in NOTICE or the about box):
  - the IJG "based in part on the work of the Independent JPEG Group" notice, for `mozjpeg`/`mozjpeg-sys`, `mozjpeg-rs` and `jpeg-encoder`;
  - Zlib and BSD-3 notices from mozjpeg-sys;
  - libwebp's BSD-3 license and patent grant;
  - rav1e (BSD-2) and rav1d/dav1d (BSD-2), plus the AOMedia patent license for AV1 (UNVERIFIED text);
  - oxipng (MIT) and libdeflate (MIT, UNVERIFIED). **[Reviewer] Verified.** The bundled `libdeflate/COPYING` is MIT (© 2016 Eric Biggers, © 2024 Google LLC). The Rust wrappers `libdeflater` and `libdeflate-sys` are Apache-2.0, and `zopfli` is Apache-2.0.
- **[Reviewer] Transitive scan of the native stack.** The probe's dependency graph (`cargo tree -e normal,build --target x86_64-pc-windows-msvc`) contains `mozjpeg-rs`, `mozjpeg`, `ravif`, `libwebp-sys`, `oxipng`, `ssimulacra2`, `zune-jpeg`, `image-webp`, `avif-decode` and `png`. It has **no GPL, AGPL or LGPL** crates. The less common license expressions are:
  - `MPL-2.0` (`avif-parse`)
  - `IJG` (`mozjpeg`)
  - `IJG AND Zlib AND BSD-3-Clause` (`mozjpeg-sys`)
  - `CC0-1.0` (`to_method`)
  - `CC0-1.0 OR Apache-2.0` (`imgref`)
  - `(MIT OR Apache-2.0) AND Unicode-3.0` (`unicode-ident`)
  - `0BSD OR MIT OR Apache-2.0`
  - `BSD-3-Clause OR Apache-2.0` (`yuv`)

  The allow-list in `docs/ARCHITECTURE.md` covers all of them.
- **Transitive check:** this survey only read each crate's declared license. Once the dependency set is fixed, run `cargo deny check licenses` (or `cargo about`) in CI with an allow-list that forbids GPL, AGPL and LGPL.

## Open questions

1. **Lossy WebP in the browser.** Can `libwebp-sys` be compiled for wasm32-unknown-unknown with a wasi-sdk or wasi-libc sysroot (`CFLAGS_wasm32_unknown_unknown=--sysroot=...`)? Or should the PWA use `wasm32-wasip1` or Emscripten for the C codecs, or the browser's canvas encoder? Plain LLVM clang failed on missing libc headers.
2. **AVIF decode in the browser.** rav1d does not compile for wasm32-unknown-unknown (`libc::ptrdiff_t`). Is the browser's native AVIF decode acceptable, given it has no bit-exact control? Or should libdav1d be built via Emscripten?
3. **nasm in CI.** `avif-decode` (rav1d) cannot turn off asm, so every native CI runner, and every contributor, needs nasm. Alternatively, patch or feature-gate it upstream.
4. **Lossy JXL encode.** Is JXL output in scope for v1? If it is, options are thin in-house bindings to BSD-licensed libjxl (C++/CMake on three OSes), trying `gamut-jxl`'s `encode` feature, or a commercial license for `jxl-encoder`.
5. **mozjpeg-rs quality claims.** Is its output really byte-identical to C mozjpeg, and is its speed on WASM (no SIMD) acceptable? Confirm with the bench harness before committing to it. It is young (~~first released 2024~~ **[Reviewer] first released 2025-12-29**, about 111k downloads, 81k of them on 0.9.2). **[Reviewer] Answered.** It is not byte-identical in the trellis and optimize_scans configuration, but it is within ±1.5% in size and 0.25 in SSIMULACRA2. In wasm, 768x512 at q75 takes 78 ms (1.5x native). Acceptable for the PWA, but not the native primary.
6. **jpegli.** The probe hit a Windows MAX_PATH error from the long scratch path. Retry from a short path, or with long paths enabled, to learn whether `jpegli` 0.1.0 (which pins libjxl 0.10.2 and was last touched in 2024-04) builds at all. It is probably not worth adopting. **[Reviewer] Answered.** It compiles but fails to link (`jpegli-static.lib` sits under `Release/`). With a manual `-L` it runs, but on kodim01 it shows no size/quality gain over mozjpeg and is ~15x slower. Defer. jpegli remains Squoosh's most requested feature (#1408), so revisit it with the official libjxl `cjpegli` or a fixed binding later.
7. **Probes were `cargo check` only.** Linking, a real encode/decode round trip, and wasm module size and speed (`wasm-bindgen` / `wasm-pack`) have not been measured. That is especially important for `oxipng` + `freestanding`, where the C was compiled but never linked. **[Reviewer] Partly answered.** The native stack was linked and run (see below). A pure-Rust wasm32-unknown-unknown module (png, mozjpeg-rs, ravif, ssimulacra2, zune-jpeg) was linked and run in Node 24. It is 1.56 MB, not wasm-opt'ed. `v_frame` (used by rav1e and yuvxyb) pulls in `wasm-bindgen`, so the module imports `__wbindgen_*` symbols; plan on wasm-bindgen for the PWA. `oxipng` + `freestanding` in wasm is still unlinked.
8. **macOS and Linux native probes were not run.** Every native result above is Windows MSVC only.
9. **zune-jpeg 0.5.16** is in RC (rc2 on 2026-09-08). Pin to 0.5.15, or track the RC?

## Native end-to-end probe (reviewer)

**[Reviewer]** This section was added on 2026-09-27 and records what actually works on Windows MSVC.

**Setup**

- Source: `scratchpad\native-probe\` (see Sources), built with `CARGO_TARGET_DIR=%TEMP%\cnp` to stay under MAX_PATH.
- Toolchain: rustc 1.98.1, VS 2022 Build Tools, **no nasm, no clang on PATH**.
- Dependencies: `png` 0.18.1, `mozjpeg-rs` 0.9.2, `mozjpeg` 0.10.13, `ravif` 0.13.0 (`default-features = false, features = ["threading"]`), `libwebp-sys` 0.14.4, `oxipng` 10.2.1 (`default-features = false, features = ["parallel","zopfli"]`), `ssimulacra2` 0.5.1, `zune-jpeg` 0.5.15, `image-webp` 0.2.4, and `avif-decode` 3.0.0 through the no-asm `[patch]` described above.
- Hardware: i7-14700KF.
- Timing: the process was pinned to the P-cores (`start /affinity FFFF`), because unpinned single-thread timings varied by 2x from P-core/E-core scheduling.

**Result:** all four encoders and all three decoders built, linked and ran. oxipng output is pixel-identical to its input.

Microbench on kodim01 (768x512), `RAYON_NUM_THREADS=1`, P-cores, 10 runs:

```text
  encode mozjpeg-rs q75  min    50.6  median    50.9
  encode mozjpeg-C q75   min    43.7  median    44.0     (no SIMD: nasm absent)
  encode webp q75        min    25.9  median    26.0
  encode avif q50 s6     min   438.3  median   441.6     (ravif, no asm)
  encode avif q75 s6     min   798.7  median   804.7
  decode jpeg (zune)     min     2.4  median     2.6
  decode avif (rav1d,noasm) min  14.5  median    16.0
  score  ssimulacra2     min   105.0  median   107.2
  oxipng preset 2        min   145.5  median   147.3
```

The same run on `example.png` (2558x1348, 3.45 MP) gave:

| Step | Time (ms) |
|---|---:|
| mozjpeg-rs encode | 275 |
| mozjpeg-C encode | 130 |
| webp encode | 141 |
| avif encode, q50 | 3,086 |
| avif encode, q75 | 5,123 |
| ssimulacra2 score | 1,043 |
| oxipng | 3,053 |

Peak working set was 558 MiB. SSIMULACRA2 alone peaked at ~144 B/px.

Squoosh-default-like settings on kodim01, compared with the jSquash baseline in `bench/baseline/results.json`:

| Codec | Crumple native bytes | ssimu2 | Squoosh/jSquash bytes | ssimu2 |
|---|---:|---:|---:|---:|
| C mozjpeg q75 (progressive, max-compression defaults) | 72,721 | 71.74 | 72,721 | 71.85 |
| libwebp q75 (`WebPEncodeRGB`) | 77,422 | 72.72 | 77,422 | 72.72 |
| mozjpeg-rs q75 `max_compression()` | 73,093 | 71.63 | n/a | n/a |

**The files are byte-identical, not just the same size.** The SHA-256 of the native C mozjpeg q75 and libwebp q75 outputs equals that of the jSquash baseline files in `bench/out/{mozjpeg,webp}/` for kodim01, kodim13 and `example`. For those two codecs, native Crumple and a jSquash-based PWA would write the same bytes.

The 0.11-point mozjpeg score gap therefore comes from the decoder alone: this probe decodes with zune-jpeg, while the baseline used jSquash's libjpeg. **Always score with one fixed decoder.**

Quality-target search: naive binary search on q in 1-100, `RAYON_NUM_THREADS=1`, P-cores. Each step is one encode, decode and score. The trailing number is the total time for the codec's search.

```text
search mozjpeg-rs  target 80: q87   109155 B  score  80.33  encodes  8  total   1448 ms
search mozjpeg-C   target 80: q87   109201 B  score  80.38  encodes  8  total   1468 ms
search webp        target 80: q83   103390 B  score  80.41  encodes  8  total   1194 ms
search avif        target 80: q86   100585 B  score  80.76  encodes  8  total   8409 ms
search mozjpeg-C   target 70: q73    69127 B  score  70.02  encodes  8  total   1381 ms
search webp        target 70: q69    73142 B  score  71.04  encodes  7  total   1024 ms
search avif        target 70: q78    67609 B  score  70.20  encodes  8  total   8180 ms
search mozjpeg-C   target 90: q98   250717 B  score  91.08  encodes  8  total   1708 ms
search webp        target 90: q96   198344 B  score  90.59  encodes  8  total   1228 ms
search avif        target 90: q94   172964 B  score  90.90  encodes  7  total   7679 ms
```

On kodim01, ravif is smallest at all three targets:

| Target | ravif vs best other codec |
|---:|---|
| 70 | 2.2% smaller than C mozjpeg |
| 80 | 2.7% smaller than WebP |
| 90 | 12.8% smaller than WebP |

The JPEG/WebP order flips with the target: JPEG is smaller at 70, WebP at 80 and 90. Per-image choice therefore matters, especially when AVIF is not allowed. Score was monotonic in q at every probed point.

Parallel batch: 25 images, each through mozjpeg-rs, webp, avif-q50 and oxipng, with 3 decodes and 3 scores per image, using rayon over images.

| Rayon threads | Wall time | Peak working set |
|---:|---:|---:|
| 28 (default) | 19.4 s | **1,117 MiB** |
| 8 | 25.4 s | 568 MiB |

For comparison, Squoosh's sequential baseline peaked at 829 MiB (`bench/baseline/RESULTS.md`, re-run pinned to the P-cores). Memory is not bounded unless Crumple bounds it.

Parity of mozjpeg-rs against C mozjpeg across the corpus (`native-probe jpegcmp`, same decoder for both):

```text
q60  25 imgs: total bytes rs/C -0.52%, per-image -1.48%..+0.28%, mean ssimu2 delta -0.218, same-size files 0, enc time sum rs 1.29s C 0.79s
q75  25 imgs: total bytes rs/C +0.03%, per-image -0.96%..+0.91%, mean ssimu2 delta -0.143, same-size files 0, enc time sum rs 1.66s C 1.13s
q90  25 imgs: total bytes rs/C +0.20%, per-image -0.48%..+0.85%, mean ssimu2 delta -0.023, same-size files 0, enc time sum rs 11.86s C 7.29s
```

Pure-Rust wasm32-unknown-unknown, `scratchpad\wasm-probe\`, Node 24, no simd128. Each ratio is against the native single-thread P-core figure:

| Step | Result | vs native |
|---|---|---:|
| mozjpeg-rs q75 | 73,093 B (same as native), 78 ms | 1.5x |
| ssimulacra2 | 197 ms | 1.8x |
| ravif q50 s6 | 36,604 B (same as native single-thread), 614 ms | 1.4x |

The module is 1.56 MB and uses 62 MiB of linear memory.

## Sources

- **[Reviewer]** Probe crates and logs (outside the repo, in a local scratch directory, not published):
  - `native-probe\` (`src\main.rs`, `run-*.txt`)
  - `wasm-probe\` (`run.txt`)
  - `jpegli-probe\` (`run.txt`)
- crates.io API: `https://crates.io/api/v1/crates/<name>` for every crate named above, plus `/<version>/dependencies` and `/versions` for dependency and license history.
- Repos: the URLs in each table, queried with `gh api repos/<owner>/<repo>`, `/commits/<branch>`, `/license` and `/readme`.
- License texts read directly:
  - [libimagequant COPYRIGHT](https://github.com/ImageOptim/libimagequant/blob/main/COPYRIGHT) and its README "License" section;
  - [dssim README](https://github.com/kornelski/dssim);
  - [jpegxl-rs README](https://github.com/inflation/jpegxl-rs);
  - [mozjpeg-rs LICENSE](https://github.com/imazen/mozjpeg-rs/blob/main/LICENSE);
  - [jxl-encoder README](https://github.com/imazen/jxl-encoder).
- Feature facts from READMEs:
  - [image-webp README](https://github.com/image-rs/image-webp) (lossless-only encoder);
  - [zune-jpegxl README](https://github.com/etemesi254/zune-image/tree/dev/crates/zune-jpegxl) (lossless encoder);
  - [image README](https://github.com/image-rs/image) (`avif-native`, `nasm`);
  - [gamut README](https://github.com/justin13888/gamut).
- Probe crates and logs, kept outside the repo in a local scratch directory (not published): `codec-probe\` (`results.txt`, `log-*.txt`).
