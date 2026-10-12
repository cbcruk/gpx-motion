//! Color helpers shared by the renderers and the style parsers: alpha
//! adjustment, hex and CSS color parsing, and hex serialization.

use tiny_skia::Color;

/// `color` with its alpha replaced by `alpha`.
///
/// An `alpha` outside `0..=1` (or NaN) leaves `color` unchanged.
pub(crate) fn with_alpha(color: Color, alpha: f32) -> Color {
    Color::from_rgba(color.red(), color.green(), color.blue(), alpha).unwrap_or(color)
}

/// `color` with its alpha multiplied by `factor`.
///
/// A product outside `0..=1` (or NaN) leaves `color` unchanged.
pub(crate) fn scale_alpha(color: Color, factor: f32) -> Color {
    with_alpha(color, color.alpha() * factor)
}

/// Parse a hex color: `#rgb`, `#rgba`, `#rrggbb`, or `#rrggbbaa`.
///
/// Digits are case-insensitive and the leading `#` is required, so this is
/// `None` without it, with a non-hex digit, or with any other length.
///
/// # Example
///
/// ```
/// use gpx_motion_core::color::parse_hex;
/// use tiny_skia::Color;
///
/// assert_eq!(parse_hex("#ff8000"), Some(Color::from_rgba8(255, 128, 0, 255)));
/// assert_eq!(parse_hex("#F80"), parse_hex("#ff8800"));
/// assert_eq!(parse_hex("ff8000"), None);
/// ```
pub fn parse_hex(text: &str) -> Option<Color> {
    let digits: Vec<u8> = text
        .strip_prefix('#')?
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    let single = |i: usize| digits[i] * 17;
    let pair = |i: usize| digits[i] * 16 + digits[i + 1];
    let (r, g, b, a) = match digits.len() {
        3 => (single(0), single(1), single(2), 0xff),
        4 => (single(0), single(1), single(2), single(3)),
        6 => (pair(0), pair(2), pair(4), 0xff),
        8 => (pair(0), pair(2), pair(4), pair(6)),
        _ => return None,
    };
    Some(Color::from_rgba8(r, g, b, a))
}

/// Parse a CSS color: any [`parse_hex`] form, `rgb()`, `rgba()`, `hsl()`,
/// `hsla()`, or a common name.
///
/// Case and surrounding whitespace are ignored; channel values out of range
/// are clamped.
pub(crate) fn parse_css(text: &str) -> Option<Color> {
    let text = text.trim().to_ascii_lowercase();
    if text.starts_with('#') {
        return parse_hex(&text);
    }
    if let Some((name, args)) = text.strip_suffix(')').and_then(|t| t.split_once('(')) {
        let parts: Vec<&str> = args
            .split([',', ' ', '/'])
            .filter(|p| !p.is_empty())
            .collect();
        let number = |p: &str| -> Option<f32> {
            match p.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().ok().map(|v| v / 100.0),
                None => p.parse().ok(),
            }
        };
        let alpha = match parts.get(3) {
            Some(a) => number(a)?.clamp(0.0, 1.0),
            None => 1.0,
        };
        return match (name.trim(), parts.len()) {
            ("rgb" | "rgba", 3 | 4) => {
                let channel = |p: &str| -> Option<f32> {
                    if p.ends_with('%') {
                        number(p)
                    } else {
                        p.parse::<f32>().ok().map(|v| v / 255.0)
                    }
                };
                let (r, g, b) = (channel(parts[0])?, channel(parts[1])?, channel(parts[2])?);
                Color::from_rgba(
                    r.clamp(0.0, 1.0),
                    g.clamp(0.0, 1.0),
                    b.clamp(0.0, 1.0),
                    alpha,
                )
            }
            ("hsl" | "hsla", 3 | 4) => {
                let h = parts[0].trim_end_matches("deg").parse::<f32>().ok()?;
                let (s, l) = (number(parts[1])?, number(parts[2])?);
                let (r, g, b) = hsl(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
                Color::from_rgba(r, g, b, alpha)
            }
            _ => None,
        };
    }
    let (r, g, b, a) = match text.as_str() {
        "transparent" => (0, 0, 0, 0),
        "black" => (0, 0, 0, 255),
        "white" => (255, 255, 255, 255),
        "gray" | "grey" => (128, 128, 128, 255),
        "silver" => (192, 192, 192, 255),
        "red" => (255, 0, 0, 255),
        "green" => (0, 128, 0, 255),
        "blue" => (0, 0, 255, 255),
        "yellow" => (255, 255, 0, 255),
        "orange" => (255, 165, 0, 255),
        "purple" => (128, 0, 128, 255),
        "navy" => (0, 0, 128, 255),
        "teal" => (0, 128, 128, 255),
        "olive" => (128, 128, 0, 255),
        "maroon" => (128, 0, 0, 255),
        "lime" => (0, 255, 0, 255),
        "aqua" | "cyan" => (0, 255, 255, 255),
        "fuchsia" | "magenta" => (255, 0, 255, 255),
        _ => return None,
    };
    Some(Color::from_rgba8(r, g, b, a))
}

/// Format `color` as `#rrggbb`, or `#rrggbbaa` when translucent.
pub(crate) fn to_hex(color: Color) -> String {
    let c = color.to_color_u8();
    if c.alpha() == 0xff {
        format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            c.red(),
            c.green(),
            c.blue(),
            c.alpha()
        )
    }
}

/// RGB (`0..=1`) of a hue in degrees and saturation/lightness in `0..=1`.
fn hsl(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::proptest_config;
    use proptest::prelude::*;

    fn rgba8(c: Color) -> (u8, u8, u8, u8) {
        let c = c.to_color_u8();
        (c.red(), c.green(), c.blue(), c.alpha())
    }

    fn hex(text: &str) -> Option<(u8, u8, u8, u8)> {
        parse_hex(text).map(rgba8)
    }

    #[test]
    fn parses_all_four_hex_lengths() {
        assert_eq!(hex("#abc"), Some((0xaa, 0xbb, 0xcc, 0xff)));
        assert_eq!(hex("#abc8"), Some((0xaa, 0xbb, 0xcc, 0x88)));
        assert_eq!(hex("#ff5a36"), Some((0xff, 0x5a, 0x36, 0xff)));
        assert_eq!(hex("#00000080"), Some((0, 0, 0, 0x80)));
    }

    #[test]
    fn hex_digits_ignore_case() {
        assert_eq!(hex("#ABCDEF"), hex("#abcdef"));
        assert_eq!(hex("#AbC"), hex("#abc"));
        assert_eq!(hex("#FfFfFf80"), Some((0xff, 0xff, 0xff, 0x80)));
    }

    #[test]
    fn rejects_invalid_hex() {
        for bad in [
            "",
            "#",
            "abc",
            "ff5a36",
            "#ab",
            "#abcde",
            "#abcdefg",
            "#abcdef0",
            "#abcdef012",
            "#xyz",
            "#12345z",
            "#+12345",
            " #abc",
            "#ab c",
        ] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn parses_css_colors() {
        let c = |t| rgba8(parse_css(t).unwrap());
        assert_eq!(c(" #ABC "), (0xaa, 0xbb, 0xcc, 0xff));
        assert_eq!(c("#abc8").3, 0x88);
        assert_eq!(c("rgb(10, 20, 30)"), (10, 20, 30, 255));
        assert_eq!(c("rgba(10,20,30,0.5)").3, 128);
        assert_eq!(c("hsl(0, 100%, 50%)"), (255, 0, 0, 255));
        assert_eq!(c("hsla(120, 100%, 25%, 1)"), (0, 128, 0, 255));
        assert_eq!(c("White"), (255, 255, 255, 255));
        assert!(parse_css("chartreuse-ish").is_none());
    }

    #[test]
    fn hex_round_trips() {
        for text in ["#123456", "#12345678"] {
            assert_eq!(to_hex(parse_hex(text).unwrap()), text);
        }
        assert_eq!(to_hex(parse_hex("#abcf").unwrap()), "#aabbcc");
    }

    #[test]
    fn alpha_is_replaced_or_scaled() {
        let half = Color::from_rgba8(10, 20, 30, 128);
        assert_eq!(with_alpha(half, 1.0).alpha(), 1.0);
        assert_eq!(rgba8(with_alpha(half, 0.0)).3, 0);
        assert!((scale_alpha(half, 0.5).alpha() - half.alpha() * 0.5).abs() < 1e-6);
        assert_eq!(with_alpha(half, 1.5), half);
        assert_eq!(scale_alpha(half, f32::NAN), half);
    }

    proptest! {
        #![proptest_config(proptest_config())]

        #[test]
        fn full_hex_round_trips_to_lowercase(text in "#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})") {
            let lower = text.to_ascii_lowercase();
            let expected = lower.strip_suffix("ff").filter(|t| t.len() == 7).unwrap_or(&lower);
            prop_assert_eq!(parse_hex(&text).map(to_hex), Some(expected.to_string()));
        }

        #[test]
        fn short_hex_survives_a_round_trip(text in "#([0-9a-fA-F]{3}|[0-9a-fA-F]{4})") {
            let color = parse_hex(&text);
            prop_assert!(color.is_some());
            prop_assert_eq!(color.map(to_hex).as_deref().and_then(parse_hex), color);
        }

        #[test]
        fn hex_parses_exactly_the_four_lengths(text in "\\PC{0,10}") {
            let digits = text.strip_prefix('#').filter(|d| d.chars().all(|c| c.is_ascii_hexdigit()));
            let valid = digits.is_some_and(|d| matches!(d.len(), 3 | 4 | 6 | 8));
            prop_assert_eq!(parse_hex(&text).is_some(), valid);
        }

        #[test]
        fn css_parsing_never_panics(
            text in "\\PC{0,24}|(rgba?|hsla?)\\(((-?[0-9]{0,4}(\\.[0-9]{1,3})?%?|nan|inf|-inf|[0-9]+deg)[ ,/]{1,2}){0,5}\\)",
        ) {
            let _ = parse_css(&text);
        }
    }
}
