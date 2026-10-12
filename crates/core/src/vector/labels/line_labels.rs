//! Layout of labels that follow a line on screen (road and river names):
//! where each glyph goes along a polyline, and which way it turns.

use std::f32::consts::PI;

/// Steepest turn between neighbouring glyphs of a line label (radians);
/// sharper bends would crumple the text.
const MAX_BEND: f32 = PI / 6.0;
/// Largest total turn across one label.
pub(super) const MAX_TOTAL_BEND: f32 = PI / 3.0;
/// Largest total turn across a label in a shaped script (joined letters
/// don't bend around corners, and the browser draws such labels whole).
pub(super) const SHAPED_MAX_TOTAL_BEND: f32 = PI / 12.0;

/// A screen polyline with its arc lengths.
pub(super) struct Polyline {
    pub points: Vec<(f32, f32)>,
    /// Arc length at each point.
    pub(crate) lengths: Vec<f32>,
}

/// Where a glyph sits: its center and angle (radians), and the segment of
/// the polyline under it (index of its first point, fraction along it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Glyph {
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub segment: usize,
    pub t: f32,
}

impl Polyline {
    pub(super) fn new(points: Vec<(f32, f32)>) -> Self {
        let mut total = 0.0;
        let mut lengths = Vec::with_capacity(points.len());
        lengths.push(0.0);
        for pair in points.windows(2) {
            total += (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1);
            lengths.push(total);
        }
        lengths.truncate(points.len());
        Self { points, lengths }
    }

    /// Total length.
    pub(super) fn length(&self) -> f32 {
        self.lengths.last().copied().unwrap_or(0.0)
    }

    /// The glyph spot at arc length `s`, or `None` on a line of under two
    /// points.
    fn at(&self, s: f32) -> Option<Glyph> {
        let i = crate::math::segment_at(&self.lengths, s)? + 1;
        let (a, b) = (self.points[i - 1], self.points[i]);
        let span = (self.lengths[i] - self.lengths[i - 1]).max(f32::EPSILON);
        let t = ((s - self.lengths[i - 1]) / span).clamp(0.0, 1.0);
        Some(Glyph {
            x: a.0 + (b.0 - a.0) * t,
            y: a.1 + (b.1 - a.1) * t,
            angle: (b.1 - a.1).atan2(b.0 - a.0),
            segment: i - 1,
            t,
        })
    }

    /// The same line walked the other way.
    fn reversed(&self) -> Self {
        Self::new(self.points.iter().rev().copied().collect())
    }

    /// Glyphs `advances` wide centered at arc length `center`, reading left
    /// to right (walking the line backwards when it runs right to left);
    /// segments always index this polyline's own order. `None` when the
    /// text runs off the line, a bend under it is too sharp, or it turns
    /// more than `max_total_bend` (radians) in all.
    pub(super) fn layout(
        &self,
        advances: &[f32],
        center: f32,
        max_total_bend: f32,
    ) -> Option<Vec<Glyph>> {
        let glyphs = self.place(advances, center, max_total_bend)?;
        let heading: f32 = glyphs.iter().map(|g| g.angle.cos()).sum();
        if heading >= 0.0 {
            return Some(glyphs);
        }
        let last = self.points.len().checked_sub(2)?;
        let back = self
            .reversed()
            .place(advances, self.length() - center, max_total_bend)?;
        Some(
            back.into_iter()
                .map(|g| Glyph {
                    segment: last - g.segment,
                    t: 1.0 - g.t,
                    ..g
                })
                .collect(),
        )
    }

    fn place(&self, advances: &[f32], center: f32, max_total_bend: f32) -> Option<Vec<Glyph>> {
        let width: f32 = advances.iter().sum();
        let start = center - width / 2.0;
        if self.points.len() < 2 || start < 0.0 || start + width > self.length() {
            return None;
        }
        let mut at = start;
        let mut glyphs = Vec::with_capacity(advances.len());
        for &advance in advances {
            glyphs.push(self.at(at + advance / 2.0)?);
            at += advance;
        }
        let turns: Vec<f32> = glyphs
            .windows(2)
            .map(|g| (g[1].angle - g[0].angle + PI).rem_euclid(2.0 * PI) - PI)
            .collect();
        let smooth = turns.iter().all(|t| t.abs() <= MAX_BEND)
            && turns.iter().sum::<f32>().abs() <= max_total_bend;
        smooth.then_some(glyphs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_follow_the_line_and_read_left_to_right() {
        let line = Polyline::new(vec![(0.0, 10.0), (100.0, 10.0)]);
        let glyphs = line
            .layout(&[10.0, 10.0, 10.0], 50.0, MAX_TOTAL_BEND)
            .unwrap();
        for (g, x) in glyphs.iter().zip([40.0, 50.0, 60.0]) {
            assert!((g.x - x).abs() < 1e-3, "{glyphs:?}");
            assert!((g.t - x / 100.0).abs() < 1e-3 && g.segment == 0);
        }
        assert!(glyphs.iter().all(|g| g.y == 10.0 && g.angle == 0.0));
        // The same line drawn right to left is read left to right too, and
        // its segments still index the line as given.
        let back = Polyline::new(vec![(100.0, 10.0), (0.0, 10.0)]);
        let glyphs = back
            .layout(&[10.0, 10.0, 10.0], 50.0, MAX_TOTAL_BEND)
            .unwrap();
        assert!(glyphs.windows(2).all(|g| g[1].x > g[0].x));
        assert!(glyphs.iter().all(|g| g.angle.abs() < 1e-6));
        assert!((glyphs[0].t - 0.6).abs() < 1e-3, "{glyphs:?}");
    }

    #[test]
    fn lines_of_under_two_points_lay_out_nothing() {
        for points in [vec![], vec![(5.0, 5.0)]] {
            let line = Polyline::new(points);
            assert_eq!(line.at(0.0), None);
            assert!(line.layout(&[1.0], 0.0, MAX_TOTAL_BEND).is_none());
        }
        // A line of repeated points has length zero: nothing fits on it.
        let dot = Polyline::new(vec![(5.0, 5.0), (5.0, 5.0)]);
        assert!(dot.at(0.0).is_some());
        assert!(dot.layout(&[1.0], 0.0, MAX_TOTAL_BEND).is_none());
    }

    #[test]
    fn rejects_text_that_overruns_or_bends_too_sharply() {
        let line = Polyline::new(vec![(0.0, 0.0), (20.0, 0.0)]);
        assert!(line
            .layout(&[10.0, 10.0, 10.0], 10.0, MAX_TOTAL_BEND)
            .is_none());
        let elbow = Polyline::new(vec![(0.0, 0.0), (50.0, 0.0), (50.0, 50.0)]);
        assert!(elbow.layout(&[10.0; 4], 50.0, MAX_TOTAL_BEND).is_none());
        let gentle = Polyline::new(vec![(0.0, 0.0), (50.0, 0.0), (100.0, 10.0)]);
        assert!(gentle.layout(&[10.0; 4], 50.0, MAX_TOTAL_BEND).is_some());
        // Each step gentle, but turning too far in all.
        let arc: Vec<(f32, f32)> = (0..=12)
            .map(|i| {
                let a = i as f32 * PI / 24.0;
                (100.0 * a.sin(), 100.0 - 100.0 * a.cos())
            })
            .collect();
        let arc = Polyline::new(arc);
        let total = arc.length();
        assert!(arc
            .layout(&[total / 12.0; 11], total / 2.0, MAX_TOTAL_BEND)
            .is_none());
        // A 20° bend is fine for most text but too much for shaped text.
        let bent = Polyline::new(vec![(0.0, 0.0), (50.0, 0.0), (100.0, 18.2)]);
        assert!(bent.layout(&[10.0; 4], 50.0, MAX_TOTAL_BEND).is_some());
        assert!(bent
            .layout(&[10.0; 4], 50.0, SHAPED_MAX_TOTAL_BEND)
            .is_none());
        let straight = Polyline::new(vec![(0.0, 0.0), (100.0, 0.0)]);
        assert!(straight
            .layout(&[10.0; 4], 50.0, SHAPED_MAX_TOTAL_BEND)
            .is_some());
    }
}
