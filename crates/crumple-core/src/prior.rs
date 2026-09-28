//! Initial-q prior (ARCHITECTURE §4.1).

/// Lossy codec kinds searched by the planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecKind {
    Jpeg,
    Webp,
    Avif,
}

/// Table re-fitted in T6 on the 25-image corpus: per codec and target, the median over images
/// of the lowest q whose score meets the target (full q = 1..100 sweep, no size bound).
/// Linear interpolation over the fitted table at targets 70/80/90, extrapolating
/// with the edge slope, rounded and clamped to 1..=100.
pub fn prior_q(kind: CodecKind, target: f64) -> u8 {
    let (a, b, c) = match kind {
        CodecKind::Jpeg => (74.0, 87.0, 98.0),
        CodecKind::Webp => (76.0, 87.0, 98.0),
        CodecKind::Avif => (76.0, 86.0, 94.0),
    };
    let q: f64 = if target <= 80.0 {
        a + (target - 70.0) * (b - a) / 10.0
    } else {
        b + (target - 80.0) * (c - b) / 10.0
    };
    if q.is_nan() {
        return 1;
    }
    q.round().clamp(1.0, 100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_values_and_clamp() {
        let t = [
            (CodecKind::Jpeg, [74, 87, 98]),
            (CodecKind::Webp, [76, 87, 98]),
            (CodecKind::Avif, [76, 86, 94]),
        ];
        for (k, v) in t {
            assert_eq!(prior_q(k, 70.0), v[0]);
            assert_eq!(prior_q(k, 80.0), v[1]);
            assert_eq!(prior_q(k, 90.0), v[2]);
            assert_eq!(prior_q(k, 1000.0), 100);
            assert_eq!(prior_q(k, -1000.0), 1);
        }
        assert_eq!(prior_q(CodecKind::Jpeg, 75.0), 81);
        assert_eq!(prior_q(CodecKind::Jpeg, 60.0), 61);
        assert_eq!(prior_q(CodecKind::Jpeg, 95.0), 100);
        assert_eq!(prior_q(CodecKind::Avif, 95.0), 98);
    }
}
