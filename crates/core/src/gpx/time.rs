//! Minimal RFC 3339 / ISO 8601 timestamp parsing for GPX `<time>` values.
//!
//! GPX timestamps are rigidly RFC 3339 (e.g. `2024-01-01T00:00:00Z`), so a
//! focused parser keeps the core dependency-free and `wasm32`-clean instead of
//! pulling in a full date-time crate.

/// Parse an RFC 3339 timestamp into milliseconds since the Unix epoch (UTC).
///
/// Accepts an optional fractional second and a `Z` or `±HH:MM` offset.
pub fn parse_epoch_ms(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }

    let year: i64 = s.get(0..4)?.parse().ok()?;
    expect(bytes, 4, b'-')?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    expect(bytes, 7, b'-')?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    if !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    expect(bytes, 13, b':')?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    expect(bytes, 16, b':')?;
    let second: i64 = s.get(17..19)?.parse().ok()?;

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut rest = &s[19..];

    let mut millis_frac: i64 = 0;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits: String = after_dot
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            return None;
        }
        let mut frac = String::with_capacity(3);
        for i in 0..3 {
            frac.push(digits.as_bytes().get(i).copied().unwrap_or(b'0') as char);
        }
        millis_frac = frac.parse().ok()?;
        rest = &rest[1 + digits.len()..];
    }

    // Timezone offset in minutes (UTC == 0).
    let offset_min: i64 = match rest.as_bytes().first() {
        Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            expect(rest.as_bytes(), 3, b':')?;
            let om: i64 = rest.get(4..6)?.parse().ok()?;
            let mag = oh * 60 + om;
            if *sign == b'-' {
                -mag
            } else {
                mag
            }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_min * 60;
    Some(secs * 1_000 + millis_frac)
}

fn expect(bytes: &[u8], idx: usize, ch: u8) -> Option<()> {
    (bytes.get(idx) == Some(&ch)).then_some(())
}

/// Days since 1970-01-01 (Howard Hinnant's civil-from-days inverse).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_origin_is_zero() {
        assert_eq!(parse_epoch_ms("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn known_utc_instant() {
        // 2024-01-01T00:00:00Z == 1704067200 s.
        assert_eq!(
            parse_epoch_ms("2024-01-01T00:00:00Z"),
            Some(1_704_067_200_000)
        );
    }

    #[test]
    fn fractional_seconds() {
        assert_eq!(
            parse_epoch_ms("2024-01-01T00:00:00.250Z"),
            Some(1_704_067_200_250)
        );
        assert_eq!(
            parse_epoch_ms("2024-01-01T00:00:00.5Z"),
            Some(1_704_067_200_500)
        );
    }

    #[test]
    fn timezone_offset_applied() {
        let z = parse_epoch_ms("2024-01-01T09:00:00Z").unwrap();
        let plus9 = parse_epoch_ms("2024-01-01T18:00:00+09:00").unwrap();
        assert_eq!(z, plus9);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_epoch_ms("not-a-time"), None);
        assert_eq!(parse_epoch_ms("2024-13-01T00:00:00Z"), None);
        assert_eq!(parse_epoch_ms(""), None);
    }
}
