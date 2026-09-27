//! EXIF orientation.

/// Applies EXIF orientation 1..=8 to RGBA8; returns (pixels, new_w, new_h). Values outside 1..=8 are treated as 1.
pub fn apply_orientation(rgba: &[u8], w: u32, h: u32, orientation: u8) -> (Vec<u8>, u32, u32) {
    let (wu, hu) = (w as usize, h as usize);
    assert_eq!(rgba.len(), wu * hu * 4, "rgba length does not match w*h*4");
    let o = if (1..=8).contains(&orientation) {
        orientation
    } else {
        1
    };
    if o == 1 {
        return (rgba.to_vec(), w, h);
    }
    let swap = o >= 5;
    let (nw, nh) = if swap { (hu, wu) } else { (wu, hu) };
    let mut out = vec![0u8; rgba.len()];
    for y in 0..nh {
        for x in 0..nw {
            // Source coordinates for destination (x, y).
            let (sx, sy) = match o {
                2 => (wu - 1 - x, y),
                3 => (wu - 1 - x, hu - 1 - y),
                4 => (x, hu - 1 - y),
                5 => (y, x),
                6 => (y, hu - 1 - x),
                7 => (wu - 1 - y, hu - 1 - x),
                _ => (wu - 1 - y, x), // 8
            };
            let s = (sy * wu + sx) * 4;
            let d = (y * nw + x) * 4;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    (out, nw as u32, nh as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(ids: &[u8]) -> Vec<u8> {
        ids.iter().flat_map(|&i| [i, i + 10, i + 20, 255]).collect()
    }

    #[test]
    fn all_orientations_3x2() {
        // Source (3x2):
        // 0 1 2
        // 3 4 5
        let src = img(&[0, 1, 2, 3, 4, 5]);
        let cases: [(u8, u32, u32, [u8; 6]); 10] = [
            (1, 3, 2, [0, 1, 2, 3, 4, 5]),
            (2, 3, 2, [2, 1, 0, 5, 4, 3]),
            (3, 3, 2, [5, 4, 3, 2, 1, 0]),
            (4, 3, 2, [3, 4, 5, 0, 1, 2]),
            (5, 2, 3, [0, 3, 1, 4, 2, 5]),
            (6, 2, 3, [3, 0, 4, 1, 5, 2]),
            (7, 2, 3, [5, 2, 4, 1, 3, 0]),
            (8, 2, 3, [2, 5, 1, 4, 0, 3]),
            (0, 3, 2, [0, 1, 2, 3, 4, 5]),
            (9, 3, 2, [0, 1, 2, 3, 4, 5]),
        ];
        for (o, ew, eh, ids) in cases {
            let (px, nw, nh) = apply_orientation(&src, 3, 2, o);
            assert_eq!((nw, nh), (ew, eh), "orientation {o}");
            assert_eq!(px, img(&ids), "orientation {o}");
        }
    }

    /// Independent of the hand-written arrays: each orientation undone by its EXIF inverse
    /// (6 and 8 are quarter turns in opposite directions; the rest are their own inverse)
    /// gives back the source, on a non-square image with distinct pixels.
    #[test]
    fn inverse_pairs_round_trip() {
        let (w, h) = (5u32, 3u32);
        let src: Vec<u8> = (0..w * h)
            .flat_map(|i| [i as u8, 100 + i as u8, 200 - i as u8, 255])
            .collect();
        for (o, inv) in [(2, 2), (3, 3), (4, 4), (5, 5), (6, 8), (7, 7), (8, 6)] {
            let (px, nw, nh) = apply_orientation(&src, w, h, o);
            let (back, bw, bh) = apply_orientation(&px, nw, nh, inv);
            assert_eq!((bw, bh), (w, h), "orientation {o}");
            assert_eq!(back, src, "orientation {o}");
        }
        // Two quarter turns make a half turn.
        let (q, qw, qh) = apply_orientation(&src, w, h, 6);
        assert_eq!(
            apply_orientation(&q, qw, qh, 6),
            apply_orientation(&src, w, h, 3)
        );
    }
}
