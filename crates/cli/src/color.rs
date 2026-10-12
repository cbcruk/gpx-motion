//! Command-line color values: hex strings parsed for the color flags.

use gpx_motion_core::color::parse_hex;
use tiny_skia::Color;

/// Parse a `#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA` hex string into a
/// color; the `#` is optional.
pub fn parse_hex_color(s: &str) -> Result<Color, String> {
    let digits = s.strip_prefix('#').unwrap_or(s);
    parse_hex(&format!("#{digits}")).ok_or_else(|| {
        format!("invalid hex color: {s} (expected #RGB, #RGBA, #RRGGBB, or #RRGGBBAA)")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_digit() {
        let c = parse_hex_color("#ff5a36").unwrap();
        assert_eq!(
            (c.red(), c.green(), c.blue()),
            (1.0, 0x5a as f32 / 255.0, 0x36 as f32 / 255.0)
        );
    }

    #[test]
    fn parses_without_hash_and_alpha() {
        let c = parse_hex_color("808080").unwrap();
        assert_eq!(c.alpha(), 1.0);
        let t = parse_hex_color("#00000080").unwrap();
        assert!((t.alpha() - 0x80 as f32 / 255.0).abs() < 1e-6);
        let short = parse_hex_color("fff8").unwrap();
        assert_eq!(short.to_color_u8().alpha(), 0x88);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_hex_color("#xyz").is_err());
        assert!(parse_hex_color("12345").is_err());
        assert!(parse_hex_color("##fff").is_err());
        assert!(parse_hex_color("+12345").is_err());
    }
}
