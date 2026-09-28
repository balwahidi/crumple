//! Native per-image pipeline (ARCHITECTURE §3): decode, orient, resize, search each
//! lossy codec with the planner, add the lossless PNG candidate, choose the smallest.

use std::path::Path;

use crumple_codecs::{self as codecs, CodecError, Format as InFormat};
use crumple_core::{
    apply_orientation, choose, fit_within, prior_q, Candidate, CodecKind, Outcome, Planner,
    SearchConfig, Step,
};
use crumple_metric::Reference;

use crate::report::Evals;
use crate::{Format, Formats, Settings};

/// mozjpeg refuses images wider or taller than this.
const JPEG_MAX_SIDE: u32 = 65_500;
/// libwebp refuses images wider or taller than this (`WEBP_MAX_DIMENSION`).
const WEBP_MAX_SIDE: u32 = 16_383;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Optimized,
    KeptOriginal,
    Skipped,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Codec {
    Jpeg,
    Webp,
    Avif,
    Png,
    Original,
}

impl Codec {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Codec::Jpeg => "jpeg",
            Codec::Webp => "webp",
            Codec::Avif => "avif",
            Codec::Png => "png",
            Codec::Original => "original",
        }
    }

    /// Output extension, or `None` for a kept original (keeps its own).
    pub(crate) fn ext(self) -> Option<&'static str> {
        match self {
            Codec::Original => None,
            c => Some(codecs::extension(c.native()?)),
        }
    }

    fn native(self) -> Option<codecs::Codec> {
        Some(match self {
            Codec::Jpeg => codecs::Codec::Jpeg,
            Codec::Webp => codecs::Codec::WebpLossy,
            Codec::Avif => codecs::Codec::Avif,
            Codec::Png => codecs::Codec::PngLossless,
            Codec::Original => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ImageOutcome {
    pub status: Status,
    pub reason: Option<String>,
    pub codec: Option<Codec>,
    pub quality: Option<u8>,
    pub score: Option<f64>,
    pub evals: Option<Evals>,
    /// Encoded output bytes; `None` with `Codec::Original` means copy the input.
    pub data: Option<Vec<u8>>,
}

impl ImageOutcome {
    fn bare(status: Status, reason: impl Into<String>) -> Self {
        ImageOutcome {
            status,
            reason: Some(reason.into()),
            codec: None,
            quality: None,
            score: None,
            evals: None,
            data: None,
        }
    }
}

/// Bytes the memory gate charges per pixel, decided from the input's first bytes before decoding.
///
/// Whole-pipeline peaks measured on 12 MP images (§5): 134 B/px for an opaque JPEG, 135 for an
/// opaque PNG and 184 for a PNG with alpha (the metric caches a second reference for alpha).
/// Alpha is unknown before decode, so every format that can carry alpha is charged the alpha
/// figure, and both charges keep headroom for per-thread overhead on small images.
pub(crate) fn bytes_per_pixel(bytes: &[u8]) -> u64 {
    match codecs::sniff(bytes) {
        Some(InFormat::Jpeg) => 200,
        _ => 260,
    }
}

/// Number of components in the first SOFn frame header, or `None` if there is none
/// before the first scan. 4 means CMYK or YCCK.
pub(crate) fn jpeg_components(bytes: &[u8]) -> Option<u8> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2;
    loop {
        // Skip fill bytes before a marker.
        while *bytes.get(i)? == 0xFF && *bytes.get(i + 1)? == 0xFF {
            i += 1;
        }
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(i + 1)?;
        match marker {
            // Standalone markers without a length.
            0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = u16::from_be_bytes([*bytes.get(i + 2)?, *bytes.get(i + 3)?]) as usize;
        let is_sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            return bytes.get(i + 9).copied();
        }
        if len < 2 {
            return None;
        }
        i += 2 + len;
    }
}

fn allowed(settings: &Settings, input: InFormat, f: Format) -> bool {
    match &settings.formats {
        Formats::List(l) => l.contains(&f),
        Formats::Same => matches!(
            (input, f),
            (InFormat::Jpeg, Format::Jpeg)
                | (InFormat::Png, Format::Png)
                | (InFormat::Webp, Format::Webp)
        ),
    }
}

pub(crate) fn optimize_one(_input: &Path, bytes: &[u8], settings: &Settings) -> ImageOutcome {
    let Some(format) = codecs::sniff(bytes) else {
        return ImageOutcome::bare(Status::Skipped, "unsupported");
    };
    // rav1d aborts the process on some corrupt AVIF data, so AVIF input is never decoded.
    if format == InFormat::Avif {
        return ImageOutcome::bare(Status::Skipped, "avif-input");
    }
    if format == InFormat::Jpeg && jpeg_components(bytes) == Some(4) {
        return ImageOutcome::bare(Status::Skipped, "cmyk-unsupported");
    }
    let decoded = match codecs::decode_with_limit(bytes, settings.max_pixels) {
        Ok(d) => d,
        Err(CodecError::TooLarge { .. }) => {
            return ImageOutcome::bare(Status::Skipped, "too-large")
        }
        Err(e) => return ImageOutcome::bare(Status::Error, e.to_string()),
    };
    if decoded.animated {
        return ImageOutcome::bare(Status::Skipped, "animated");
    }
    // Belt and braces: images whose size `imagesize` could not read bypassed the gate.
    if u64::from(decoded.width) * u64::from(decoded.height) > settings.max_pixels {
        return ImageOutcome::bare(Status::Skipped, "too-large");
    }
    let icc = decoded.icc.clone();
    let (rgba, w, h) = apply_orientation(
        &decoded.rgba,
        decoded.width,
        decoded.height,
        decoded.exif_orientation,
    );
    drop(decoded);
    let resized =
        settings.max_width.is_some_and(|m| w > m) || settings.max_height.is_some_and(|m| h > m);
    let (rgba, w, h) = if resized {
        fit_within(&rgba, w, h, settings.max_width, settings.max_height)
    } else {
        (rgba, w, h)
    };
    match run_candidates(
        &rgba,
        w,
        h,
        icc.as_deref(),
        format,
        bytes.len() as u64,
        resized,
        settings,
    ) {
        Ok(o) => o,
        Err(e) => ImageOutcome::bare(Status::Error, e),
    }
}

struct Found {
    codec: Codec,
    q: Option<u8>,
    score: f64,
    data: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn run_candidates(
    rgba: &[u8],
    w: u32,
    h: u32,
    icc: Option<&[u8]>,
    format: InFormat,
    original_len: u64,
    resized: bool,
    settings: &Settings,
) -> Result<ImageOutcome, String> {
    let target = settings.target;
    // A resized image cannot fall back to the original, so nothing bounds the search.
    let mut size_bound = (!resized).then_some(original_len);
    let mut evals = Evals {
        jpeg: 0,
        webp: 0,
        avif: 0,
    };
    let mut found: Vec<Found> = Vec::new();
    let mut note = None;

    // Images below 8x8 cannot be scored; only the lossless candidate applies.
    let reference = Reference::new(rgba, w, h).ok();
    if let Some(reference) = &reference {
        let alpha = reference.has_alpha();
        let lossy = [
            (Codec::Jpeg, Format::Jpeg, CodecKind::Jpeg),
            (Codec::Webp, Format::Webp, CodecKind::Webp),
            (Codec::Avif, Format::Avif, CodecKind::Avif),
        ];
        for (codec, fmt, kind) in lossy {
            if !allowed(settings, format, fmt) {
                continue;
            }
            if codec == Codec::Jpeg && (alpha || w > JPEG_MAX_SIDE || h > JPEG_MAX_SIDE) {
                continue;
            }
            if codec == Codec::Webp && (w > WEBP_MAX_SIDE || h > WEBP_MAX_SIDE) {
                continue;
            }
            // ravif 0.13 has no ICC API.
            if codec == Codec::Avif && icc.is_some() {
                continue;
            }
            let (outcome, data) =
                search_codec(codec, kind, reference, rgba, w, h, icc, target, size_bound)?;
            let n = match outcome {
                Outcome::Found { evals, .. }
                | Outcome::Unreachable { evals, .. }
                | Outcome::Pruned { evals } => evals,
            };
            match codec {
                Codec::Jpeg => evals.jpeg = n,
                Codec::Webp => evals.webp = n,
                _ => evals.avif = n,
            }
            if let (
                Outcome::Found {
                    q, bytes, score, ..
                },
                Some(data),
            ) = (outcome, data)
            {
                size_bound = Some(size_bound.map_or(bytes, |b| b.min(bytes)));
                found.push(Found {
                    codec,
                    q: Some(q),
                    score,
                    data,
                });
            }
        }
    } else {
        note = Some("too-small-to-score".to_string());
    }

    if format == InFormat::Png && allowed(settings, format, Format::Png) {
        let data = codecs::encode(codecs::Codec::PngLossless, rgba, w, h, 100, icc)
            .map_err(|e| format!("png: {e}"))?;
        found.push(Found {
            codec: Codec::Png,
            q: None,
            score: 100.0,
            data,
        });
    }

    let candidates: Vec<Candidate> = found
        .iter()
        .map(|f| Candidate {
            id: f.codec as u8,
            bytes: f.data.len() as u64,
            passes: true,
        })
        .collect();
    let bound = if resized { u64::MAX } else { original_len };
    let evals = Some(evals);
    Ok(match choose(bound, &candidates) {
        Some(i) => {
            let f = found.swap_remove(i);
            ImageOutcome {
                status: Status::Optimized,
                reason: None,
                codec: Some(f.codec),
                quality: f.q,
                score: Some(f.score),
                evals,
                data: Some(f.data),
            }
        }
        // The original is not an answer to --max-width/--max-height: writing it would ignore
        // the resize the user asked for, so a resized image with no candidate is an error.
        None if resized => ImageOutcome {
            evals,
            ..ImageOutcome::bare(
                Status::Error,
                note.unwrap_or_else(|| "no-candidate-after-resize".to_string()),
            )
        },
        None => ImageOutcome {
            status: Status::KeptOriginal,
            reason: note,
            codec: Some(Codec::Original),
            quality: None,
            score: None,
            evals,
            data: None,
        },
    })
}

/// Runs the §4.1 planner for one codec. Returns the outcome and, when `Found`, the exact
/// bytes that were scored (invariant 1: never re-encode after the search).
#[allow(clippy::too_many_arguments)]
fn search_codec(
    codec: Codec,
    kind: CodecKind,
    reference: &Reference,
    rgba: &[u8],
    w: u32,
    h: u32,
    icc: Option<&[u8]>,
    target: f64,
    size_bound: Option<u64>,
) -> Result<(Outcome, Option<Vec<u8>>), String> {
    let native = codec.native().expect("lossy codec");
    let mut planner = Planner::new(SearchConfig::new(target, prior_q(kind, target)), size_bound);
    let mut pending: Option<(u8, Vec<u8>)> = None;
    // The smallest passing encode so far (ties to the lower q), matching `Outcome::Found`.
    let mut best: Option<(u8, Vec<u8>)> = None;
    loop {
        match planner.next() {
            Step::Encode(q) => {
                let data = codecs::encode(native, rgba, w, h, q, icc)
                    .map_err(|e| format!("{}: {e}", codec.name()))?;
                planner.on_encoded(q, data.len() as u64);
                pending = Some((q, data));
            }
            Step::Score(q) => {
                let (pq, data) = pending.take().expect("score follows its encode");
                debug_assert_eq!(pq, q);
                let d = codecs::decode(&data).map_err(|e| format!("{}: {e}", codec.name()))?;
                if d.width != w || d.height != h {
                    return Err(format!("{}: candidate size changed", codec.name()));
                }
                let score = reference
                    .score(&d.rgba)
                    .map_err(|e| format!("{}: score: {e:?}", codec.name()))?;
                planner.on_scored(q, score);
                if score >= target {
                    let better = best.as_ref().is_none_or(|(bq, b)| {
                        data.len() < b.len() || (data.len() == b.len() && q < *bq)
                    });
                    if better {
                        best = Some((q, data));
                    }
                }
            }
            Step::Finished(o) => {
                let data = match (o, best) {
                    (Outcome::Found { q, .. }, Some((bq, data))) if bq == q => Some(data),
                    (Outcome::Found { .. }, _) => {
                        return Err(format!("{}: planner and driver disagree", codec.name()))
                    }
                    _ => None,
                };
                return Ok((o, data));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(target: f64) -> Settings {
        Settings {
            target,
            formats: Formats::List(vec![Format::Jpeg, Format::Webp, Format::Avif, Format::Png]),
            max_width: None,
            max_height: None,
            max_pixels: 24_000_000,
        }
    }

    /// A smooth synthetic photo-like RGBA image.
    fn image(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let n = ((x * 7919 + y * 104_729) % 17) as u8;
                v.extend_from_slice(&[
                    (x * 255 / w) as u8 ^ n,
                    (y * 255 / h) as u8,
                    ((x + y) * 3) as u8,
                    if alpha && x < w / 3 { 128 } else { 255 },
                ]);
            }
        }
        v
    }

    fn png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        // Uncompressed-ish store via the lossless codec, then pad by re-encoding RGBA
        // through a cheap path would still be small; oxipng output is the fairest input.
        codecs::encode(
            codecs::Codec::PngLossless,
            &image(w, h, alpha),
            w,
            h,
            100,
            None,
        )
        .unwrap()
    }

    /// A baseline JPEG header with `nf` components (only the markers the parser reads).
    fn jpeg_header(nf: u8) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]); // APP0, 2 payload bytes
        v.extend_from_slice(&[0xFF, 0xFF]); // fill byte
        v.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x02]); // empty DHT (not a SOF)
        let len = 8 + 3 * nf as u16;
        v.extend_from_slice(&[0xFF, 0xC2]);
        v.extend_from_slice(&len.to_be_bytes());
        v.extend_from_slice(&[8, 0, 16, 0, 16, nf]);
        for c in 0..nf {
            v.extend_from_slice(&[c + 1, 0x11, 0]);
        }
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]);
        v
    }

    #[test]
    fn jpeg_component_count() {
        assert_eq!(jpeg_components(&jpeg_header(1)), Some(1));
        assert_eq!(jpeg_components(&jpeg_header(3)), Some(3));
        assert_eq!(jpeg_components(&jpeg_header(4)), Some(4));
        assert_eq!(jpeg_components(&[0xFF, 0xD8, 0xFF, 0xDA]), None);
        assert_eq!(jpeg_components(&jpeg_header(4)[..12]), None);
        assert_eq!(jpeg_components(b"garbage"), None);
        // A real mozjpeg file is 3-component.
        let rgba = image(32, 32, false);
        let j = codecs::encode(codecs::Codec::Jpeg, &rgba, 32, 32, 80, None).unwrap();
        assert_eq!(jpeg_components(&j), Some(3));
    }

    #[test]
    fn cmyk_jpeg_is_skipped_before_decode() {
        let o = optimize_one(Path::new("x.jpg"), &jpeg_header(4), &settings(80.0));
        assert_eq!(o.status, Status::Skipped);
        assert_eq!(o.reason.as_deref(), Some("cmyk-unsupported"));
    }

    #[test]
    fn avif_input_is_skipped_by_content_not_name() {
        let rgba = image(32, 32, false);
        let avif = codecs::encode(codecs::Codec::Avif, &rgba, 32, 32, 60, None).unwrap();
        let o = optimize_one(Path::new("looks-like.jpg"), &avif, &settings(80.0));
        assert_eq!(o.status, Status::Skipped);
        assert_eq!(o.reason.as_deref(), Some("avif-input"));
    }

    #[test]
    fn too_large_is_skipped() {
        let input = png(64, 64, false);
        let mut s = settings(80.0);
        s.max_pixels = 64 * 64 - 1;
        let o = optimize_one(Path::new("x.png"), &input, &s);
        assert_eq!(o.status, Status::Skipped);
        assert_eq!(o.reason.as_deref(), Some("too-large"));
    }

    #[test]
    fn memory_cost_per_format() {
        assert_eq!(bytes_per_pixel(&jpeg_header(3)), 200);
        assert_eq!(bytes_per_pixel(&png(8, 8, false)), 260);
        assert_eq!(bytes_per_pixel(b"??"), 260);
    }

    /// Invariants 1-3 of §3: the written bytes are the scored bytes, every output meets
    /// the target, and the result is deterministic.
    #[test]
    fn outputs_meet_target_and_are_the_scored_bytes() {
        for alpha in [false, true] {
            let (w, h) = (96, 64);
            let input = png(w, h, alpha);
            let reference = Reference::new(&image(w, h, alpha), w, h).unwrap();
            for target in [70.0, 85.0] {
                let s = settings(target);
                let a = optimize_one(Path::new("x.png"), &input, &s);
                let b = optimize_one(Path::new("x.png"), &input, &s);
                assert_eq!(a.data, b.data, "deterministic");
                assert_eq!(a.codec, b.codec);
                match a.status {
                    Status::Optimized => {
                        let data = a.data.as_ref().unwrap();
                        assert!((data.len() as u64) < input.len() as u64);
                        let d = codecs::decode(data).unwrap();
                        let score = reference.score(&d.rgba).unwrap();
                        assert_eq!(Some(score), a.score, "reported score is the written bytes'");
                        assert!(score >= target, "{score} < {target}");
                        if alpha {
                            assert_ne!(a.codec, Some(Codec::Jpeg));
                            assert_eq!(a.evals.unwrap().jpeg, 0, "JPEG searched for alpha");
                        }
                    }
                    Status::KeptOriginal => assert!(a.data.is_none()),
                    s => panic!("unexpected {s:?}: {:?}", a.reason),
                }
            }
        }
    }

    /// A JPEG carrying EXIF orientation `o` (a minimal big-endian APP1 after SOI).
    fn with_orientation(jpeg: &[u8], o: u8) -> Vec<u8> {
        let mut app1: Vec<u8> = vec![
            b'E', b'x', b'i', b'f', 0, 0, b'M', b'M', 0, 0x2A, 0, 0, 0, 8,
        ];
        app1.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, o, 0, 0, 0, 0, 0, 0]);
        let len = (app1.len() as u16 + 2).to_be_bytes();
        [
            &jpeg[..2],
            &[0xFF, 0xE1, len[0], len[1]],
            &app1[..],
            &jpeg[2..],
        ]
        .concat()
    }

    /// §3: orientation is applied before scoring and encoding, so the output is upright and
    /// its reported score is against the upright pixels.
    #[test]
    fn orientation_is_applied_before_scoring_and_encoding() {
        let (w, h) = (96, 64);
        let rgba = image(w, h, false);
        let jpeg = codecs::encode(codecs::Codec::Jpeg, &rgba, w, h, 95, None).unwrap();
        let input = with_orientation(&jpeg, 6);
        assert_eq!(codecs::decode(&input).unwrap().exif_orientation, 6);
        let mut s = settings(70.0);
        s.formats = Formats::List(vec![Format::Jpeg, Format::Webp]);
        let o = optimize_one(Path::new("x.jpg"), &input, &s);
        assert_eq!(o.status, Status::Optimized, "{:?}", o.reason);
        let out = codecs::decode(o.data.as_ref().unwrap()).unwrap();
        assert_eq!((out.width, out.height, out.exif_orientation), (h, w, 1));
        let d = codecs::decode(&jpeg).unwrap();
        let (upright, uw, uh) = apply_orientation(&d.rgba, w, h, 6);
        let reference = Reference::new(&upright, uw, uh).unwrap();
        assert_eq!(Some(reference.score(&out.rgba).unwrap()), o.score);
    }

    /// §3: an ICC profile is carried into the output and rules out AVIF (ravif has no ICC API).
    #[test]
    fn icc_is_carried_and_skips_avif() {
        let icc: Vec<u8> = (0..3144).map(|i| (i * 7 % 251) as u8).collect();
        let (w, h) = (192, 128);
        // Grain that PNG cannot compress, so a lossy candidate beats the input.
        let mut rgba = image(w, h, false);
        let mut r = 0x2545_F491_u32;
        for (i, v) in rgba.iter_mut().enumerate() {
            if i % 4 != 3 {
                r = r.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *v = v.saturating_add((r >> 28) as u8);
            }
        }
        let input =
            codecs::encode(codecs::Codec::PngLossless, &rgba, w, h, 100, Some(&icc)).unwrap();
        let o = optimize_one(Path::new("x.png"), &input, &settings(70.0));
        assert_eq!(o.status, Status::Optimized, "{:?}", o.reason);
        assert_eq!(o.evals.unwrap().avif, 0);
        let out = codecs::decode(o.data.as_ref().unwrap()).unwrap();
        assert_eq!(out.icc.as_deref(), Some(&icc[..]), "{:?}", o.codec);
    }

    /// A resize the user asked for is never answered with the full-size original.
    #[test]
    fn resized_without_candidate_is_an_error_not_the_original() {
        let rgba = image(96, 64, false);
        let jpeg = codecs::encode(codecs::Codec::Jpeg, &rgba, 96, 64, 95, None).unwrap();
        // No lossy codec reaches 100 and a JPEG input has no lossless candidate.
        let mut s = settings(100.0);
        s.max_width = Some(48);
        let o = optimize_one(Path::new("x.jpg"), &jpeg, &s);
        assert_eq!(o.status, Status::Error);
        assert_eq!(o.reason.as_deref(), Some("no-candidate-after-resize"));
        assert!(o.data.is_none() && o.codec.is_none());
        // Without the resize the same image keeps its original.
        s.max_width = None;
        let o = optimize_one(Path::new("x.jpg"), &jpeg, &s);
        assert_eq!(o.status, Status::KeptOriginal);
        // A PNG input always has the lossless candidate, at the new size.
        let png = png(96, 64, false);
        s.max_width = Some(48);
        let o = optimize_one(Path::new("x.png"), &png, &s);
        assert_eq!((o.status, o.codec), (Status::Optimized, Some(Codec::Png)));
        let d = codecs::decode(o.data.as_ref().unwrap()).unwrap();
        assert_eq!((d.width, d.height), (48, 32));
    }

    /// libwebp refuses sides above 16,383 px; that skips WebP instead of failing the image.
    #[test]
    fn webp_is_skipped_above_its_size_limit() {
        let (w, h) = (WEBP_MAX_SIDE + 1, 8);
        let input = png(w, h, false);
        let mut s = settings(70.0);
        s.formats = Formats::List(vec![Format::Jpeg, Format::Webp]);
        let o = optimize_one(Path::new("x.png"), &input, &s);
        assert_ne!(o.status, Status::Error, "{:?}", o.reason);
        let e = o.evals.unwrap();
        assert_eq!(e.webp, 0);
        assert!(e.jpeg > 0);
    }

    #[test]
    fn same_restricts_to_input_family() {
        let rgba = image(64, 64, false);
        let jpeg = codecs::encode(codecs::Codec::Jpeg, &rgba, 64, 64, 98, None).unwrap();
        let mut s = settings(70.0);
        s.formats = Formats::Same;
        let o = optimize_one(Path::new("x.jpg"), &jpeg, &s);
        let e = o.evals.unwrap();
        assert_eq!((e.webp, e.avif), (0, 0));
        assert!(matches!(o.codec, Some(Codec::Jpeg | Codec::Original)));
    }
}
