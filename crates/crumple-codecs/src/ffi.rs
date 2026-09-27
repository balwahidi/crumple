//! libwebp FFI. The only module in the crate allowed to use `unsafe`.
#![allow(unsafe_code)]

use crate::CodecError;
use libwebp_sys as sys;

/// Copies a libwebp-owned buffer into a Vec and frees it.
///
/// # Safety
/// `ptr` must be null or a buffer of `len` bytes allocated by libwebp.
unsafe fn take(ptr: *mut u8, len: usize) -> Result<Vec<u8>, CodecError> {
    if ptr.is_null() || len == 0 {
        if !ptr.is_null() {
            unsafe { sys::WebPFree(ptr.cast()) };
        }
        return Err(CodecError::Encode("libwebp encode failed".into()));
    }
    let v = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { sys::WebPFree(ptr.cast()) };
    Ok(v)
}

/// Checks that `pixels` holds exactly `h` rows of `w * bpp` bytes (libwebp reads that many
/// through a raw pointer, so this keeps these safe wrappers sound on their own) and returns
/// (width, height, stride) as C ints.
fn dims(pixels: &[u8], w: u32, h: u32, bpp: u32) -> Result<(i32, i32, i32), CodecError> {
    let too_big = || CodecError::Encode("image too large for libwebp".into());
    if pixels.len() as u128 != u128::from(w) * u128::from(h) * u128::from(bpp) {
        return Err(CodecError::Encode("pixel buffer size mismatch".into()));
    }
    let stride = w.checked_mul(bpp).ok_or_else(too_big)?;
    Ok((
        i32::try_from(w).map_err(|_| too_big())?,
        i32::try_from(h).map_err(|_| too_big())?,
        i32::try_from(stride).map_err(|_| too_big())?,
    ))
}

pub(crate) fn webp_encode_rgb(rgb: &[u8], w: u32, h: u32, q: f32) -> Result<Vec<u8>, CodecError> {
    let (w, h, stride) = dims(rgb, w, h, 3)?;
    let mut out: *mut u8 = std::ptr::null_mut();
    // SAFETY: `rgb` holds h rows of `stride` bytes (checked by `dims`).
    unsafe {
        let n = sys::WebPEncodeRGB(rgb.as_ptr(), w, h, stride, q, &mut out);
        take(out, n)
    }
}

pub(crate) fn webp_encode_rgba(rgba: &[u8], w: u32, h: u32, q: f32) -> Result<Vec<u8>, CodecError> {
    let (w, h, stride) = dims(rgba, w, h, 4)?;
    let mut out: *mut u8 = std::ptr::null_mut();
    // SAFETY: `rgba` holds h rows of `stride` bytes (checked by `dims`).
    unsafe {
        let n = sys::WebPEncodeRGBA(rgba.as_ptr(), w, h, stride, q, &mut out);
        take(out, n)
    }
}

/// Wraps a single-image WebP bitstream with an ICCP chunk using the libwebp mux API.
pub(crate) fn webp_add_icc(webp: &[u8], icc: &[u8]) -> Result<Vec<u8>, CodecError> {
    let fail = |what: &str, e: sys::WebPMuxError| {
        Err(CodecError::Encode(format!("libwebp mux {what}: {e:?}")))
    };
    let image = sys::WebPData {
        bytes: webp.as_ptr(),
        size: webp.len(),
    };
    let chunk = sys::WebPData {
        bytes: icc.as_ptr(),
        size: icc.len(),
    };
    // SAFETY: the mux copies both inputs (copy_data = 1); it is deleted on every path,
    // and the assembled buffer is freed by `take`.
    unsafe {
        let mux = sys::WebPNewInternal(sys::WEBP_MUX_ABI_VERSION as _);
        if mux.is_null() {
            return Err(CodecError::Encode("libwebp mux alloc failed".into()));
        }
        let result = (|| {
            let e = sys::WebPMuxSetImage(mux, &image, 1);
            if e != sys::WebPMuxError::WEBP_MUX_OK {
                return fail("set image", e);
            }
            let e = sys::WebPMuxSetChunk(mux, c"ICCP".as_ptr(), &chunk, 1);
            if e != sys::WebPMuxError::WEBP_MUX_OK {
                return fail("set ICCP", e);
            }
            let mut out = sys::WebPData {
                bytes: std::ptr::null(),
                size: 0,
            };
            let e = sys::WebPMuxAssemble(mux, &mut out);
            if e != sys::WebPMuxError::WEBP_MUX_OK {
                return fail("assemble", e);
            }
            take(out.bytes as *mut u8, out.size)
        })();
        sys::WebPMuxDelete(mux);
        result
    }
}
