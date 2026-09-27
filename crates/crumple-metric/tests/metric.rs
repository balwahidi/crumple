use crumple_metric::{ssimulacra2, MetricError, Reference};
use ssimulacra2::{compute_frame_ssimulacra2, ColorPrimaries, Rgb, TransferCharacteristic};
use std::path::PathBuf;

fn corpus() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench/corpus/kodak");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "png"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn load_rgba(path: &PathBuf) -> (Vec<u8>, u32, u32) {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => unreachable!("EXPAND removes indexed"),
    };
    (rgba, w, h)
}

fn d1_blur(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = src.to_vec();
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut sum = 0u32;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let yy = (y as i64 + dy).clamp(0, h as i64 - 1) as usize;
                        let xx = (x as i64 + dx).clamp(0, w as i64 - 1) as usize;
                        sum += u32::from(src[(yy * w + xx) * 4 + c]);
                    }
                }
                out[(y * w + x) * 4 + c] = ((sum + 4) / 9) as u8;
            }
        }
    }
    out
}

fn d2_posterize(src: &[u8]) -> Vec<u8> {
    src.iter()
        .enumerate()
        .map(|(i, &v)| if i % 4 == 3 { v } else { v & 0xF8 })
        .collect()
}

fn d3_noise(src: &[u8]) -> Vec<u8> {
    src.iter()
        .enumerate()
        .map(|(i, &v)| {
            if i % 4 == 3 {
                return v;
            }
            let n = (((i as u64 * 1103515245 + 12345) >> 16) % 13) as i32 - 6;
            (i32::from(v) + n).clamp(0, 255) as u8
        })
        .collect()
}

fn d4_nearest(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (sw, sh) = (w.div_ceil(2), h.div_ceil(2));
    let small: Vec<[u8; 4]> = (0..sh * sw)
        .map(|i| {
            let (y, x) = (i / sw, i % sw);
            let o = ((y * 2) * w + x * 2) * 4;
            [src[o], src[o + 1], src[o + 2], src[o + 3]]
        })
        .collect();
    let mut out = src.to_vec();
    for y in 0..h {
        for x in 0..w {
            let p = small[(y / 2) * sw + x / 2];
            out[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&p[..3]);
        }
    }
    out
}

fn crate_rgb(rgba: &[u8], w: u32, h: u32) -> Rgb {
    let data = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            [
                f32::from(p[0]) / 255.0,
                f32::from(p[1]) / 255.0,
                f32::from(p[2]) / 255.0,
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

#[test]
fn parity_with_crate_on_kodak() {
    let files = corpus();
    if files.is_empty() {
        eprintln!("SKIP parity: bench/corpus/kodak/*.png not found");
        return;
    }
    let mut max_diff = 0f64;
    for path in &files {
        let (rgba, w, h) = load_rgba(path);
        let (wu, hu) = (w as usize, h as usize);
        let r = Reference::new(&rgba, w, h).unwrap();
        let dists = [
            ("D1", d1_blur(&rgba, wu, hu)),
            ("D2", d2_posterize(&rgba)),
            ("D3", d3_noise(&rgba)),
            ("D4", d4_nearest(&rgba, wu, hu)),
        ];
        for (name, d) in &dists {
            let ours = r.score(d).unwrap();
            let theirs =
                compute_frame_ssimulacra2(crate_rgb(&rgba, w, h), crate_rgb(d, w, h)).unwrap();
            let diff = (ours - theirs).abs();
            max_diff = max_diff.max(diff);
            assert!(
                diff <= 1e-4,
                "{} {name}: ours {ours} crate {theirs} diff {diff}",
                path.display()
            );
        }
    }
    eprintln!(
        "parity: {} files x 4 distortions, max |diff| = {max_diff:e}",
        files.len()
    );
}

fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8, 255]);
        }
    }
    v
}

#[test]
fn identical_scores_100() {
    let img = gradient(64, 64);
    let r = Reference::new(&img, 64, 64).unwrap();
    assert!(!r.has_alpha());
    let s = r.score(&img).unwrap();
    assert!(s >= 99.999, "{s}");
    if let Some(path) = corpus().first() {
        let (rgba, w, h) = load_rgba(path);
        let s = Reference::new(&rgba, w, h).unwrap().score(&rgba).unwrap();
        assert!(s >= 99.999, "{s}");
    }
}

#[test]
fn alpha_hidden_pixels_ignored() {
    let (w, h) = (64u32, 64u32);
    let mut img = gradient(w, h);
    for y in 0..h as usize {
        for x in 0..(w as usize / 2) {
            img[(y * w as usize + x) * 4 + 3] = 0;
        }
    }
    let r = Reference::new(&img, w, h).unwrap();
    assert!(r.has_alpha());
    let mut other = img.clone();
    let mut seed = 0x1234_5678u32;
    for y in 0..h as usize {
        for x in 0..(w as usize / 2) {
            for c in 0..3 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                other[(y * w as usize + x) * 4 + c] = (seed >> 24) as u8;
            }
        }
    }
    let s = r.score(&other).unwrap();
    assert!(s >= 99.999, "hidden change scored {s}");

    let mut changed = other.clone();
    let o = (10 * w as usize + 50) * 4;
    changed[o] = changed[o].wrapping_add(128);
    let s2 = r.score(&changed).unwrap();
    assert!(s2 < s, "visible change {s2} not below {s}");
}

#[test]
fn errors() {
    let img = gradient(16, 16);
    assert_eq!(
        Reference::new(&img[..img.len() - 1], 16, 16).err(),
        Some(MetricError::BadLength)
    );
    let small = vec![0u8; 7 * 7 * 4];
    assert_eq!(
        Reference::new(&small, 7, 7).err(),
        Some(MetricError::TooSmall)
    );
    let r = Reference::new(&img, 16, 16).unwrap();
    assert_eq!(r.width(), 16);
    assert_eq!(r.height(), 16);
    let other = gradient(16, 17);
    assert_eq!(r.score(&other), Err(MetricError::BadLength));
    assert_eq!(
        ssimulacra2(&img, &other, 16, 16),
        Err(MetricError::BadLength)
    );
}

// ---- [Reviewer] tests below: synthetic, so they also run in CI without the corpus. ----

/// Deterministic pseudo-random bytes (LCG), smoothed a little so the blur has structure.
fn synth(w: u32, h: u32, seed: u32, alpha: impl Fn(usize, usize) -> u8) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut s = seed;
    let mut next = || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 24) as u8
    };
    let mut v = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let base = ((x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8);
            v.extend_from_slice(&[
                base.0.wrapping_add(next() / 4),
                base.1.wrapping_add(next() / 4),
                next(),
                alpha(x, y),
            ]);
        }
    }
    v
}

fn crate_score(a: &[u8], b: &[u8], w: u32, h: u32) -> f64 {
    compute_frame_ssimulacra2(crate_rgb(a, w, h), crate_rgb(b, w, h)).unwrap()
}

/// Every scale-count boundary: sizes where the 2nd..6th downscale does or does not
/// happen, odd sizes (edge-clamped downscale) and very thin images.
#[test]
fn parity_odd_and_tiny_sizes() {
    let sizes = [
        (8, 8),
        (8, 9),
        (9, 8),
        (9, 9),
        (15, 15),
        (16, 16),
        (17, 17),
        (31, 33),
        (33, 31),
        (32, 32),
        (63, 65),
        (64, 64),
        (65, 63),
        (127, 9),
        (9, 127),
        (8, 300),
        (300, 8),
        (255, 257),
        (256, 256),
        (257, 255),
        (511, 13),
    ];
    let mut max_diff = 0f64;
    for (w, h) in sizes {
        let r = synth(w, h, w * 31 + h, |_, _| 255);
        let d = d3_noise(&d1_blur(&r, w as usize, h as usize));
        let ours = Reference::new(&r, w, h).unwrap().score(&d).unwrap();
        let theirs = crate_score(&r, &d, w, h);
        let diff = (ours - theirs).abs();
        max_diff = max_diff.max(diff);
        assert!(diff <= 1e-9, "{w}x{h}: ours {ours} crate {theirs}");
        assert!(ours.is_finite(), "{w}x{h}: {ours}");
    }
    eprintln!("odd/tiny sizes: max |diff| = {max_diff:e}");
    // Sizes whose byte count overflows u64 (2^31 * 2^31 * 4 = 2^64) must not wrap to 0.
    for (w, h) in [(1u32 << 31, 1u32 << 31), (u32::MAX, u32::MAX)] {
        assert_eq!(
            Reference::new(&[], w, h).err(),
            Some(MetricError::BadLength),
            "{w}x{h}"
        );
    }
    for (w, h) in [(7, 8), (8, 7), (7, 100), (100, 7), (0, 0), (0, 10)] {
        let img = vec![0u8; w * h * 4];
        assert_eq!(
            Reference::new(&img, w as u32, h as u32).err(),
            Some(MetricError::TooSmall),
            "{w}x{h}"
        );
    }
}

/// Straight-alpha composite, integer reference implementation of the spec's
/// `c*a/255 + bg*(1 - a/255)`, rounded to nearest.
fn composite(rgba: &[u8], bg: u8) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = u32::from(p[3]);
            let mix = |c: u8| ((u32::from(c) * a + u32::from(bg) * (255 - a) + 127) / 255) as u8;
            [mix(p[0]), mix(p[1]), mix(p[2]), 255]
        })
        .collect()
}

/// With alpha, the score must equal min(crate over black, crate over white), where the
/// distorted image is composited with its OWN alpha.
#[test]
fn alpha_parity_with_crate_on_composites() {
    let (w, h) = (96u32, 80u32);
    let r = synth(w, h, 7, |x, y| ((x * 3 + y * 5) % 256) as u8);
    // Distorted: different RGB and different alpha.
    let mut d = d3_noise(&r);
    for (i, px) in d.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        px[3] = px[3].saturating_add((i % 7) as u8 * 3);
    }
    let reference = Reference::new(&r, w, h).unwrap();
    assert!(reference.has_alpha());
    let black = crate_score(&composite(&r, 0), &composite(&d, 0), w, h);
    let white = crate_score(&composite(&r, 255), &composite(&d, 255), w, h);
    let ours = reference.score(&d).unwrap();
    assert!(
        (ours - black.min(white)).abs() <= 1e-9,
        "ours {ours} black {black} white {white}"
    );

    // Changing only the distorted alpha must lower the score (own alpha is used).
    let mut alpha_only = r.clone();
    for px in alpha_only.as_chunks_mut::<4>().0.iter_mut() {
        px[3] /= 2;
    }
    let s = reference.score(&alpha_only).unwrap();
    assert!(s < 90.0, "alpha-only change scored {s}");
}

/// Opaque reference: the distorted image's alpha bytes are ignored.
#[test]
fn opaque_reference_ignores_distorted_alpha() {
    let (w, h) = (40u32, 24u32);
    let r = synth(w, h, 3, |_, _| 255);
    let d = d2_posterize(&r);
    let mut d_alpha = d.clone();
    for (i, px) in d_alpha.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        px[3] = (i % 256) as u8;
    }
    let reference = Reference::new(&r, w, h).unwrap();
    assert!(!reference.has_alpha());
    assert_eq!(
        reference.score(&d).unwrap().to_bits(),
        reference.score(&d_alpha).unwrap().to_bits()
    );
}
