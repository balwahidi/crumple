//! [Reviewer] Decoder conversions and metadata, encoder determinism, and the JPEG ICC layout.
#![cfg(all(feature = "jpeg", feature = "webp", feature = "avif", feature = "png"))]

use crumple_codecs::{decode, encode, Codec, Format};

// ---------------------------------------------------------------- PNG conversions

/// Packs samples (already in the image's bit depth) into PNG scanlines.
fn pack(samples: &[u16], depth: u8, w: usize, h: usize, spp: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in samples.chunks(w * spp).take(h) {
        if depth >= 8 {
            for &s in row {
                if depth == 16 {
                    out.extend_from_slice(&s.to_be_bytes());
                } else {
                    out.push(s as u8);
                }
            }
        } else {
            let (mut acc, mut bits) = (0u16, 0u8);
            for &s in row {
                acc = (acc << depth) | s;
                bits += depth;
                if bits == 8 {
                    out.push(acc as u8);
                    (acc, bits) = (0, 0);
                }
            }
            if bits > 0 {
                out.push((acc << (8 - bits)) as u8);
            }
        }
    }
    out
}

/// A sample at `depth` scaled to 8 bits the way the decoder must: bit replication for
/// low depths, the high byte for 16 bits (`STRIP_16`).
fn to8(v: u16, depth: u8) -> u8 {
    match depth {
        16 => (v >> 8) as u8,
        8 => v as u8,
        d => (u32::from(v) * 255 / ((1u32 << d) - 1)) as u8,
    }
}

fn write_png(
    w: u32,
    h: u32,
    color: png::ColorType,
    depth: u8,
    data: &[u8],
    palette: Option<Vec<u8>>,
    trns: Option<Vec<u8>>,
) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(color);
        e.set_depth(png::BitDepth::from_u8(depth).unwrap());
        if let Some(p) = palette {
            e.set_palette(p);
        }
        if let Some(t) = trns {
            e.set_trns(t);
        }
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(data).unwrap();
        wr.finish().unwrap();
    }
    out
}

#[test]
fn png_every_color_type_and_depth_expands_to_rgba8() {
    use png::ColorType as C;
    let (w, h) = (13usize, 5usize); // odd width: exercises row padding of packed depths
    let mut cases = 0;
    for (color, depths, spp) in [
        (C::Grayscale, &[1u8, 2, 4, 8, 16][..], 1usize),
        (C::GrayscaleAlpha, &[8, 16][..], 2),
        (C::Rgb, &[8, 16][..], 3),
        (C::Rgba, &[8, 16][..], 4),
        (C::Indexed, &[1, 2, 4, 8][..], 1),
    ] {
        for &depth in depths {
            let max = if depth == 16 {
                65535u32
            } else {
                (1u32 << depth) - 1
            };
            let samples: Vec<u16> = (0..w * h * spp)
                .map(|i| (((i as u32).wrapping_mul(2_654_435_761) >> 7) % (max + 1)) as u16)
                .collect();
            let data = pack(&samples, depth, w, h, spp);
            let has_trns_variant = !matches!(color, C::GrayscaleAlpha | C::Rgba);
            for with_trns in [false, true] {
                if with_trns && !has_trns_variant {
                    continue;
                }
                // Palette: entry i = (i, 255 - i, i*7) mod 256; tRNS covers the first 3 entries.
                let entries = 1usize << depth.min(8);
                let palette: Vec<u8> = (0..entries)
                    .flat_map(|i| [i as u8, 255 - i as u8, (i * 7) as u8])
                    .collect();
                let pal_alpha = [0u8, 77, 200];
                // tRNS key: the first pixel's value, so at least one pixel matches.
                let key: Vec<u16> = samples[..spp].to_vec();
                let trns = with_trns.then(|| match color {
                    C::Indexed => pal_alpha[..entries.min(3)].to_vec(),
                    _ => key.iter().flat_map(|k| k.to_be_bytes()).collect(),
                });
                let png = write_png(
                    w as u32,
                    h as u32,
                    color,
                    depth,
                    &data,
                    (color == C::Indexed).then(|| palette.clone()),
                    trns,
                );
                let expected: Vec<u8> = samples
                    .chunks(spp)
                    .flat_map(|s| {
                        let keyed = with_trns && s == &key[..];
                        match color {
                            C::Grayscale => {
                                let g = to8(s[0], depth);
                                [g, g, g, if keyed { 0 } else { 255 }]
                            }
                            C::GrayscaleAlpha => {
                                let g = to8(s[0], depth);
                                [g, g, g, to8(s[1], depth)]
                            }
                            C::Rgb => [
                                to8(s[0], depth),
                                to8(s[1], depth),
                                to8(s[2], depth),
                                if keyed { 0 } else { 255 },
                            ],
                            C::Rgba => [
                                to8(s[0], depth),
                                to8(s[1], depth),
                                to8(s[2], depth),
                                to8(s[3], depth),
                            ],
                            C::Indexed => {
                                let i = s[0] as usize;
                                let a = if with_trns {
                                    pal_alpha.get(i).copied().unwrap_or(255)
                                } else {
                                    255
                                };
                                [palette[i * 3], palette[i * 3 + 1], palette[i * 3 + 2], a]
                            }
                        }
                    })
                    .collect();
                let d = decode(&png).unwrap();
                assert_eq!((d.width, d.height), (w as u32, h as u32));
                assert!(
                    d.rgba == expected,
                    "{color:?} {depth}-bit trns={with_trns}: first pixels {:?} vs {:?}",
                    &d.rgba[..16],
                    &expected[..16]
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 26);
}

// ---------------------------------------------------------------- JPEG color models

fn mozjpeg(
    cs: mozjpeg::ColorSpace,
    jpeg_cs: Option<mozjpeg::ColorSpace>,
    data: &[u8],
    w: u32,
    h: u32,
) -> Vec<u8> {
    let mut c = mozjpeg::Compress::new(cs);
    if let Some(j) = jpeg_cs {
        c.set_color_space(j);
    }
    c.set_size(w as usize, h as usize);
    c.set_quality(95.0);
    let mut c = c.start_compress(Vec::new()).unwrap();
    c.write_scanlines(data).unwrap();
    c.finish().unwrap()
}

fn smooth(w: u32, h: u32) -> Vec<[u8; 3]> {
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8]
        })
        .collect()
}

fn max_err(got: &[u8], want: impl Iterator<Item = [u8; 3]>) -> i32 {
    got.as_chunks::<4>()
        .0
        .iter()
        .zip(want)
        .map(|(g, w)| {
            assert_eq!(g[3], 255);
            (0..3)
                .map(|c| (i32::from(g[c]) - i32::from(w[c])).abs())
                .max()
                .unwrap()
        })
        .max()
        .unwrap()
}

#[test]
fn grayscale_and_cmyk_jpegs_decode_to_rgb() {
    use mozjpeg::ColorSpace as J;
    let (w, h) = (48u32, 32u32);
    let px = smooth(w, h);

    let gray: Vec<u8> = px.iter().map(|p| p[1]).collect();
    let d = decode(&mozjpeg(J::JCS_GRAYSCALE, None, &gray, w, h)).unwrap();
    assert!(d
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| p[0] == p[1] && p[1] == p[2]));
    let e = max_err(&d.rgba, gray.iter().map(|&g| [g, g, g]));
    assert!(e <= 4, "gray error {e}");

    // Adobe convention (what browsers do): stored CMYK is inverted, RGB = C*K/255.
    let k = 180u8;
    let cmyk: Vec<u8> = px.iter().flat_map(|p| [p[0], p[1], p[2], k]).collect();
    let want = || {
        px.iter()
            .map(move |p| p.map(|c| ((u32::from(c) * u32::from(k) + 127) / 255) as u8))
    };
    for (name, jcs) in [("CMYK", None), ("YCCK", Some(J::JCS_YCCK))] {
        let d = decode(&mozjpeg(J::JCS_CMYK, jcs, &cmyk, w, h)).unwrap();
        assert_eq!(d.rgba.len(), (w * h * 4) as usize);
        let e = max_err(&d.rgba, want());
        assert!(e <= 6, "{name} error {e}");
    }
}

// ---------------------------------------------------------------- ICC

/// A 3144-byte blob with a real ICC header colour space at bytes 16..20.
fn icc_with_space(space: &[u8; 4]) -> Vec<u8> {
    let mut b: Vec<u8> = (0..3144).map(|i| (i * 7 % 251) as u8).collect();
    b[16..20].copy_from_slice(space);
    b
}

#[test]
fn non_rgb_icc_profiles_are_dropped() {
    let (w, h) = (16u32, 16u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 9, 99, 255]).collect();
    for codec in [Codec::Jpeg, Codec::WebpLossy, Codec::PngLossless] {
        let rgb = icc_with_space(b"RGB ");
        let out = encode(codec, &px, w, h, 80, Some(&rgb)).unwrap();
        assert_eq!(decode(&out).unwrap().icc, Some(rgb), "{codec:?}");
        for space in [b"GRAY", b"CMYK"] {
            let out = encode(codec, &px, w, h, 80, Some(&icc_with_space(space))).unwrap();
            assert_eq!(decode(&out).unwrap().icc, None, "{codec:?} {space:?}");
        }
        // An empty profile is no profile.
        let out = encode(codec, &px, w, h, 80, Some(&[])).unwrap();
        assert_eq!(decode(&out).unwrap().icc, None, "{codec:?} empty");
    }
}

/// APP2 ICC_PROFILE markers, in file order: (sequence number, count, data length).
fn icc_markers(jpeg: &[u8]) -> Vec<(u8, u8, usize)> {
    let mut out = Vec::new();
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF && jpeg[i + 1] != 0xDA {
        let len = usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
        let payload = &jpeg[i + 4..i + 2 + len];
        if jpeg[i + 1] == 0xE2 && payload.starts_with(b"ICC_PROFILE\0") {
            out.push((payload[12], payload[13], payload.len() - 14));
        }
        i += 2 + len;
    }
    out
}

#[test]
fn jpeg_icc_uses_spec_app2_layout() {
    let (w, h) = (16u32, 16u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 9, 99, 255]).collect();
    for (len, want) in [
        (3144usize, vec![(1u8, 1u8, 3144usize)]),
        (65_519, vec![(1, 1, 65_519)]),
        (65_520, vec![(1, 2, 65_519), (2, 2, 1)]),
        (
            150_000,
            vec![(1, 3, 65_519), (2, 3, 65_519), (3, 3, 18_962)],
        ),
    ] {
        let blob: Vec<u8> = (0..len).map(|i| (i * 31 % 253) as u8).collect();
        let jpg = encode(Codec::Jpeg, &px, w, h, 80, Some(&blob)).unwrap();
        assert_eq!(icc_markers(&jpg), want, "{len} bytes");
        assert_eq!(decode(&jpg).unwrap().icc, Some(blob), "{len} bytes");
    }
}

// ---------------------------------------------------------------- EXIF orientation

type U16Bytes = fn(u16) -> [u8; 2];
type U32Bytes = fn(u32) -> [u8; 4];

fn exif_tiff(o: u16, big_endian: bool) -> Vec<u8> {
    let (u16b, u32b): (U16Bytes, U32Bytes) = if big_endian {
        (u16::to_be_bytes, u32::to_be_bytes)
    } else {
        (u16::to_le_bytes, u32::to_le_bytes)
    };
    let mut t = if big_endian {
        b"MM\0*".to_vec()
    } else {
        b"II*\0".to_vec()
    };
    t.extend_from_slice(&u32b(8));
    t.extend_from_slice(&u16b(1));
    t.extend_from_slice(&u16b(0x0112));
    t.extend_from_slice(&u16b(3));
    t.extend_from_slice(&u32b(1));
    t.extend_from_slice(&u16b(o));
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&u32b(0));
    t
}

fn png_with_chunk_after_ihdr(png: &[u8], kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    let mut crc = !0u32;
    for &b in &c[4..] {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    c.extend_from_slice(&(!crc).to_be_bytes());
    let mut out = png[..33].to_vec();
    out.extend(c);
    out.extend_from_slice(&png[33..]);
    out
}

#[test]
fn exif_orientation_from_all_three_containers() {
    let (w, h) = (16u32, 8u32);
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 50, 200]).collect();
    for o in 0u16..=9 {
        let want = if (1..=8).contains(&o) { o as u8 } else { 1 };
        for be in [false, true] {
            let tiff = exif_tiff(o, be);

            let mut app1 = b"Exif\0\0".to_vec();
            app1.extend_from_slice(&tiff);
            let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
            c.set_size(w as usize, h as usize);
            let mut c = c.start_compress(Vec::new()).unwrap();
            c.write_marker(mozjpeg::Marker::APP(1), &app1);
            c.write_scanlines(&rgb).unwrap();
            let jpg = c.finish().unwrap();

            let png = write_png(w, h, png::ColorType::Rgb, 8, &rgb, None, None);
            let png = png_with_chunk_after_ihdr(&png, b"eXIf", &tiff);

            let mut webp = Vec::new();
            let mut enc = image_webp::WebPEncoder::new(&mut webp);
            enc.set_exif_metadata(tiff.clone());
            enc.encode(&rgb, w, h, image_webp::ColorType::Rgb8).unwrap();

            for (name, bytes) in [("jpeg", jpg), ("png", png), ("webp", webp)] {
                let d = decode(&bytes).unwrap();
                assert_eq!(d.exif_orientation, want, "{name} orientation {o} be={be}");
            }
        }
    }
}

// ---------------------------------------------------------------- animation

fn u24(v: u32) -> [u8; 3] {
    let b = v.to_le_bytes();
    [b[0], b[1], b[2]]
}

/// A two-frame animated WebP assembled by hand from image-webp's VP8L still.
fn animated_webp(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut still = Vec::new();
    image_webp::WebPEncoder::new(&mut still)
        .encode(rgba, w, h, image_webp::ColorType::Rgba8)
        .unwrap();
    assert_eq!(&still[12..16], b"VP8L");
    let vp8l = &still[12..];
    let mut anmf = Vec::new();
    anmf.extend_from_slice(&[0; 6]);
    anmf.extend_from_slice(&u24(w - 1));
    anmf.extend_from_slice(&u24(h - 1));
    anmf.extend_from_slice(&u24(100));
    // Flags: do not blend (image-webp's alpha blend is off by one even at alpha 255).
    anmf.push(0x02);
    anmf.extend_from_slice(vp8l);
    let mut body = b"VP8X".to_vec();
    body.extend_from_slice(&10u32.to_le_bytes());
    body.extend_from_slice(&[0x12, 0, 0, 0]);
    body.extend_from_slice(&u24(w - 1));
    body.extend_from_slice(&u24(h - 1));
    body.extend_from_slice(b"ANIM");
    body.extend_from_slice(&6u32.to_le_bytes());
    body.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    for _ in 0..2 {
        body.extend_from_slice(b"ANMF");
        body.extend_from_slice(&(anmf.len() as u32).to_le_bytes());
        body.extend_from_slice(&anmf);
        if anmf.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend(body);
    out
}

#[test]
fn animation_is_detected() {
    let (w, h) = (8u32, 6u32);
    let rgba: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 3, 7, 255]).collect();

    let still = encode(Codec::PngLossless, &rgba, w, h, 0, None).unwrap();
    assert!(!decode(&still).unwrap().animated);
    let mut apng = Vec::new();
    {
        let mut e = png::Encoder::new(&mut apng, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_animated(2, 0).unwrap();
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(&rgba).unwrap();
        wr.write_image_data(&rgba).unwrap();
        wr.finish().unwrap();
    }
    let d = decode(&apng).unwrap();
    assert!(d.animated && d.format == Format::Png && d.rgba == rgba);

    let still = encode(Codec::WebpLossy, &rgba, w, h, 80, None).unwrap();
    assert!(!decode(&still).unwrap().animated);
    let d = decode(&animated_webp(&rgba, w, h)).unwrap();
    assert_eq!((d.animated, d.format), (true, Format::Webp));
    assert_eq!(&d.rgba[..16], &rgba[..16]);
    assert!(d.rgba == rgba);
}

// ---------------------------------------------------------------- AVIF depth

#[test]
fn avif_ten_bit_output_is_rounded_to_eight_bits() {
    use avif_decode::Image;
    let (w, h) = (24u32, 16u32);
    let to8 = |v: u16| ((u32::from(v) * 255 + 32_767) / 65_535) as u8;
    for alpha in [false, true] {
        let px: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                [
                    (i * 5) as u8,
                    (i * 3) as u8,
                    128,
                    if alpha { (i * 2) as u8 } else { 255 },
                ]
            })
            .collect();
        let avif = encode(Codec::Avif, &px, w, h, 80, None).unwrap();
        let ours = decode(&avif).unwrap().rgba;
        let raw = avif_decode::Decoder::from_avif(&avif)
            .unwrap()
            .to_image()
            .unwrap();
        let want: Vec<u8> = match raw {
            Image::Rgb16(i) if !alpha => i
                .pixels()
                .flat_map(|p| [to8(p.r), to8(p.g), to8(p.b), 255])
                .collect(),
            Image::Rgba16(i) if alpha => i
                .pixels()
                .flat_map(|p| [to8(p.r), to8(p.g), to8(p.b), to8(p.a)])
                .collect(),
            _ => panic!("ravif output is expected to be 10-bit (alpha={alpha})"),
        };
        assert!(ours == want, "alpha={alpha}");
    }
}

// ---------------------------------------------------------------- determinism

#[test]
fn every_encoder_is_deterministic_across_threads() {
    let (w, h) = (96u32, 64u32);
    let opaque: Vec<u8> = (0..w * h)
        .flat_map(|i| [(i * 7) as u8, (i / w * 3) as u8, (i % w * 2) as u8, 255])
        .collect();
    let alpha: Vec<u8> = opaque
        .chunks(4)
        .enumerate()
        .flat_map(|(i, p)| [p[0], p[1], p[2], (i % 251) as u8])
        .collect();
    let icc = icc_with_space(b"RGB ");
    type Job<'a> = (Codec, &'a [u8], Option<&'a [u8]>);
    let jobs: Vec<Job> = vec![
        (Codec::Jpeg, &opaque, None),
        (Codec::Jpeg, &opaque, Some(&icc)),
        (Codec::WebpLossy, &opaque, None),
        (Codec::WebpLossy, &alpha, Some(&icc)),
        (Codec::Avif, &opaque, None),
        (Codec::Avif, &alpha, None),
        (Codec::PngLossless, &opaque, None),
        (Codec::PngLossless, &alpha, Some(&icc)),
    ];
    let run = |(c, px, icc): &Job| encode(*c, px, w, h, 55, *icc).unwrap();
    let first: Vec<Vec<u8>> = jobs.iter().map(run).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .flat_map(|_| jobs.iter().map(|j| s.spawn(move || run(j))))
            .collect();
        for (k, hdl) in handles.into_iter().enumerate() {
            let i = k % jobs.len();
            assert!(
                hdl.join().unwrap() == first[i],
                "{:?} job {i} differs",
                jobs[i].0
            );
        }
    });
}
