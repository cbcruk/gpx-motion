//! Scalar interpolation helpers.

/// The value `t` of the way from `a` to `b`: `a` at `t = 0`, `b` at `t = 1`.
///
/// `t` is not clamped, so values outside `0..=1` extrapolate.
pub(crate) fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Hermite ease of `t` (`3t² − 2t³`): `0` at or below `0`, `1` at or above
/// `1`, with zero slope at both ends.
pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The segment of a polyline holding arc length `s`: the index `k` with
/// `lengths[k] <= s < lengths[k + 1]`, clamped to the first and last
/// segments. `None` when `lengths` has fewer than two points (no segment).
///
/// `lengths` are the cumulative arc lengths at each point, non-decreasing.
/// A NaN `s` picks the first segment.
pub(crate) fn segment_at<T: PartialOrd + Copy>(lengths: &[T], s: T) -> Option<usize> {
    let n = lengths.len();
    (n >= 2).then(|| lengths.partition_point(|&l| l <= s).clamp(1, n - 1) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_hits_both_ends_and_extrapolates() {
        assert_eq!(lerp(2.0, 6.0, 0.0), 2.0);
        assert_eq!(lerp(2.0, 6.0, 1.0), 6.0);
        assert_eq!(lerp(2.0, 6.0, 0.25), 3.0);
        assert_eq!(lerp(2.0, 6.0, 1.5), 8.0);
        assert_eq!(lerp(6.0, 2.0, 0.5), 4.0);
    }

    #[test]
    fn segment_at_finds_the_segment_and_clamps_to_the_ends() {
        let lengths = [0.0, 10.0, 25.0, 25.0, 40.0];
        assert_eq!(segment_at(&lengths, 0.0), Some(0));
        assert_eq!(segment_at(&lengths, 9.9), Some(0));
        assert_eq!(segment_at(&lengths, 10.0), Some(1));
        assert_eq!(segment_at(&lengths, 30.0), Some(3));
        assert_eq!(segment_at(&lengths, -5.0), Some(0));
        assert_eq!(segment_at(&lengths, 40.0), Some(3));
        assert_eq!(segment_at(&lengths, 99.0), Some(3));
        assert_eq!(segment_at(&lengths, f64::NAN), Some(0));
        assert_eq!(segment_at(&[0.0f32, 3.0], 1.0), Some(0));
    }

    #[test]
    fn segment_at_has_no_segment_below_two_points() {
        assert_eq!(segment_at::<f64>(&[], 1.0), None);
        assert_eq!(segment_at(&[5.0], 1.0), None);
    }

    #[test]
    fn smoothstep_eases_and_clamps() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(0.5), 0.5);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(-3.0), 0.0);
        assert_eq!(smoothstep(4.0), 1.0);
        assert!((smoothstep(0.25) - 0.15625).abs() < 1e-12);
    }
}
