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
    fn smoothstep_eases_and_clamps() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(0.5), 0.5);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(-3.0), 0.0);
        assert_eq!(smoothstep(4.0), 1.0);
        assert!((smoothstep(0.25) - 0.15625).abs() < 1e-12);
    }
}
