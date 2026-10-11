//! Layout of labels that follow a line on screen (road and river names):
//! where each glyph goes along a polyline, and which way it turns.

use std::f32::consts::PI;

/// Steepest turn between neighbouring glyphs of a line label (radians);
/// sharper bends would crumple the text.
const MAX_BEND: f32 = PI / 6.0;

/// Arc length of `line` at each of its points.
fn lengths(line: &[(f32, f32)]) -> Vec<f32> {
    let mut total = 0.0;
    let mut out = Vec::with_capacity(line.len());
    out.push(0.0);
    for pair in line.windows(2) {
        total += (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1);
        out.push(total);
    }
    out
}

/// Total length of `line`.
pub(super) fn length(line: &[(f32, f32)]) -> f32 {
    lengths(line).last().copied().unwrap_or(0.0)
}

/// The point at arc length `s` along `line` and the direction there.
fn point_at(line: &[(f32, f32)], lengths: &[f32], s: f32) -> (f32, f32, f32) {
    let i = lengths
        .partition_point(|&l| l <= s)
        .clamp(1, line.len().max(2) - 1);
    let (a, b) = (line[i - 1], line[i]);
    let span = (lengths[i] - lengths[i - 1]).max(f32::EPSILON);
    let t = ((s - lengths[i - 1]) / span).clamp(0.0, 1.0);
    let angle = (b.1 - a.1).atan2(b.0 - a.0);
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, angle)
}

/// Glyph centers and angles for glyphs `advances` wide, centered at arc
/// length `center` of `line`, reading left to right (the line is walked
/// backwards when it runs right to left). `None` when the text runs off the
/// line or a bend under it is too sharp.
pub(super) fn layout(
    line: &[(f32, f32)],
    advances: &[f32],
    center: f32,
) -> Option<Vec<(f32, f32, f32)>> {
    let place = |line: &[(f32, f32)], center: f32| -> Option<Vec<(f32, f32, f32)>> {
        let lengths = lengths(line);
        let total = *lengths.last()?;
        let width: f32 = advances.iter().sum();
        let start = center - width / 2.0;
        if line.len() < 2 || start < 0.0 || start + width > total {
            return None;
        }
        let mut at = start;
        let mut glyphs = Vec::with_capacity(advances.len());
        for &advance in advances {
            glyphs.push(point_at(line, &lengths, at + advance / 2.0));
            at += advance;
        }
        let smooth = glyphs.windows(2).all(|g| {
            let turn = (g[1].2 - g[0].2 + PI).rem_euclid(2.0 * PI) - PI;
            turn.abs() <= MAX_BEND
        });
        smooth.then_some(glyphs)
    };
    let glyphs = place(line, center)?;
    let heading: (f32, f32) = glyphs
        .iter()
        .fold((0.0, 0.0), |(x, y), g| (x + g.2.cos(), y + g.2.sin()));
    if heading.0 >= 0.0 {
        return Some(glyphs);
    }
    let reversed: Vec<(f32, f32)> = line.iter().rev().copied().collect();
    place(&reversed, length(line) - center)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_follow_the_line_and_read_left_to_right() {
        let line = [(0.0, 10.0), (100.0, 10.0)];
        let glyphs = layout(&line, &[10.0, 10.0, 10.0], 50.0).unwrap();
        for (g, x) in glyphs.iter().zip([40.0, 50.0, 60.0]) {
            assert!((g.0 - x).abs() < 1e-3, "{glyphs:?}");
        }
        assert!(glyphs.iter().all(|g| g.1 == 10.0 && g.2 == 0.0));
        // The same line drawn right to left is read left to right too.
        let back = [(100.0, 10.0), (0.0, 10.0)];
        let glyphs = layout(&back, &[10.0, 10.0, 10.0], 50.0).unwrap();
        assert!(glyphs.windows(2).all(|g| g[1].0 > g[0].0));
        assert!(glyphs.iter().all(|g| g.2.abs() < 1e-6));
    }

    #[test]
    fn rejects_text_that_overruns_or_bends_too_sharply() {
        let line = [(0.0, 0.0), (20.0, 0.0)];
        assert!(layout(&line, &[10.0, 10.0, 10.0], 10.0).is_none());
        let elbow = [(0.0, 0.0), (50.0, 0.0), (50.0, 50.0)];
        assert!(layout(&elbow, &[10.0; 4], 50.0).is_none());
        let gentle = [(0.0, 0.0), (50.0, 0.0), (100.0, 10.0)];
        assert!(layout(&gentle, &[10.0; 4], 50.0).is_some());
    }
}
