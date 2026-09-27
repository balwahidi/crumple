//! [Reviewer] Decoder robustness: truncated, corrupted and hostile inputs must give `Err`
//! (or a well-formed `Ok`), never a panic, an abort or a huge allocation.
#![cfg(all(feature = "jpeg", feature = "webp", feature = "avif", feature = "png"))]

use crumple_codecs::{decode, decode_with_limit, encode, Codec, CodecError};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Once;

fn quiet_panics() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("CRUMPLE_FUZZ_VERBOSE").is_none() {
            std::panic::set_hook(Box::new(|_| {}));
        }
    });
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

fn image(w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let a = if alpha { (x * 9 + y) as u8 | 1 } else { 255 };
            v.extend_from_slice(&[(x * 7) as u8, (y * 11) as u8, (x ^ y) as u8, a]);
        }
    }
    v
}

fn png_bytes(
    w: u32,
    h: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    data: &[u8],
    setup: impl FnOnce(&mut png::Encoder<&mut Vec<u8>>),
) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(color);
        e.set_depth(depth);
        setup(&mut e);
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(data).unwrap();
        wr.finish().unwrap();
    }
    out
}

fn samples() -> Vec<(&'static str, Vec<u8>)> {
    let (w, h) = (40u32, 24u32);
    let opaque = image(w, h, false);
    let alpha = image(w, h, true);
    let icc: Vec<u8> = (0..700).map(|i| (i * 13 % 256) as u8).collect();
    let mut v = vec![
        (
            "jpeg",
            encode(Codec::Jpeg, &opaque, w, h, 80, None).unwrap(),
        ),
        (
            "jpeg+icc",
            encode(Codec::Jpeg, &opaque, w, h, 80, Some(&icc)).unwrap(),
        ),
        (
            "webp",
            encode(Codec::WebpLossy, &opaque, w, h, 80, None).unwrap(),
        ),
        (
            "webp+alpha",
            encode(Codec::WebpLossy, &alpha, w, h, 80, None).unwrap(),
        ),
        (
            "webp+icc",
            encode(Codec::WebpLossy, &opaque, w, h, 80, Some(&icc)).unwrap(),
        ),
        (
            "png",
            encode(Codec::PngLossless, &opaque, w, h, 0, None).unwrap(),
        ),
        (
            "png+alpha+icc",
            encode(Codec::PngLossless, &alpha, w, h, 0, Some(&icc)).unwrap(),
        ),
        (
            "avif",
            encode(Codec::Avif, &opaque, w, h, 60, None).unwrap(),
        ),
        (
            "avif+alpha",
            encode(Codec::Avif, &alpha, w, h, 60, None).unwrap(),
        ),
    ];
    // Lossless WebP (VP8L) with EXIF, from image-webp's own encoder.
    let mut lossless = Vec::new();
    let mut enc = image_webp::WebPEncoder::new(&mut lossless);
    enc.set_exif_metadata(exif_tiff(6));
    enc.encode(&alpha, w, h, image_webp::ColorType::Rgba8)
        .unwrap();
    v.push(("webp-lossless+exif", lossless));
    // Baseline, grayscale and CMYK JPEGs straight from mozjpeg.
    let rgb: Vec<u8> = opaque
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let gray: Vec<u8> = opaque.as_chunks::<4>().0.iter().map(|p| p[1]).collect();
    let cmyk: Vec<u8> = opaque
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2], 40])
        .collect();
    v.push((
        "jpeg-baseline",
        mozjpeg_raw(mozjpeg::ColorSpace::JCS_RGB, &rgb, w, h, true),
    ));
    v.push((
        "jpeg-gray",
        mozjpeg_raw(mozjpeg::ColorSpace::JCS_GRAYSCALE, &gray, w, h, false),
    ));
    v.push((
        "jpeg-cmyk",
        mozjpeg_raw(mozjpeg::ColorSpace::JCS_CMYK, &cmyk, w, h, false),
    ));
    // 16-bit RGBA PNG and a two-frame APNG.
    let rgba16: Vec<u8> = alpha.iter().flat_map(|&b| [b, b ^ 0x5A]).collect();
    v.push((
        "png-rgba16",
        png_bytes(
            w,
            h,
            png::ColorType::Rgba,
            png::BitDepth::Sixteen,
            &rgba16,
            |_| {},
        ),
    ));
    let mut apng = Vec::new();
    {
        let mut e = png::Encoder::new(&mut apng, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.set_animated(2, 0).unwrap();
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(&alpha).unwrap();
        wr.write_image_data(&opaque).unwrap();
        wr.finish().unwrap();
    }
    v.push(("apng", apng));
    // Palette PNG with tRNS.
    let idx: Vec<u8> = (0..w * h).map(|i| (i % 4) as u8).collect();
    v.push((
        "png-palette-trns",
        png_bytes(
            w,
            h,
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            &idx,
            |e| {
                e.set_palette(vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
                e.set_trns(vec![0, 128]);
            },
        ),
    ));
    v
}

fn mozjpeg_raw(cs: mozjpeg::ColorSpace, data: &[u8], w: u32, h: u32, baseline: bool) -> Vec<u8> {
    let mut c = mozjpeg::Compress::new(cs);
    if baseline {
        c.set_fastest_defaults();
    }
    c.set_size(w as usize, h as usize);
    c.set_quality(85.0);
    let mut c = c.start_compress(Vec::new()).unwrap();
    c.write_scanlines(data).unwrap();
    c.finish().unwrap()
}

/// Little-endian TIFF with one IFD entry: Orientation (0x0112) SHORT = `o`.
fn exif_tiff(o: u16) -> Vec<u8> {
    let mut t = b"II*\0".to_vec();
    t.extend_from_slice(&8u32.to_le_bytes());
    t.extend_from_slice(&1u16.to_le_bytes());
    t.extend_from_slice(&0x0112u16.to_le_bytes());
    t.extend_from_slice(&3u16.to_le_bytes());
    t.extend_from_slice(&1u32.to_le_bytes());
    t.extend_from_slice(&o.to_le_bytes());
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&0u32.to_le_bytes());
    t
}

/// avif-parse 2.1.0 has `debug_assert_eq!(0, limit, "bad parser state bytes left")` for
/// malformed boxes: with debug assertions on (the dev/test profile) it panics, in release
/// it returns `Err`. Only that known debug-only panic is tolerated, and only in debug.
fn known_debug_only_panic(msg: &str) -> bool {
    cfg!(debug_assertions) && msg.contains("bad parser state bytes left")
}

/// Decodes `bytes`; returns a description if it panicked or returned a malformed `Ok`.
fn check(bytes: &[u8]) -> Option<String> {
    match catch_unwind(AssertUnwindSafe(|| decode(bytes))) {
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            (!known_debug_only_panic(&msg)).then_some(msg)
        }
        Ok(Ok(d)) => {
            let want = d.width as usize * d.height as usize * 4;
            (d.rgba.len() != want || d.width == 0 || d.height == 0).then(|| {
                format!(
                    "Ok with {} bytes for {}x{} RGBA",
                    d.rgba.len(),
                    d.width,
                    d.height
                )
            })
        }
        Ok(Err(_)) => None,
    }
}

fn report(failures: Vec<String>) {
    if !failures.is_empty() {
        let n = failures.len();
        let shown: Vec<_> = failures.into_iter().take(25).collect();
        // Printed as well, because the quiet panic hook swallows the panic message.
        let msg = format!("{n} failures, first ones:\n{}", shown.join("\n"));
        eprintln!("{msg}");
        panic!("{msg}");
    }
}

#[test]
fn samples_decode_cleanly() {
    for (name, bytes) in samples() {
        let d = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(d.rgba.len(), 40 * 24 * 4, "{name}");
    }
}

#[test]
fn truncations_never_panic() {
    quiet_panics();
    let mut failures = Vec::new();
    for (name, bytes) in samples() {
        let n = bytes.len();
        let step = (n / 700).max(1);
        let offsets = (0..n).step_by(step).chain(n.saturating_sub(64)..n);
        for cut in offsets {
            if let Some(f) = check(&bytes[..cut]) {
                failures.push(format!("{name} truncated to {cut}/{n}: {f}"));
            }
        }
    }
    report(failures);
}

#[test]
fn corruptions_never_panic() {
    quiet_panics();
    let mut rng = Lcg(0x5eed);
    let mut failures = Vec::new();
    // AVIF is left out: rav1d 1.1.0 panics inside `extern "C"` functions on some corrupt
    // AV1 data, which aborts the whole process (see `rav1d_aborts_on_corrupt_avif`).
    for (name, bytes) in samples()
        .into_iter()
        .filter(|(n, _)| !n.starts_with("avif"))
    {
        let rounds = std::env::var("CRUMPLE_FUZZ_ROUNDS")
            .ok()
            .and_then(|r| r.parse().ok())
            .unwrap_or(if cfg!(debug_assertions) { 400 } else { 1500 });
        for round in 0..rounds {
            let mut m = bytes.clone();
            for _ in 0..1 + rng.below(4) {
                let at = rng.below(m.len());
                match rng.below(3) {
                    0 => m[at] ^= 1 << rng.below(8),
                    1 => m[at] = rng.byte(),
                    _ => m[at] = [0x00, 0xFF, 0x7F, 0x80][rng.below(4)],
                }
            }
            if std::env::var_os("CRUMPLE_FUZZ_VERBOSE").is_some() {
                let diff: Vec<_> = bytes
                    .iter()
                    .zip(&m)
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(i, (a, b))| (i, *a, *b))
                    .collect();
                eprintln!("{name} len {} round {round} diff {diff:?}", bytes.len());
            }
            if let Some(f) = check(&m) {
                failures.push(format!("{name} mutation round {round}: {f}"));
            }
        }
    }
    report(failures);
}

#[test]
fn random_bytes_with_valid_magic_never_panic() {
    quiet_panics();
    let mut rng = Lcg(42);
    let magics: [&[u8]; 5] = [
        b"\x89PNG\r\n\x1a\n",
        &[0xFF, 0xD8, 0xFF],
        b"RIFF\0\0\0\0WEBP",
        b"RIFF\x40\0\0\0WEBPVP8X\x0a\0\0\0",
        b"\0\0\0\x18ftypavif\0\0\0\0avifmif1",
    ];
    let mut failures = Vec::new();
    for magic in magics {
        for len in [0usize, 1, 7, 16, 33, 100, 1000, 5000] {
            for _ in 0..40 {
                let mut b = magic.to_vec();
                b.extend((0..len).map(|_| rng.byte()));
                if let Some(f) = check(&b) {
                    failures.push(format!("magic {:02x?} + {len} random: {f}", &magic[..4]));
                }
            }
        }
    }
    report(failures);
}

/// Hostile EXIF payloads in all three containers must fall back to orientation 1.
#[test]
fn hostile_exif_never_panics() {
    quiet_panics();
    let mut rng = Lcg(7);
    let (w, h) = (16u32, 16u32);
    let px = image(w, h, false);
    let rgb: Vec<u8> = px
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let base = exif_tiff(6);
    let mut failures = Vec::new();
    for round in 0..400 {
        let mut tiff = if round % 4 == 0 {
            (0..rng.below(200)).map(|_| rng.byte()).collect()
        } else {
            let mut t = base.clone();
            for _ in 0..1 + rng.below(3) {
                let at = rng.below(t.len());
                t[at] = rng.byte();
            }
            t
        };
        if tiff.is_empty() {
            tiff.push(0);
        }
        // JPEG APP1
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        c.set_size(w as usize, h as usize);
        let mut c = c.start_compress(Vec::new()).unwrap();
        c.write_marker(mozjpeg::Marker::APP(1), &app1);
        c.write_scanlines(&rgb).unwrap();
        let jpg = c.finish().unwrap();
        // PNG eXIf
        let png = png_bytes(
            w,
            h,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &rgb,
            |_| {},
        );
        let png = insert_png_chunk_after_ihdr(&png, b"eXIf", &tiff);
        // WebP EXIF
        let mut webp = Vec::new();
        let mut enc = image_webp::WebPEncoder::new(&mut webp);
        enc.set_exif_metadata(tiff.clone());
        enc.encode(&rgb, w, h, image_webp::ColorType::Rgb8).unwrap();
        for (name, bytes) in [("jpeg", jpg), ("png", png), ("webp", webp)] {
            if let Some(f) = check(&bytes) {
                failures.push(format!("{name} exif round {round}: {f}"));
            } else if let Ok(d) = decode(&bytes) {
                if !(1..=8).contains(&d.exif_orientation) {
                    failures.push(format!("{name}: orientation {}", d.exif_orientation));
                }
            }
        }
    }
    report(failures);
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    let crc = crc32(&c[4..]);
    c.extend_from_slice(&crc.to_be_bytes());
    c
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn insert_png_chunk_after_ihdr(png: &[u8], kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    // Signature (8) + IHDR chunk (4 len + 4 type + 13 data + 4 crc) = 33 bytes.
    let mut out = png[..33].to_vec();
    out.extend(png_chunk(kind, data));
    out.extend_from_slice(&png[33..]);
    out
}

/// Tiny files that declare enormous images must be refused before the pixel
/// buffer is allocated (a panic, an abort or a multi-GB allocation all fail this).
#[test]
fn decompression_bombs_are_refused() {
    let big = |e: &Result<crumple_codecs::Decoded, CodecError>| {
        matches!(e, Err(CodecError::TooLarge { .. }))
    };

    // PNG: IHDR says 60000 x 60000 RGBA16, followed by a tiny IDAT.
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&60_000u32.to_be_bytes());
    ihdr.extend_from_slice(&60_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[16, 6, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(png_chunk(b"IHDR", &ihdr));
    png.extend(png_chunk(
        b"IDAT",
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    ));
    png.extend(png_chunk(b"IEND", &[]));
    assert!(big(&decode(&png)), "png bomb");

    // JPEG: a valid 16x16 JPEG with its SOF0 dimensions patched to 65000 x 65000.
    let px = image(16, 16, false);
    let mut jpg = encode(Codec::Jpeg, &px, 16, 16, 80, None).unwrap();
    let sof = jpg
        .windows(2)
        .position(|w| w[0] == 0xFF && (w[1] == 0xC0 || w[1] == 0xC2))
        .expect("SOF marker");
    jpg[sof + 5..sof + 7].copy_from_slice(&65_000u16.to_be_bytes());
    jpg[sof + 7..sof + 9].copy_from_slice(&65_000u16.to_be_bytes());
    assert!(big(&decode(&jpg)), "jpeg bomb");

    // WebP VP8X: canvas 60000 x 60000 (image-webp itself only refuses w*h >= 2^32)
    // declared in a 40-byte file.
    let mut webp = b"RIFF".to_vec();
    webp.extend_from_slice(&0u32.to_le_bytes());
    webp.extend_from_slice(b"WEBPVP8X");
    webp.extend_from_slice(&10u32.to_le_bytes());
    webp.extend_from_slice(&[0, 0, 0, 0]);
    webp.extend_from_slice(&[0x5F, 0xEA, 0x00, 0x5F, 0xEA, 0x00]);
    webp.extend_from_slice(b"VP8L");
    webp.extend_from_slice(&5u32.to_le_bytes());
    webp.extend_from_slice(&[0x2f, 0xFF, 0xFF, 0xFF, 0x0F, 0]);
    let riff = (webp.len() - 8) as u32;
    webp[4..8].copy_from_slice(&riff.to_le_bytes());
    let r = decode(&webp);
    assert!(big(&r), "webp bomb: {:?}", r.err());

    // WebP: a small valid image with an ICCP chunk claiming 4 GiB.
    let icc = vec![1u8; 64];
    let mut webp = encode(Codec::WebpLossy, &px, 16, 16, 80, Some(&icc)).unwrap();
    let at = webp.windows(4).position(|w| w == b"ICCP").unwrap();
    webp[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(check(&webp).is_none());
    assert!(decode(&webp).is_err(), "oversized ICCP chunk accepted");

    // The limit is the caller's: a 16x16 image with a 255-pixel limit.
    let ok = encode(Codec::PngLossless, &px, 16, 16, 0, None).unwrap();
    assert!(decode_with_limit(&ok, 256).is_ok());
    assert!(big(&decode_with_limit(&ok, 255)), "limit not applied");
    for codec in [Codec::Jpeg, Codec::WebpLossy, Codec::Avif] {
        let b = encode(codec, &px, 16, 16, 80, None).unwrap();
        assert!(decode_with_limit(&b, 256).is_ok(), "{codec:?}");
        assert!(
            big(&decode_with_limit(&b, 255)),
            "{codec:?} limit not applied"
        );
    }
}

/// Reproducer, ignored because it ABORTS the test process (it cannot be caught):
/// rav1d 1.1.0 hits `Option::unwrap()` on `None` in `rav1d_decode_frame_init_cdf`
/// (src/decode.rs:4997) under `dav1d_send_data`, which is `extern "C"`, so the panic
/// cannot unwind. Two corrupted bytes in a 364-byte AVIF from our own encoder suffice.
/// Consequence: `decode` must only ever see AVIF bytes that `encode` produced.
#[test]
#[ignore = "aborts the process: rav1d panics across extern \"C\""]
fn rav1d_aborts_on_corrupt_avif() {
    let px = image(40, 24, false);
    let mut avif = encode(Codec::Avif, &px, 40, 24, 60, None).unwrap();
    assert_eq!(avif.len(), 364);
    assert_eq!((avif[192], avif[352]), (129, 255));
    avif[192] = 61;
    avif[352] = 247;
    let _ = decode(&avif);
}

/// Sizes outside what a codec supports must come back as `Err`, not a panic or abort.
/// (mozjpeg reports libjpeg errors by unwinding through C; that must stay catchable.)
#[test]
fn encoders_handle_extreme_sizes() {
    let all = [
        Codec::Jpeg,
        Codec::WebpLossy,
        Codec::Avif,
        Codec::PngLossless,
    ];
    // Tiny sizes for every codec; just past JPEG's 65,500 and WebP's 16,383 limits.
    let cases: [(u32, u32, &[Codec]); 6] = [
        (1, 1, &all),
        (2, 1, &all),
        (1, 3, &all),
        (65_501, 1, &[Codec::Jpeg]),
        (16_384, 1, &[Codec::WebpLossy]),
        (1, 16_384, &[Codec::WebpLossy]),
    ];
    for (w, h, codecs) in cases {
        let px = [200u8, 100, 50, 255].repeat((w * h) as usize);
        for &codec in codecs {
            let r = catch_unwind(AssertUnwindSafe(|| encode(codec, &px, w, h, 80, None)));
            let r = r.unwrap_or_else(|_| panic!("{codec:?} {w}x{h} panicked"));
            match r {
                Ok(bytes) => {
                    let d = decode(&bytes).unwrap_or_else(|e| panic!("{codec:?} {w}x{h}: {e}"));
                    assert_eq!((d.width, d.height), (w, h), "{codec:?}");
                }
                Err(e) => eprintln!("{codec:?} {w}x{h}: {e}"),
            }
        }
    }
    // Buffer/size mismatches, including sizes whose byte count overflows.
    let px = vec![0u8; 16];
    for (w, h) in [
        (0u32, 4u32),
        (4, 0),
        (1 << 31, 1 << 31),
        (u32::MAX, u32::MAX),
        (3, 1),
    ] {
        for codec in [
            Codec::Jpeg,
            Codec::WebpLossy,
            Codec::Avif,
            Codec::PngLossless,
        ] {
            assert!(
                matches!(
                    encode(codec, &px, w, h, 80, None),
                    Err(CodecError::Encode(_))
                ),
                "{codec:?} {w}x{h}"
            );
        }
    }
}
