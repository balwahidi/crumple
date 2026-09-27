//! Downscale-to-fit resizing.

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

fn target_dims(w: u32, h: u32, max_w: Option<u32>, max_h: Option<u32>) -> (u32, u32) {
    let sw = max_w.map_or(f64::INFINITY, |m| m as f64 / w as f64);
    let sh = max_h.map_or(f64::INFINITY, |m| m as f64 / h as f64);
    let s = sw.min(sh);
    if s >= 1.0 {
        return (w, h);
    }
    let nw = ((w as f64 * s).round() as u32).max(1);
    let nh = ((h as f64 * s).round() as u32).max(1);
    (nw, nh)
}

/// Fit within (max_w, max_h), never upscale, keep aspect ratio (round to nearest, min 1 px).
/// Lanczos3 with premultiplied alpha (fast_image_resize MulDiv). Returns the input unchanged if it already fits.
pub fn fit_within(
    rgba: &[u8],
    w: u32,
    h: u32,
    max_w: Option<u32>,
    max_h: Option<u32>,
) -> (Vec<u8>, u32, u32) {
    assert_eq!(
        rgba.len(),
        w as usize * h as usize * 4,
        "rgba length does not match w*h*4"
    );
    let (nw, nh) = target_dims(w, h, max_w, max_h);
    if (nw, nh) == (w, h) || w == 0 || h == 0 {
        return (rgba.to_vec(), w, h);
    }
    let src = ImageRef::new(w, h, rgba, PixelType::U8x4).expect("valid source image");
    let mut dst = Image::new(nw, nh, PixelType::U8x4);
    let opts = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
        .use_alpha(true);
    Resizer::new()
        .resize(&src, &mut dst, &opts)
        .expect("resize with matching pixel types");
    (dst.into_vec(), nw, nh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_768x512_to_400() {
        let src = vec![128u8; 768 * 512 * 4];
        let (px, w, h) = fit_within(&src, 768, 512, Some(400), Some(400));
        assert_eq!((w, h), (400, 267));
        assert_eq!(px.len(), 400 * 267 * 4);
    }

    #[test]
    fn already_fits_unchanged() {
        let src: Vec<u8> = (0..100 * 50 * 4).map(|i| (i % 251) as u8).collect();
        let (px, w, h) = fit_within(&src, 100, 50, Some(400), None);
        assert_eq!((w, h), (100, 50));
        assert_eq!(px, src);
        let (px, _, _) = fit_within(&src, 100, 50, None, None);
        assert_eq!(px, src);
    }

    #[test]
    fn transparent_rgb_does_not_bleed() {
        // Opaque red next to fully transparent green.
        let src = vec![255, 0, 0, 255, 0, 255, 0, 0];
        let (px, w, h) = fit_within(&src, 2, 1, Some(1), None);
        assert_eq!((w, h), (1, 1));
        assert_eq!(px[1], 0, "green bled into the result: {px:?}");
        assert!(px[0] >= 250, "{px:?}");
        assert!(px[3] > 0 && px[3] < 255, "{px:?}");
    }

    #[test]
    fn rounding_and_minimum_size() {
        let px = |w: u32, h: u32| vec![7u8; (w * h * 4) as usize];
        // Only the height is bounded; the width follows the aspect ratio, rounded to nearest.
        assert_eq!(fit_within(&px(300, 1000), 300, 1000, None, Some(100)).1, 30);
        let (_, w, h) = fit_within(&px(333, 1000), 333, 1000, None, Some(100));
        assert_eq!((w, h), (33, 100));
        let (_, w, h) = fit_within(&px(335, 1000), 335, 1000, None, Some(100));
        assert_eq!((w, h), (34, 100)); // 33.5 rounds up
                                       // A side never drops below 1 px, and the result never exceeds the box.
        let (_, w, h) = fit_within(&px(1000, 2), 1000, 2, Some(10), Some(10));
        assert_eq!((w, h), (10, 1));
        // Never upscale, even with a box larger than the image on one side only.
        let (_, w, h) = fit_within(&px(50, 40), 50, 40, Some(500), Some(40));
        assert_eq!((w, h), (50, 40));
    }
}
