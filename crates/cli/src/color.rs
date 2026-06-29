use tiny_skia::Color;

/// Parse a `#RGB`, `#RRGGBB`, or `#RRGGBBAA` hex string into a color.
pub fn parse_hex_color(s: &str) -> Result<Color, String> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    let (r, g, b, a) = match hex.len() {
        3 => {
            let v = u16::from_str_radix(hex, 16).map_err(|_| invalid(s))?;
            let r = ((v >> 8) & 0xf) as u8;
            let g = ((v >> 4) & 0xf) as u8;
            let b = (v & 0xf) as u8;
            (r * 0x11, g * 0x11, b * 0x11, 0xff)
        }
        6 => {
            let v = u32::from_str_radix(hex, 16).map_err(|_| invalid(s))?;
            ((v >> 16) as u8, (v >> 8) as u8, v as u8, 0xff)
        }
        8 => {
            let v = u32::from_str_radix(hex, 16).map_err(|_| invalid(s))?;
            ((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)
        }
        _ => return Err(invalid(s)),
    };
    Ok(Color::from_rgba8(r, g, b, a))
}

fn invalid(s: &str) -> String {
    format!("invalid hex color: {s}")
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
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_hex_color("#xyz").is_err());
        assert!(parse_hex_color("12345").is_err());
    }
}
