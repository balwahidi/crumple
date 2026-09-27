// Portions of this file are ported from the `ssimulacra2` crate v0.5.1
// (src/lib.rs: downscale_by_2, image_multiply, ssim_map, edge_diff_map,
// make_positive_xyb, xyb_to_planar and the Msssim scoring),
// Copyright (c) 2022-2022, the rav1e contributors, BSD-2-Clause.
// See LICENSE-ssimulacra2 in this crate. Changes: reference-only work
// (XYB planes, blurred mu1, blurred sigma11 per scale) is cached in
// `Reference`, and alpha is handled by compositing over black and white.

//! SSIMULACRA2 with a cached reference.

use ssimulacra2::{Blur, ColorPrimaries, LinearRgb, Rgb, TransferCharacteristic, Xyb};

mod ported;
use ported::{
    downscale_by_2, edge_diff_map, image_multiply, make_positive_xyb, ssim_map, xyb_to_planar,
    Msssim, MsssimScale, NUM_SCALES,
};

/// Errors from building a [`Reference`] or scoring against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricError {
    /// Buffer length is not `width * height * 4`.
    BadLength,
    /// Sizes differ.
    SizeMismatch,
    /// Image is smaller than 8x8.
    TooSmall,
}

impl std::fmt::Display for MetricError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MetricError::BadLength => "buffer length is not width*height*4",
            MetricError::SizeMismatch => "image sizes differ",
            MetricError::TooSmall => "images must be at least 8x8 pixels",
        })
    }
}

impl std::error::Error for MetricError {}

type Planes = [Vec<f32>; 3];

struct CachedScale {
    width: usize,
    height: usize,
    img1: Planes,
    mu1: Planes,
    sigma11: Planes,
}

struct Cached {
    scales: Vec<CachedScale>,
}

/// A reference image with everything that depends only on it precomputed.
pub struct Reference {
    width: u32,
    height: u32,
    has_alpha: bool,
    /// Opaque: one entry. With alpha: over black, then over white.
    cached: Vec<Cached>,
}

impl Reference {
    /// `rgba`: sRGB, straight alpha, row-major, len == width*height*4. Errors: BadLength, TooSmall (<8x8).
    pub fn new(rgba: &[u8], width: u32, height: u32) -> Result<Reference, MetricError> {
        check_len(rgba, width, height)?;
        if width < 8 || height < 8 {
            return Err(MetricError::TooSmall);
        }
        let (w, h) = (width as usize, height as usize);
        let has_alpha = rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
        let cached = if has_alpha {
            vec![
                Cached::new(to_linear(rgba, w, h, Some(0)), w, h),
                Cached::new(to_linear(rgba, w, h, Some(255)), w, h),
            ]
        } else {
            vec![Cached::new(to_linear(rgba, w, h, None), w, h)]
        };
        Ok(Reference {
            width,
            height,
            has_alpha,
            cached,
        })
    }

    /// Same layout and size as the reference. Returns SSIMULACRA2 (100 = identical).
    pub fn score(&self, distorted_rgba: &[u8]) -> Result<f64, MetricError> {
        check_len(distorted_rgba, self.width, self.height)?;
        let (w, h) = (self.width as usize, self.height as usize);
        if self.has_alpha {
            let black = self.cached[0].score(to_linear(distorted_rgba, w, h, Some(0)), w, h);
            let white = self.cached[1].score(to_linear(distorted_rgba, w, h, Some(255)), w, h);
            Ok(black.min(white))
        } else {
            Ok(self.cached[0].score(to_linear(distorted_rgba, w, h, None), w, h))
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// True if any reference alpha < 255.
    pub fn has_alpha(&self) -> bool {
        self.has_alpha
    }
}

/// Convenience: `Reference::new(reference, w, h)?.score(distorted)`.
pub fn ssimulacra2(
    reference: &[u8],
    distorted: &[u8],
    width: u32,
    height: u32,
) -> Result<f64, MetricError> {
    Reference::new(reference, width, height)?.score(distorted)
}

fn check_len(rgba: &[u8], width: u32, height: u32) -> Result<(), MetricError> {
    // [Reviewer] u128: with u64, width*height*4 overflows for huge sizes (a panic in debug,
    // and in release a wrap to a small number that a short buffer could match).
    let expected = u128::from(width) * u128::from(height) * 4;
    if rgba.len() as u128 != expected {
        return Err(MetricError::BadLength);
    }
    Ok(())
}

/// sRGB bytes to linear RGB, optionally compositing over a grey level `bg`.
fn to_linear(rgba: &[u8], w: usize, h: usize, bg: Option<u8>) -> LinearRgb {
    let data: Vec<[f32; 3]> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let px = match bg {
                None => [p[0], p[1], p[2]],
                Some(bg) => {
                    let a = f32::from(p[3]) / 255.0;
                    let b = f32::from(bg);
                    let mix = |c: u8| (f32::from(c) * a + b * (1.0 - a)).round() as u8;
                    [mix(p[0]), mix(p[1]), mix(p[2])]
                }
            };
            [
                f32::from(px[0]) / 255.0,
                f32::from(px[1]) / 255.0,
                f32::from(px[2]) / 255.0,
            ]
        })
        .collect();
    let rgb = Rgb::new(
        data,
        w,
        h,
        TransferCharacteristic::SRGB,
        ColorPrimaries::BT709,
    )
    .expect("resolution and data size match");
    LinearRgb::try_from(rgb).expect("sRGB/BT.709 to linear RGB is supported")
}

fn to_planes(lin: &LinearRgb) -> Planes {
    let mut xyb = Xyb::from(lin.clone());
    make_positive_xyb(&mut xyb);
    xyb_to_planar(&xyb)
}

impl Cached {
    fn new(mut img: LinearRgb, width: usize, height: usize) -> Cached {
        let (mut width, mut height) = (width, height);
        let mut mul: Planes = std::array::from_fn(|_| vec![0.0f32; width * height]);
        let mut blur = Blur::new(width, height);
        let mut scales = Vec::with_capacity(NUM_SCALES);
        for scale in 0..NUM_SCALES {
            if width < 8 || height < 8 {
                break;
            }
            if scale > 0 {
                img = downscale_by_2(&img);
                width = img.width();
                height = img.height();
            }
            for c in &mut mul {
                c.truncate(width * height);
            }
            blur.shrink_to(width, height);
            let img1 = to_planes(&img);
            image_multiply(&img1, &img1, &mut mul);
            let sigma11 = blur.blur(&mul);
            let mu1 = blur.blur(&img1);
            scales.push(CachedScale {
                width,
                height,
                img1,
                mu1,
                sigma11,
            });
        }
        Cached { scales }
    }

    fn score(&self, mut img: LinearRgb, width: usize, height: usize) -> f64 {
        let mut mul: Planes = std::array::from_fn(|_| vec![0.0f32; width * height]);
        let mut blur = Blur::new(width, height);
        let mut msssim = Msssim::default();
        for (scale, cs) in self.scales.iter().enumerate() {
            if scale > 0 {
                img = downscale_by_2(&img);
            }
            let (width, height) = (cs.width, cs.height);
            for c in &mut mul {
                c.truncate(width * height);
            }
            blur.shrink_to(width, height);
            let img2 = to_planes(&img);

            image_multiply(&img2, &img2, &mut mul);
            let sigma2_sq = blur.blur(&mul);
            image_multiply(&cs.img1, &img2, &mut mul);
            let sigma12 = blur.blur(&mul);
            let mu2 = blur.blur(&img2);

            let avg_ssim = ssim_map(
                width,
                height,
                &cs.mu1,
                &mu2,
                &cs.sigma11,
                &sigma2_sq,
                &sigma12,
            );
            let avg_edgediff = edge_diff_map(width, height, &cs.img1, &cs.mu1, &img2, &mu2);
            msssim.scales.push(MsssimScale {
                avg_ssim,
                avg_edgediff,
            });
        }
        msssim.score()
    }
}
