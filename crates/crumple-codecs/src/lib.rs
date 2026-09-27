//! Encoders and decoders for Crumple.
//!
//! Encoders use frozen settings (see `docs/ARCHITECTURE.md` §9 T2). Decoders sniff
//! magic bytes and always return 8-bit RGBA.
#![deny(unsafe_code)]

#[cfg(feature = "webp")]
mod ffi;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    Jpeg,
    WebpLossy,
    Avif,
    PngLossless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Avif,
}

pub struct Decoded {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub icc: Option<Vec<u8>>,
    /// 1..=8, 1 if absent.
    pub exif_orientation: u8,
    pub format: Format,
    pub animated: bool,
}

#[derive(Debug)]
pub enum CodecError {
    UnknownFormat,
    Decode(String),
    Encode(String),
    Unsupported(&'static str),
    /// [Reviewer] The header declares more than the allowed number of pixels
    /// (see [`decode_with_limit`]). Nothing large was allocated.
    TooLarge {
        width: u32,
        height: u32,
    },
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::UnknownFormat => write!(f, "unknown image format"),
            CodecError::Decode(e) => write!(f, "decode error: {e}"),
            CodecError::Encode(e) => write!(f, "encode error: {e}"),
            CodecError::Unsupported(what) => write!(f, "unsupported: {what}"),
            CodecError::TooLarge { width, height } => {
                write!(f, "image is {width}x{height}, above the pixel limit")
            }
        }
    }
}

impl std::error::Error for CodecError {}

pub fn supports_alpha(codec: Codec) -> bool {
    !matches!(codec, Codec::Jpeg)
}

pub fn extension(codec: Codec) -> &'static str {
    match codec {
        Codec::Jpeg => "jpg",
        Codec::WebpLossy => "webp",
        Codec::Avif => "avif",
        Codec::PngLossless => "png",
    }
}

fn is_opaque(rgba: &[u8]) -> bool {
    rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
}

#[allow(dead_code)]
fn to_rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

/// quality 1..=100 (ignored for PngLossless). `icc`: embed if supported, else Err(Unsupported("icc")).
pub fn encode(
    codec: Codec,
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, CodecError> {
    // Checked u64 math, so that width*height*4 can neither wrap and let a short buffer
    // through nor panic on overflow.
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(rgba.len() as u64) {
        let expected = expected.map_or("more than u64::MAX".into(), |n| n.to_string());
        return Err(CodecError::Encode(format!(
            "buffer is {} bytes, expected {expected} for {width}x{height} RGBA",
            rgba.len()
        )));
    }
    // An empty profile is treated as no profile, for every codec.
    let icc = icc.filter(|p| !p.is_empty());
    let q = quality.clamp(1, 100);
    let opaque = is_opaque(rgba);
    match codec {
        Codec::Jpeg => encode_jpeg(rgba, width, height, q, icc, opaque),
        Codec::WebpLossy => encode_webp(rgba, width, height, q, icc, opaque),
        Codec::Avif => encode_avif(rgba, width, height, q, icc, opaque),
        Codec::PngLossless => encode_png(rgba, width, height, icc, opaque),
    }
}

#[cfg(feature = "jpeg")]
fn encode_jpeg(
    rgba: &[u8],
    w: u32,
    h: u32,
    q: u8,
    icc: Option<&[u8]>,
    opaque: bool,
) -> Result<Vec<u8>, CodecError> {
    if !opaque {
        return Err(CodecError::Unsupported("alpha"));
    }
    if icc.is_some_and(|i| i.len() > 65_519 * 255) {
        return Err(CodecError::Encode("ICC profile size out of range".into()));
    }
    let rgb = to_rgb(rgba);
    // mozjpeg reports libjpeg errors by panicking; turn that into an error value.
    std::panic::catch_unwind(|| -> std::io::Result<Vec<u8>> {
        let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        c.set_size(w as usize, h as usize);
        c.set_quality(q as f32);
        c.set_progressive_mode();
        c.set_optimize_scans(true);
        let mut c = c.start_compress(Vec::new())?;
        if let Some(icc) = icc {
            // Not `write_icc_profile`: mozjpeg 0.10.13 numbers the APP2 chunks from 0,
            // but the ICC spec (and zune-jpeg) require sequence numbers starting at 1.
            // Layout per ICC.1 Annex B.4: "ICC_PROFILE\0", sequence number (1-based),
            // chunk count, then at most 65,519 profile bytes (65,533-byte APP2 payload
            // minus the 14-byte header).
            // TODO: switch back to `write_icc_profile` once ImageOptim/mozjpeg-rust#56
            // (1-based sequence numbers) is released, and bump the pinned version.
            let chunks = icc.chunks(65_519);
            let total = chunks.len() as u8;
            for (i, chunk) in chunks.enumerate() {
                let mut seg = b"ICC_PROFILE\0".to_vec();
                seg.extend([i as u8 + 1, total]);
                seg.extend_from_slice(chunk);
                c.write_marker(mozjpeg::Marker::APP(2), &seg);
            }
        }
        c.write_scanlines(&rgb)?;
        c.finish()
    })
    .map_err(|p| {
        let msg = p
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| p.downcast_ref::<&str>().copied())
            .unwrap_or("panicked");
        CodecError::Encode(format!("mozjpeg: {msg}"))
    })?
    .map_err(|e| CodecError::Encode(e.to_string()))
}

#[cfg(not(feature = "jpeg"))]
fn encode_jpeg(
    _: &[u8],
    _: u32,
    _: u32,
    _: u8,
    _: Option<&[u8]>,
    _: bool,
) -> Result<Vec<u8>, CodecError> {
    Err(CodecError::Unsupported("jpeg"))
}

#[cfg(feature = "webp")]
fn encode_webp(
    rgba: &[u8],
    w: u32,
    h: u32,
    q: u8,
    icc: Option<&[u8]>,
    opaque: bool,
) -> Result<Vec<u8>, CodecError> {
    let out = if opaque {
        ffi::webp_encode_rgb(&to_rgb(rgba), w, h, q as f32)
    } else {
        ffi::webp_encode_rgba(rgba, w, h, q as f32)
    }?;
    match icc {
        Some(icc) => ffi::webp_add_icc(&out, icc),
        None => Ok(out),
    }
}

#[cfg(not(feature = "webp"))]
fn encode_webp(
    _: &[u8],
    _: u32,
    _: u32,
    _: u8,
    _: Option<&[u8]>,
    _: bool,
) -> Result<Vec<u8>, CodecError> {
    Err(CodecError::Unsupported("webp"))
}

#[cfg(feature = "avif")]
fn encode_avif(
    rgba: &[u8],
    w: u32,
    h: u32,
    q: u8,
    icc: Option<&[u8]>,
    opaque: bool,
) -> Result<Vec<u8>, CodecError> {
    use rgb::FromSlice;
    if icc.is_some() {
        return Err(CodecError::Unsupported("icc"));
    }
    let enc = ravif::Encoder::new()
        .with_quality(q as f32)
        .with_alpha_quality(q as f32)
        .with_speed(6)
        .with_num_threads(Some(1));
    let (w, h) = (w as usize, h as usize);
    let res = if opaque {
        let rgb = to_rgb(rgba);
        enc.encode_rgb(imgref::Img::new(rgb.as_rgb(), w, h))
    } else {
        enc.encode_rgba(imgref::Img::new(rgba.as_rgba(), w, h))
    };
    res.map(|e| e.avif_file)
        .map_err(|e| CodecError::Encode(e.to_string()))
}

#[cfg(not(feature = "avif"))]
fn encode_avif(
    _: &[u8],
    _: u32,
    _: u32,
    _: u8,
    _: Option<&[u8]>,
    _: bool,
) -> Result<Vec<u8>, CodecError> {
    Err(CodecError::Unsupported("avif"))
}

#[cfg(feature = "png")]
fn encode_png(
    rgba: &[u8],
    w: u32,
    h: u32,
    icc: Option<&[u8]>,
    opaque: bool,
) -> Result<Vec<u8>, CodecError> {
    use oxipng::{BitDepth, ColorType, RawImage};
    let (ct, data) = if opaque {
        (
            ColorType::RGB {
                transparent_color: None,
            },
            to_rgb(rgba),
        )
    } else {
        (ColorType::RGBA, rgba.to_vec())
    };
    let mut img = RawImage::new(w, h, ct, BitDepth::Eight, data)
        .map_err(|e| CodecError::Encode(e.to_string()))?;
    if let Some(icc) = icc {
        img.add_icc_profile(icc);
    }
    img.create_optimized_png(&oxipng::Options::from_preset(2))
        .map_err(|e| CodecError::Encode(e.to_string()))
}

#[cfg(not(feature = "png"))]
fn encode_png(_: &[u8], _: u32, _: u32, _: Option<&[u8]>, _: bool) -> Result<Vec<u8>, CodecError> {
    Err(CodecError::Unsupported("png"))
}

/// [Reviewer] The format `decode` would use for these bytes, from magic bytes only.
/// Callers use it to refuse input formats before decoding (see the AVIF note on [`decode`]).
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG") {
        return Some(Format::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(Format::Webp);
    }
    if bytes.len() >= 16 && &bytes[4..8] == b"ftyp" {
        let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let end = size.clamp(16, bytes.len());
        // major brand at 8..12, minor version at 12..16, compatible brands after.
        let is_avif = |b: &[u8]| b == b"avif" || b == b"avis";
        if is_avif(&bytes[8..12]) || bytes[16..end].as_chunks::<4>().0.iter().any(|b| is_avif(b)) {
            return Some(Format::Avif);
        }
    }
    None
}

/// [Reviewer] Pixel limit used by [`decode`]: 2^27 (about 134 MP, 512 MiB of RGBA8).
/// Larger declared sizes are refused with [`CodecError::TooLarge`] after the header is
/// read and before any pixel buffer is allocated, so a small hostile file cannot make
/// the process allocate gigabytes (a failed allocation aborts, it does not return Err).
pub const DEFAULT_MAX_PIXELS: u64 = 1 << 27;

/// Sniffs magic bytes. Always returns 8-bit RGBA (16-bit PNG → 8-bit; gray/palette expanded).
/// Same as [`decode_with_limit`] with [`DEFAULT_MAX_PIXELS`].
///
/// [Reviewer] **AVIF: only decode bytes produced by [`encode`].** rav1d 1.1.0 (inside
/// `avif-decode`) panics on some corrupt AV1 data inside `extern "C"` functions, and that
/// aborts the whole process; it cannot be caught. Refuse AVIF inputs with [`sniff`] first.
///
/// An ICC profile whose header names a non-RGB colour space (GRAY, CMYK, ...) is dropped,
/// because the pixels returned are always RGB(A).
pub fn decode(bytes: &[u8]) -> Result<Decoded, CodecError> {
    decode_with_limit(bytes, DEFAULT_MAX_PIXELS)
}

/// [Reviewer] [`decode`] with a caller-chosen limit on `width * height` (for example the
/// CLI's `--max-pixels`). A header above the limit gives [`CodecError::TooLarge`].
pub fn decode_with_limit(bytes: &[u8], max_pixels: u64) -> Result<Decoded, CodecError> {
    let d = match sniff(bytes).ok_or(CodecError::UnknownFormat)? {
        Format::Png => decode_png(bytes, max_pixels),
        Format::Jpeg => decode_jpeg(bytes, max_pixels),
        Format::Webp => decode_webp(bytes, max_pixels),
        Format::Avif => decode_avif(bytes, max_pixels),
    }?;
    // Every decoder must hand back exactly width*height RGBA pixels.
    check_dims(d.width, d.height, max_pixels)?;
    if d.rgba.len() as u128 != u128::from(d.width) * u128::from(d.height) * 4 {
        return Err(CodecError::Decode(format!(
            "decoder returned {} bytes for {}x{}",
            d.rgba.len(),
            d.width,
            d.height
        )));
    }
    Ok(Decoded {
        icc: d.icc.filter(|p| !p.is_empty() && !non_rgb_icc(p)),
        ..d
    })
}

/// True if the ICC header's data colour space (bytes 16..20) is a known non-RGB one.
/// Such a profile (for example from a grayscale or CMYK source) describes pixels this
/// crate never returns, so it must not be carried into RGB outputs.
fn non_rgb_icc(icc: &[u8]) -> bool {
    const NON_RGB: [&[u8; 4]; 24] = [
        b"XYZ ", b"Lab ", b"Luv ", b"YCbr", b"Yxy ", b"GRAY", b"HSV ", b"HLS ", b"CMYK", b"CMY ",
        b"2CLR", b"3CLR", b"4CLR", b"5CLR", b"6CLR", b"7CLR", b"8CLR", b"9CLR", b"ACLR", b"BCLR",
        b"CCLR", b"DCLR", b"ECLR", b"FCLR",
    ];
    icc.get(16..20)
        .is_some_and(|cs| NON_RGB.iter().any(|n| cs == *n))
}

/// Refuses zero-sized images and images above `max_pixels`.
fn check_dims(width: u32, height: u32, max_pixels: u64) -> Result<(), CodecError> {
    if width == 0 || height == 0 {
        return Err(CodecError::Decode(format!("empty image {width}x{height}")));
    }
    if u64::from(width) * u64::from(height) > max_pixels {
        return Err(CodecError::TooLarge { width, height });
    }
    Ok(())
}

/// Reads tag 0x0112 from raw Exif (TIFF header, optionally prefixed by `Exif\0\0`).
#[allow(dead_code)]
fn orientation(exif: Option<&[u8]>) -> u8 {
    let Some(mut data) = exif else { return 1 };
    if data.starts_with(b"Exif\0\0") {
        data = &data[6..];
    }
    let Ok(parsed) = exif::Reader::new().read_raw(data.to_vec()) else {
        return 1;
    };
    parsed
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .filter(|v| (1..=8).contains(v))
        .map_or(1, |v| v as u8)
}

#[allow(dead_code)]
fn expand(buf: &[u8], channels: usize) -> Vec<u8> {
    match channels {
        1 => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        2 => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        3 => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        _ => buf.to_vec(),
    }
}

#[cfg(feature = "png")]
fn decode_png(bytes: &[u8], max_pixels: u64) -> Result<Decoded, CodecError> {
    let err = |e: png::DecodingError| CodecError::Decode(e.to_string());
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(err)?;
    check_dims(r.info().width, r.info().height, max_pixels)?;
    let size = r
        .output_buffer_size()
        .ok_or_else(|| CodecError::Decode("image too large".into()))?;
    let mut buf = vec![0; size];
    let frame = r.next_frame(&mut buf).map_err(err)?;
    buf.truncate(frame.buffer_size());
    let channels = frame.color_type.samples();
    let info = r.info();
    Ok(Decoded {
        rgba: expand(&buf, channels),
        width: frame.width,
        height: frame.height,
        icc: info.icc_profile.as_ref().map(|c| c.to_vec()),
        exif_orientation: orientation(info.exif_metadata.as_deref()),
        format: Format::Png,
        animated: info.animation_control.is_some(),
    })
}

#[cfg(not(feature = "png"))]
fn decode_png(_: &[u8], _: u64) -> Result<Decoded, CodecError> {
    Err(CodecError::Unsupported("png"))
}

#[cfg(feature = "jpeg")]
fn decode_jpeg(bytes: &[u8], max_pixels: u64) -> Result<Decoded, CodecError> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let err = |e: zune_jpeg::errors::DecodeErrors| CodecError::Decode(format!("{e:?}"));
    // zune-jpeg's default per-side cap is 16,384, which rejects legitimate panoramas;
    // allow the full JPEG range and rely on `max_pixels` instead.
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(usize::from(u16::MAX))
        .set_max_height(usize::from(u16::MAX));
    let mut d = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(bytes), opts);
    d.decode_headers().map_err(err)?;
    let (w, h) = d
        .dimensions()
        .ok_or_else(|| CodecError::Decode("no dimensions".into()))?;
    // zune-jpeg dimensions are u16 internally, so these casts are lossless.
    let (w, h) = (w as u32, h as u32);
    check_dims(w, h, max_pixels)?;
    let px = d.decode().map_err(err)?;
    // Two-component JPEGs decode as raw "multiband" samples, not RGB.
    if matches!(d.input_colorspace(), Some(ColorSpace::MultiBand(_))) {
        return Err(CodecError::Unsupported("jpeg component layout"));
    }
    Ok(Decoded {
        rgba: expand(&px, 3),
        width: w,
        height: h,
        icc: d.icc_profile(),
        exif_orientation: orientation(d.exif().map(Vec::as_slice)),
        format: Format::Jpeg,
        animated: false,
    })
}

#[cfg(not(feature = "jpeg"))]
fn decode_jpeg(_: &[u8], _: u64) -> Result<Decoded, CodecError> {
    Err(CodecError::Unsupported("jpeg"))
}

#[cfg(feature = "webp")]
fn decode_webp(bytes: &[u8], max_pixels: u64) -> Result<Decoded, CodecError> {
    let err = |e: image_webp::DecodingError| CodecError::Decode(e.to_string());
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).map_err(err)?;
    // Chunk sizes are not checked against the file length, and ICCP/EXIF are read into
    // a buffer of the declared size; no chunk can be larger than the whole file.
    d.set_memory_limit(bytes.len());
    let (w, h) = d.dimensions();
    check_dims(w, h, max_pixels)?;
    let size = d
        .output_buffer_size()
        .ok_or_else(|| CodecError::Decode("image too large".into()))?;
    let mut buf = vec![0; size];
    d.read_image(&mut buf).map_err(err)?;
    let channels = if d.has_alpha() { 4 } else { 3 };
    let animated = d.is_animated();
    let icc = d.icc_profile().map_err(err)?;
    let exif = d.exif_metadata().map_err(err)?;
    Ok(Decoded {
        rgba: expand(&buf, channels),
        width: w,
        height: h,
        icc,
        exif_orientation: orientation(exif.as_deref()),
        format: Format::Webp,
        animated,
    })
}

#[cfg(not(feature = "webp"))]
fn decode_webp(_: &[u8], _: u64) -> Result<Decoded, CodecError> {
    Err(CodecError::Unsupported("webp"))
}

#[cfg(feature = "avif")]
fn decode_avif(bytes: &[u8], max_pixels: u64) -> Result<Decoded, CodecError> {
    use avif_decode::Image;
    let err = |e: avif_decode::Error| CodecError::Decode(e.to_string());
    // rav1d sizes its frame buffers from the AV1 sequence header with no cap, so check
    // the declared maximum frame size of the color and alpha items before decoding.
    {
        let perr = |e: avif_parse::Error| CodecError::Decode(e.to_string());
        let avif = avif_parse::read_avif(&mut &bytes[..]).map_err(perr)?;
        let color = avif.primary_item_metadata().map_err(perr)?;
        check_dims(
            color.max_frame_width.get(),
            color.max_frame_height.get(),
            max_pixels,
        )?;
        if let Some(alpha) = avif.alpha_item_metadata().map_err(perr)? {
            check_dims(
                alpha.max_frame_width.get(),
                alpha.max_frame_height.get(),
                max_pixels,
            )?;
        }
    }
    let img = avif_decode::Decoder::from_avif(bytes)
        .map_err(err)?
        .to_image()
        .map_err(err)?;
    let to8 = |v: u16| ((v as u32 * 255 + 32767) / 65535) as u8;
    let (rgba, w, h): (Vec<u8>, usize, usize) = match img {
        Image::Rgb8(i) => (
            i.pixels().flat_map(|p| [p.r, p.g, p.b, 255]).collect(),
            i.width(),
            i.height(),
        ),
        Image::Rgba8(i) => (
            i.pixels().flat_map(|p| [p.r, p.g, p.b, p.a]).collect(),
            i.width(),
            i.height(),
        ),
        Image::Rgb16(i) => (
            i.pixels()
                .flat_map(|p| [to8(p.r), to8(p.g), to8(p.b), 255])
                .collect(),
            i.width(),
            i.height(),
        ),
        Image::Rgba16(i) => (
            i.pixels()
                .flat_map(|p| [to8(p.r), to8(p.g), to8(p.b), to8(p.a)])
                .collect(),
            i.width(),
            i.height(),
        ),
        Image::Gray8(i) => (
            i.pixels()
                .flat_map(|p| {
                    let g = *p;
                    [g, g, g, 255]
                })
                .collect(),
            i.width(),
            i.height(),
        ),
        Image::Gray16(i) => (
            i.pixels()
                .flat_map(|p| {
                    let g = to8(*p);
                    [g, g, g, 255]
                })
                .collect(),
            i.width(),
            i.height(),
        ),
    };
    Ok(Decoded {
        rgba,
        width: w as u32,
        height: h as u32,
        icc: None,
        exif_orientation: 1,
        format: Format::Avif,
        animated: false,
    })
}

#[cfg(not(feature = "avif"))]
fn decode_avif(_: &[u8], _: u64) -> Result<Decoded, CodecError> {
    Err(CodecError::Unsupported("avif"))
}
