use tiny_skia::Color;

/// Low-to-high stops: deep blue, cyan, green, yellow, red.
const STOPS: [(f32, [u8; 3]); 5] = [
    (0.0, [0x2c, 0x7b, 0xb6]),
    (0.25, [0x00, 0xb4, 0xd8]),
    (0.5, [0x6c, 0xd9, 0x7e]),
    (0.75, [0xf9, 0xd0, 0x57]),
    (1.0, [0xe0, 0x3a, 0x2f]),
];

/// Color for a normalized elevation `t` (`0` = lowest, `1` = highest).
///
/// Values outside `0..=1` are clamped.
pub fn elevation_color(t: f64) -> Color {
    let t = (t as f32).clamp(0.0, 1.0);
    let upper = STOPS
        .iter()
        .position(|&(at, _)| at >= t)
        .unwrap_or(STOPS.len() - 1);
    let lower = upper.saturating_sub(1);
    let ((a_at, a), (b_at, b)) = (STOPS[lower], STOPS[upper]);
    let f = if b_at > a_at {
        (t - a_at) / (b_at - a_at)
    } else {
        0.0
    };
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f) / 255.0;
    Color::from_rgba(mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]), 1.0).unwrap_or(Color::WHITE)
}

/// The ramp's color stops as `(position, color)`, for building gradients.
pub(crate) fn stops() -> impl Iterator<Item = (f32, Color)> {
    STOPS
        .iter()
        .map(|&(at, [r, g, b])| (at, Color::from_rgba8(r, g, b, 0xff)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_hit_first_and_last_stops() {
        let low = elevation_color(0.0).to_color_u8();
        let high = elevation_color(1.0).to_color_u8();
        assert_eq!((low.red(), low.green(), low.blue()), (0x2c, 0x7b, 0xb6));
        assert_eq!((high.red(), high.green(), high.blue()), (0xe0, 0x3a, 0x2f));
    }

    #[test]
    fn out_of_range_is_clamped() {
        assert_eq!(elevation_color(-1.0), elevation_color(0.0));
        assert_eq!(elevation_color(2.0), elevation_color(1.0));
    }
}
