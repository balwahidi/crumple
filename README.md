# Crumple

> **Status: pre-alpha. Nothing works yet.** This repository is a scaffold.
> There is no usable optimizer, no stable CLI, and no release.

Crumple is a planned open-source, local-first batch image optimizer.

## Vision

- Drop in a folder of images and set a quality target, for example
  "visually lossless" or "SSIMULACRA2 ≥ 80".
- Crumple picks the codec and settings for each image to meet that target.
- Everything runs on your machine. Crumple never uploads your images.

One Rust core, shared by three front ends:

- `crumple`, a command-line tool
- a desktop app (Tauri)
- a PWA (the core compiled to WebAssembly)

## Layout

- `crates/crumple-core` — the shared library (empty placeholder for now)
- `crates/crumple-cli` — the `crumple` binary (prints "not implemented yet")

## Building

```sh
cargo build
cargo test
cargo run -p crumple-cli -- --version
```

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
  benchmark tools only: Crumple itself ships no codec yet. Each codec Crumple
  ships will be listed here and in NOTICE, with its license.
- [SSIMULACRA2](https://github.com/cloudinary/ssimulacra2), the perceptual
  metric behind the quality targets. The benchmark uses the Rust port
  [`ssimulacra2_rs`](https://github.com/rust-av/ssimulacra2_bin) (BSD-2-Clause).
