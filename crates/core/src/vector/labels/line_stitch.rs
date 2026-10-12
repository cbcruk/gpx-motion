//! Named lines cut at tile edges, put back together: each tile's piece is
//! clipped to the tile (tiles overlap by a buffer), and pieces of the same
//! name that meet at an edge are joined, so a road gets one label course.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::geo::clip::{self, Edges};

/// Parts of the tile-normalized line `points` inside the tile (`0..=1` on
/// both axes), each at least one segment long.
pub(super) fn clip_to_tile(points: &[(f64, f64)]) -> Vec<Vec<(f64, f64)>> {
    let mut parts: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    for pair in points.windows(2) {
        let Some((a, b)) = clip_segment(pair[0], pair[1]) else {
            if current.len() > 1 {
                parts.push(std::mem::take(&mut current));
            }
            current.clear();
            continue;
        };
        if current.last() != Some(&a) {
            if current.len() > 1 {
                parts.push(std::mem::take(&mut current));
            }
            current = vec![a];
        }
        current.push(b);
        if b != pair[1] {
            parts.push(std::mem::take(&mut current));
        }
    }
    if current.len() > 1 {
        parts.push(current);
    }
    parts
}

/// The part of segment `a`–`b` inside the unit square (Liang–Barsky), or
/// `None` when it misses it or touches it in a single point.
///
/// The square is [`Edges::HalfOpen`]: a segment running along the right or
/// bottom edge belongs to the next tile (which has it on its left or top
/// edge), so the stitched line doesn't hold it twice.
fn clip_segment(a: (f64, f64), b: (f64, f64)) -> Option<((f64, f64), (f64, f64))> {
    let (t0, t1) = clip::clip_segment(a, b, [0.0, 0.0, 1.0, 1.0], Edges::HalfOpen)?;
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let at = |t: f64| {
        if t == 0.0 {
            a
        } else if t == 1.0 {
            b
        } else {
            (a.0 + dx * t, a.1 + dy * t)
        }
    };
    Some((at(t0), at(t1)))
}

/// A line to join: its points, and what must match for two to join.
pub(super) trait Stitch: Clone {
    /// What each original piece contributes to a joined line (kept from
    /// every piece joined).
    type Mark: Clone;
    /// Same name, kind, and zoom.
    fn group(&self) -> (u64, u8, u64);
    fn marks(&self) -> &[Self::Mark];
    fn points(&self) -> &[(f64, f64)];
    /// How far apart two meeting ends may be.
    fn tolerance(&self) -> f64;
    /// Whether the line ends at its tile's edge (start, end), where it may
    /// continue in the next tile.
    fn open(&self) -> (bool, bool);
    /// This line with `points` and the `marks` of every piece in it instead
    /// (open where those ends are).
    fn with_points(
        &self,
        points: Vec<(f64, f64)>,
        open: (bool, bool),
        marks: Vec<Self::Mark>,
    ) -> Self;
}

/// `lines` with pieces that meet end to end at tile edges joined into one;
/// lines with no open end pass through as they are.
pub(super) fn stitch<'a, L: Stitch + 'a>(
    lines: impl IntoIterator<Item = &'a L>,
) -> Vec<Cow<'a, L>> {
    let mut out: Vec<Cow<'a, L>> = Vec::new();
    let mut groups: HashMap<(u64, u8, u64), Vec<&'a L>> = HashMap::new();
    for line in lines {
        let (a, b) = line.open();
        if a || b {
            groups.entry(line.group()).or_default().push(line);
        } else {
            out.push(Cow::Borrowed(line));
        }
    }
    for pieces in groups.into_values() {
        if pieces.len() == 1 {
            out.push(Cow::Borrowed(pieces[0]));
            continue;
        }
        out.extend(join(&pieces).into_iter().map(Cow::Owned));
    }
    out
}

/// Chains of `pieces` joined where their open ends meet.
fn join<L: Stitch>(pieces: &[&L]) -> Vec<L> {
    let tolerance = pieces[0].tolerance();
    let cell = |p: (f64, f64)| {
        (
            (p.0 / tolerance).round() as i64,
            (p.1 / tolerance).round() as i64,
        )
    };
    // Open ends by grid cell: (piece, whether it's the start).
    let mut ends: HashMap<(i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for (i, piece) in pieces.iter().enumerate() {
        let (pts, (a, b)) = (piece.points(), piece.open());
        if a {
            ends.entry(cell(pts[0])).or_default().push((i, true));
        }
        if b {
            ends.entry(cell(pts[pts.len() - 1]))
                .or_default()
                .push((i, false));
        }
    }
    let mut used = vec![false; pieces.len()];
    // The unused piece with an open end meeting `at`, and whether that end
    // is its start.
    let partner = |at: (f64, f64), used: &[bool]| -> Option<(usize, bool)> {
        let (cx, cy) = cell(at);
        (cx - 1..=cx + 1)
            .flat_map(|x| (cy - 1..=cy + 1).map(move |y| (x, y)))
            .filter_map(|c| ends.get(&c))
            .flatten()
            .copied()
            .find(|&(i, start)| {
                let pts = pieces[i].points();
                let end = if start { pts[0] } else { pts[pts.len() - 1] };
                !used[i] && (end.0 - at.0).hypot(end.1 - at.1) <= tolerance
            })
    };
    let mut chains = Vec::new();
    for i in 0..pieces.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut points = pieces[i].points().to_vec();
        let mut marks = pieces[i].marks().to_vec();
        let (mut start_open, mut end_open) = pieces[i].open();
        // Forward from the tail…
        while end_open {
            let Some((next, from_start)) = partner(points[points.len() - 1], &used) else {
                break;
            };
            used[next] = true;
            marks.extend_from_slice(pieces[next].marks());
            let mut more = pieces[next].points().to_vec();
            let (a, b) = pieces[next].open();
            if !from_start {
                more.reverse();
            }
            end_open = if from_start { b } else { a };
            points.extend(more.into_iter().skip(1));
        }
        // …and back from the head.
        while start_open {
            let Some((prev, from_start)) = partner(points[0], &used) else {
                break;
            };
            used[prev] = true;
            marks.extend_from_slice(pieces[prev].marks());
            let mut more = pieces[prev].points().to_vec();
            let (a, b) = pieces[prev].open();
            if from_start {
                more.reverse();
            }
            start_open = if from_start { b } else { a };
            more.pop();
            more.extend(points);
            points = more;
        }
        chains.push(pieces[i].with_points(points, (start_open, end_open), marks));
    }
    chains
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_the_parts_inside_the_tile() {
        let parts = clip_to_tile(&[(-0.5, 0.5), (0.5, 0.5), (0.5, 1.5)]);
        assert_eq!(parts, vec![vec![(0.0, 0.5), (0.5, 0.5), (0.5, 1.0)]]);
        // Leaving and coming back makes two parts.
        let parts = clip_to_tile(&[(0.2, 0.2), (1.5, 0.2), (1.5, 0.8), (0.2, 0.8)]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], vec![(0.2, 0.2), (1.0, 0.2)]);
        assert_eq!(parts[1], vec![(1.0, 0.8), (0.2, 0.8)]);
        assert!(clip_to_tile(&[(1.5, 0.5), (2.0, 0.5)]).is_empty());
        // Along the right edge: the next tile's; along the left: this one's.
        assert!(clip_to_tile(&[(1.0, 0.2), (1.0, 0.8)]).is_empty());
        assert_eq!(clip_to_tile(&[(0.0, 0.2), (0.0, 0.8)]).len(), 1);
    }

    #[derive(Clone, Debug, PartialEq)]
    struct Line {
        name: u64,
        points: Vec<(f64, f64)>,
        open: (bool, bool),
        marks: Vec<u32>,
    }

    impl Stitch for Line {
        type Mark = u32;
        fn group(&self) -> (u64, u8, u64) {
            (self.name, 0, 0)
        }
        fn marks(&self) -> &[u32] {
            &self.marks
        }
        fn points(&self) -> &[(f64, f64)] {
            &self.points
        }
        fn tolerance(&self) -> f64 {
            0.5
        }
        fn open(&self) -> (bool, bool) {
            self.open
        }
        fn with_points(
            &self,
            points: Vec<(f64, f64)>,
            open: (bool, bool),
            marks: Vec<u32>,
        ) -> Self {
            Line {
                points,
                open,
                marks,
                ..self.clone()
            }
        }
    }

    fn line(name: u64, points: Vec<(f64, f64)>, open: (bool, bool)) -> Line {
        Line {
            name,
            points,
            open,
            marks: Vec::new(),
        }
    }

    fn marked(mark: u32, points: Vec<(f64, f64)>, open: (bool, bool)) -> Line {
        Line {
            marks: vec![mark],
            ..line(1, points, open)
        }
    }

    #[test]
    fn clipping_cuts_a_line_crossing_the_whole_tile_at_both_edges() {
        let parts = clip_to_tile(&[(-1.0, 0.5), (2.0, 0.5)]);
        assert_eq!(parts, vec![vec![(0.0, 0.5), (1.0, 0.5)]]);
    }

    #[test]
    fn clipping_drops_corner_touches_and_lines_too_short_to_clip() {
        assert!(clip_to_tile(&[(-0.5, 0.5), (0.5, -0.5)]).is_empty());
        assert!(clip_to_tile(&[(0.5, 0.5)]).is_empty());
        assert!(clip_to_tile(&[]).is_empty());
    }

    #[test]
    fn clipping_keeps_inside_points_exact() {
        let points = [(0.1, 0.1), (0.3, 0.7), (0.9, 0.2)];
        assert_eq!(clip_to_tile(&points), vec![points.to_vec()]);
    }

    #[test]
    fn a_lone_open_piece_is_passed_through_unchanged() {
        let pieces = [line(1, vec![(0.0, 0.0), (100.0, 0.0)], (false, true))];
        let joined = stitch(pieces.iter());
        assert_eq!(joined.len(), 1);
        assert!(matches!(joined[0], Cow::Borrowed(_)));
    }

    #[test]
    fn joined_lines_keep_every_piece_marks_and_their_outer_open_ends() {
        let pieces = [
            marked(10, vec![(0.0, 0.0), (100.0, 0.0)], (true, true)),
            marked(20, vec![(100.0, 0.0), (200.0, 0.0)], (true, true)),
        ];
        let joined = stitch(pieces.iter());
        assert_eq!(joined.len(), 1);
        let mut marks = joined[0].marks.clone();
        marks.sort_unstable();
        assert_eq!(marks, [10, 20]);
        assert_eq!(joined[0].points.len(), 3);
        // Still open at both far ends: the line may go on in other tiles.
        assert_eq!(joined[0].open, (true, true));
    }

    #[test]
    fn ends_farther_apart_than_the_tolerance_stay_apart() {
        let pieces = [
            marked(10, vec![(0.0, 0.0), (100.0, 0.0)], (false, true)),
            marked(20, vec![(100.6, 0.0), (200.0, 0.0)], (true, false)),
        ];
        let joined = stitch(pieces.iter());
        assert_eq!(joined.len(), 2);
        assert!(joined.iter().all(|l| l.points.len() == 2));
        assert!(joined.iter().all(|l| l.marks.len() == 1));
    }

    #[test]
    fn only_open_ends_are_joined() {
        // The second piece starts where the first ends, but that start is
        // closed (it begins inside its tile), so nothing joins.
        let pieces = [
            marked(10, vec![(0.0, 0.0), (100.0, 0.0)], (false, true)),
            marked(20, vec![(100.0, 0.0), (200.0, 0.0)], (false, true)),
        ];
        assert_eq!(stitch(pieces.iter()).len(), 2);
    }

    #[test]
    fn a_loop_of_pieces_joins_once_and_stops() {
        let pieces = [
            marked(10, vec![(0.0, 0.0), (100.0, 0.0)], (true, true)),
            marked(20, vec![(100.0, 0.0), (0.0, 0.0)], (true, true)),
        ];
        let joined = stitch(pieces.iter());
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].points, [(0.0, 0.0), (100.0, 0.0), (0.0, 0.0)]);
        assert_eq!(joined[0].marks.len(), 2);
    }

    #[test]
    fn pieces_meeting_at_edges_join_in_order() {
        let pieces = [
            // Middle piece first, drawn backwards.
            line(1, vec![(200.0, 0.0), (100.0, 0.0)], (true, true)),
            line(1, vec![(0.0, 0.0), (100.2, 0.0)], (false, true)),
            line(1, vec![(200.1, 0.0), (300.0, 0.0)], (true, false)),
            // Same place, other name: kept apart.
            line(2, vec![(100.0, 0.0), (150.0, 0.0)], (true, false)),
            // Closed: passes through.
            line(1, vec![(0.0, 50.0), (10.0, 50.0)], (false, false)),
        ];
        let joined = stitch(pieces.iter());
        assert_eq!(joined.len(), 3);
        let main = joined
            .iter()
            .find(|l| l.name == 1 && l.points.len() > 2)
            .unwrap();
        // Either way along, every piece once, in order.
        let mut xs: Vec<f64> = main.points.iter().map(|p| p.0.round()).collect();
        if xs[0] > xs[xs.len() - 1] {
            xs.reverse();
        }
        assert_eq!(xs, vec![0.0, 100.0, 200.0, 300.0]);
        assert_eq!(main.open, (false, false));
        assert!(joined.iter().any(|l| l.name == 2));
        // Two pieces both drawn away from where they meet.
        let outward = [
            line(3, vec![(100.0, 0.0), (0.0, 0.0)], (true, false)),
            line(3, vec![(100.0, 0.0), (200.0, 0.0)], (true, false)),
        ];
        let joined = stitch(outward.iter());
        assert_eq!(joined.len(), 1);
        let mut xs: Vec<f64> = joined[0].points.iter().map(|p| p.0).collect();
        if xs[0] > xs[xs.len() - 1] {
            xs.reverse();
        }
        assert_eq!(xs, vec![0.0, 100.0, 200.0]);
    }
}
