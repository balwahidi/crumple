#![cfg(all(feature = "jpeg", feature = "webp", feature = "avif", feature = "png"))]

use crumple_codecs::{decode, encode, Codec, CodecError, Format};
use ssimulacra2::{compute_frame_ssimulacra2, ColorPrimaries, Rgb, TransferCharacteristic};

fn kodim01() -> Option<(Vec<u8>, u32, u32)> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../bench/corpus/kodak/kodim01.png"
    );
    match std::fs::read(path) {
        Ok(b) => {
            let d = decode(&b).expect("decode kodim01");
            Some((d.rgba, d.width, d.height))
        }
        Err(_) => {
            eprintln!("SKIP: {path} missing (run node bench/corpus/fetch.mjs)");
            None
        }
    }
}

fn srgb(rgba: &[u8], w: u32, h: u32) -> Rgb {
    let data = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            [
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            ]
        })
        .collect();
    Rgb::new(
        data,
        w as usize,
        h as usize,
        TransferCharacteristic::SRGB,
        ColorPrimaries::BT709,
    )
    .unwrap()
}

fn score(orig: &[u8], encoded: &[u8], w: u32, h: u32) -> f64 {
    let d = decode(encoded).unwrap();
    assert_eq!((d.width, d.height), (w, h));
    compute_frame_ssimulacra2(srgb(orig, w, h), srgb(&d.rgba, w, h)).unwrap()
}

fn within_pct(actual: usize, expected: f64, pct: f64) {
    let dev = (actual as f64 / expected - 1.0).abs() * 100.0;
    assert!(
        dev <= pct,
        "{actual} B is {dev:.3}% off {expected} (limit {pct}%)"
    );
}

#[test]
fn a_jpeg_q75() {
    let Some((px, w, h)) = kodim01() else { return };
    let out = encode(Codec::Jpeg, &px, w, h, 75, None).unwrap();
    let s = score(&px, &out, w, h);
    eprintln!("jpeg q75: {} B, score {s:.3}", out.len());
    within_pct(out.len(), 72_721.0, 0.5);
    assert!((s - 71.74).abs() <= 0.2, "score {s}");
}

#[test]
fn b_webp_q75() {
    let Some((px, w, h)) = kodim01() else { return };
    let out = encode(Codec::WebpLossy, &px, w, h, 75, None).unwrap();
    let s = score(&px, &out, w, h);
    eprintln!("webp q75: {} B, score {s:.3}", out.len());
    within_pct(out.len(), 77_422.0, 0.5);
    assert!((s - 72.72).abs() <= 0.2, "score {s}");
}

#[test]
fn c_avif_q50() {
    let Some((px, w, h)) = kodim01() else { return };
    let a = encode(Codec::Avif, &px, w, h, 50, None).unwrap();
    let b = encode(Codec::Avif, &px, w, h, 50, None).unwrap();
    eprintln!("avif q50: {} B", a.len());
    assert_eq!(a, b, "AVIF output is not deterministic");
    within_pct(a.len(), 36_604.0, 3.0);
    assert_eq!(decode(&a).unwrap().format, Format::Avif);
}

#[test]
fn d_png_lossless() {
    let Some((px, w, h)) = kodim01() else { return };
    let out = encode(Codec::PngLossless, &px, w, h, 0, None).unwrap();
    eprintln!("png: {} B", out.len());
    assert!(out.len() <= 700_000);
    assert!(decode(&out).unwrap().rgba == px);
}

fn small_image(alpha: bool) -> (Vec<u8>, u32, u32) {
    let (w, h) = (64u32, 48u32);
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let a = if alpha { ((x * 4) as u8).max(16) } else { 255 };
            v.extend_from_slice(&[(x * 4) as u8, (y * 5) as u8, 128, a]);
        }
    }
    (v, w, h)
}

#[test]
fn e_icc_round_trip() {
    let blob: Vec<u8> = (0..3144).map(|i| (i * 7 % 251) as u8).collect();
    let (px, w, h) = small_image(false);
    for codec in [Codec::Jpeg, Codec::WebpLossy, Codec::PngLossless] {
        let out = encode(codec, &px, w, h, 80, Some(&blob)).unwrap();
        assert_eq!(decode(&out).unwrap().icc, Some(blob.clone()), "{codec:?}");
    }
    assert!(matches!(
        encode(Codec::Avif, &px, w, h, 80, Some(&blob)),
        Err(CodecError::Unsupported("icc"))
    ));
}

#[test]
fn f_orientation() {
    // Hand-made Exif APP1: little-endian TIFF, one IFD entry 0x0112 SHORT = 6.
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(b"II*\0");
    app1.extend_from_slice(&8u32.to_le_bytes());
    app1.extend_from_slice(&1u16.to_le_bytes());
    app1.extend_from_slice(&0x0112u16.to_le_bytes());
    app1.extend_from_slice(&3u16.to_le_bytes());
    app1.extend_from_slice(&1u32.to_le_bytes());
    app1.extend_from_slice(&6u16.to_le_bytes());
    app1.extend_from_slice(&[0, 0]);
    app1.extend_from_slice(&0u32.to_le_bytes());

    let (px, w, h) = small_image(false);
    let rgb: Vec<u8> = px
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
    c.set_size(w as usize, h as usize);
    let mut c = c.start_compress(Vec::new()).unwrap();
    c.write_marker(mozjpeg::Marker::APP(1), &app1);
    c.write_scanlines(&rgb).unwrap();
    let jpg = c.finish().unwrap();

    let d = decode(&jpg).unwrap();
    assert_eq!(d.format, Format::Jpeg);
    assert_eq!(d.exif_orientation, 6);
}

#[test]
fn g_alpha() {
    let (px, w, h) = small_image(true);
    let check = |codec: Codec, tol: i16| {
        let out = encode(codec, &px, w, h, 90, None).unwrap();
        let d = decode(&out).unwrap();
        let worst = px
            .as_chunks::<4>()
            .0
            .iter()
            .zip(d.rgba.as_chunks::<4>().0.iter())
            .map(|(a, b)| (a[3] as i16 - b[3] as i16).abs())
            .max()
            .unwrap();
        assert!(worst <= tol, "{codec:?}: alpha off by {worst}");
    };
    check(Codec::WebpLossy, 8);
    check(Codec::Avif, 8);
    check(Codec::PngLossless, 0);
    assert!(matches!(
        encode(Codec::Jpeg, &px, w, h, 80, None),
        Err(CodecError::Unsupported("alpha"))
    ));
}

#[test]
fn h_errors() {
    assert!(matches!(decode(b"garbage"), Err(CodecError::UnknownFormat)));
}
