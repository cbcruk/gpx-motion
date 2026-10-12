//! Clipping and containment for flat geometry: rings cut to half-planes and
//! rectangles (Sutherland–Hodgman), segments cut to rectangles
//! (Liang–Barsky), and the even-odd point-in-polygon test.
//!
//! Rectangles are `[left, top, right, bottom]` with y down, as in tile and
//! screen space.

use std::ops::{Add, Div, Mul, Neg, Sub};

/// A float the segment and containment helpers work in ([`f32`] or
/// [`f64`]), so screen-space callers keep their single precision.
pub(crate) trait Scalar:
    Copy
    + PartialOrd
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
{
    /// Zero.
    const ZERO: Self;
    /// One.
    const ONE: Self;
    /// The larger of `self` and `other`, as the float type's own `max`.
    fn max(self, other: Self) -> Self;
    /// The smaller of `self` and `other`, as the float type's own `min`.
    fn min(self, other: Self) -> Self;
}

impl Scalar for f32 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    fn max(self, other: Self) -> Self {
        f32::max(self, other)
    }
    fn min(self, other: Self) -> Self {
        f32::min(self, other)
    }
}

impl Scalar for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    fn max(self, other: Self) -> Self {
        f64::max(self, other)
    }
    fn min(self, other: Self) -> Self {
        f64::min(self, other)
    }
}

/// Which points on a rectangle's border count as inside it, for
/// [`clip_segment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edges {
    /// The closed rectangle: every edge is inside, and a segment that only
    /// touches the rectangle in a single point still clips to that point.
    ///
    /// For drawing and hit tests. A map tile strokes the lines on its own
    /// border, since a line along an edge shared with a neighbouring tile
    /// must be drawn by at least one of them; and a label's box that merely
    /// touches a keep-out segment counts as hitting it.
    Closed,
    /// A segment lying along the right or bottom edge is outside, one along
    /// the left or top edge is inside, and a segment that meets the
    /// rectangle in a single point clips to nothing (a zero-length segment
    /// follows its point).
    ///
    /// For pieces stitched back together across tiles. A segment running
    /// along an edge two tiles share then belongs only to the tile with it
    /// on its left or top edge, so the stitched line doesn't hold it twice.
    HalfOpen,
}

/// The parameter range `(t0, t1)` of segment `a`–`b` (`a` at `0`, `b` at
/// `1`) inside `rect` (Liang–Barsky).
///
/// Returns `None` when the segment misses the rectangle under the `edges`
/// rule. A zero-length segment is kept whole (`(0, 1)`) when its point is
/// inside.
pub(crate) fn clip_segment<T: Scalar>(
    a: (T, T),
    b: (T, T),
    rect: [T; 4],
    edges: Edges,
) -> Option<(T, T)> {
    let [left, top, right, bottom] = rect;
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (T::ZERO, T::ONE);
    for (p, q, far) in [
        (-dx, a.0 - left, false),
        (dx, right - a.0, true),
        (-dy, a.1 - top, false),
        (dy, bottom - a.1, true),
    ] {
        if p == T::ZERO {
            let on_far_edge = far && q == T::ZERO && edges == Edges::HalfOpen;
            if q < T::ZERO || on_far_edge {
                return None;
            }
        } else if p < T::ZERO {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    let empty = match edges {
        Edges::Closed => t0 > t1,
        Edges::HalfOpen => t0 >= t1,
    };
    (!empty).then_some((t0, t1))
}

/// Clip a closed ring to one half-plane (one Sutherland–Hodgman pass).
///
/// `inside` says which side a point is on; `crossing(prev, cur)` is the
/// point where the edge from `prev` (on one side) to `cur` (on the other)
/// meets the boundary. The result keeps the input's winding.
pub(crate) fn clip_ring<P: Copy>(
    ring: &[P],
    inside: impl Fn(P) -> bool,
    crossing: impl Fn(P, P) -> P,
) -> Vec<P> {
    let mut out = Vec::with_capacity(ring.len() + 2);
    for (i, &cur) in ring.iter().enumerate() {
        let prev = ring[(i + ring.len() - 1) % ring.len()];
        let (cur_in, prev_in) = (inside(cur), inside(prev));
        if cur_in != prev_in {
            out.push(crossing(prev, cur));
        }
        if cur_in {
            out.push(cur);
        }
    }
    out
}

/// Keep a ring's part inside the closed rectangle `rect`
/// (Sutherland–Hodgman, one [`clip_ring`] pass per edge).
///
/// The result may hold zero-area slivers along the rectangle's edges where
/// a concave ring leaves and re-enters it.
pub(crate) fn clip_ring_to_rect(ring: &[(f64, f64)], rect: [f64; 4]) -> Vec<(f64, f64)> {
    let mut points = ring.to_vec();
    let edges: [(usize, f64, bool); 4] = [
        (0, rect[0], true),
        (0, rect[2], false),
        (1, rect[1], true),
        (1, rect[3], false),
    ];
    for (axis, bound, lower) in edges {
        let coord = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };
        let inside = |p: (f64, f64)| {
            if lower {
                coord(p) >= bound
            } else {
                coord(p) <= bound
            }
        };
        let crossing = |a: (f64, f64), b: (f64, f64)| {
            let t = (bound - coord(a)) / (coord(b) - coord(a));
            (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
        };
        points = clip_ring(&points, inside, crossing);
        if points.is_empty() {
            break;
        }
    }
    points
}

/// Whether `ring` holds `point` (even-odd rule; the ring closes itself).
///
/// Points on the border fall either way by the usual crossing rule: an
/// edge spans the rows from its smaller-y end up to but not including its
/// larger-y end, and toggles the result only when strictly right of the point.
pub(crate) fn contains<T: Scalar>(ring: &[(T, T)], (x, y): (T, T)) -> bool {
    let mut inside = false;
    for (&(ax, ay), &(bx, by)) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        if (ay > y) != (by > y) && x < ax + (y - ay) / (by - ay) * (bx - ax) {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: [f64; 4] = [0.0, 0.0, 1.0, 1.0];
    const BOTH: [Edges; 2] = [Edges::Closed, Edges::HalfOpen];

    #[test]
    fn a_segment_along_the_right_edge_belongs_to_the_closed_rect_only() {
        let crossing = clip_segment((-1.0, 0.5), (1.0, 0.5), UNIT, Edges::Closed);
        assert_eq!(crossing, Some((0.5, 1.0)));
        // Drawn by this tile, stitched by its neighbour.
        assert!(clip_segment((1.0, 0.2), (1.0, 0.8), UNIT, Edges::Closed).is_some());
        assert!(clip_segment((1.0, 0.2), (1.0, 0.8), UNIT, Edges::HalfOpen).is_none());
    }

    #[test]
    fn a_ring_is_cut_to_the_rect_corner_it_overlaps() {
        let square = [(0.5, 0.5), (1.5, 0.5), (1.5, 1.5), (0.5, 1.5)];
        let kept = clip_ring_to_rect(&square, UNIT);
        assert_eq!(kept, [(0.5, 1.0), (0.5, 0.5), (1.0, 0.5), (1.0, 1.0)]);
    }

    #[test]
    fn a_notched_ring_leaves_out_its_notch() {
        let notched = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (2.0, 2.0), (0.0, 4.0)];
        assert!(contains(&notched, (1.0, 1.0)));
        assert!(!contains(&notched, (2.0, 3.0)));
    }

    #[test]
    fn segment_inside_is_kept_whole_and_outside_is_dropped() {
        for edges in BOTH {
            assert_eq!(
                clip_segment((0.2, 0.3), (0.7, 0.9), UNIT, edges),
                Some((0.0, 1.0))
            );
            assert_eq!(clip_segment((1.5, 0.5), (2.0, 0.5), UNIT, edges), None);
            assert_eq!(clip_segment((-1.0, 2.0), (2.0, 1.5), UNIT, edges), None);
        }
    }

    #[test]
    fn crossing_segment_is_cut_at_the_edges() {
        for edges in BOTH {
            assert_eq!(
                clip_segment((-1.0, 0.5), (3.0, 0.5), UNIT, edges),
                Some((0.25, 0.5))
            );
            assert_eq!(
                clip_segment((0.5, 0.5), (0.5, 2.5), UNIT, edges),
                Some((0.0, 0.25))
            );
        }
    }

    #[test]
    fn segments_on_the_near_edges_are_kept_under_both_rules() {
        for edges in BOTH {
            let left = clip_segment((0.0, 0.2), (0.0, 0.8), UNIT, edges);
            let top = clip_segment((0.2, 0.0), (0.8, 0.0), UNIT, edges);
            assert_eq!(left, Some((0.0, 1.0)), "{edges:?}");
            assert_eq!(top, Some((0.0, 1.0)), "{edges:?}");
        }
    }

    #[test]
    fn segments_on_the_far_edges_are_kept_only_when_closed() {
        let right = ((1.0, 0.2), (1.0, 0.8));
        let bottom = ((0.2, 1.0), (0.8, 1.0));
        for (a, b) in [right, bottom] {
            assert_eq!(clip_segment(a, b, UNIT, Edges::Closed), Some((0.0, 1.0)));
            assert_eq!(clip_segment(a, b, UNIT, Edges::HalfOpen), None);
        }
    }

    #[test]
    fn single_point_contacts_are_kept_only_when_closed() {
        let ends_on_left = ((-1.0, 0.5), (0.0, 0.5));
        let through_corner = ((-0.5, 0.5), (0.5, -0.5));
        for (a, b) in [ends_on_left, through_corner] {
            assert!(clip_segment(a, b, UNIT, Edges::Closed).is_some());
            assert_eq!(clip_segment(a, b, UNIT, Edges::HalfOpen), None);
        }
    }

    #[test]
    fn zero_length_segments_follow_their_point() {
        for edges in BOTH {
            let p = (0.4, 0.6);
            assert_eq!(clip_segment(p, p, UNIT, edges), Some((0.0, 1.0)));
            let out = (1.5, 0.5);
            assert_eq!(clip_segment(out, out, UNIT, edges), None);
            let near = (0.0, 0.5);
            assert_eq!(clip_segment(near, near, UNIT, edges), Some((0.0, 1.0)));
        }
        let far = (1.0, 0.5);
        assert_eq!(
            clip_segment(far, far, UNIT, Edges::Closed),
            Some((0.0, 1.0))
        );
        assert_eq!(clip_segment(far, far, UNIT, Edges::HalfOpen), None);
        // Ending on the right edge from inside keeps the end point.
        assert_eq!(
            clip_segment((0.5, 0.5), (1.0, 0.5), UNIT, Edges::HalfOpen),
            Some((0.0, 1.0))
        );
    }

    #[test]
    fn segment_clipping_works_in_single_precision() {
        let rect = [10.0f32, 10.0, 20.0, 20.0];
        let t = clip_segment((0.0f32, 15.0), (40.0, 15.0), rect, Edges::Closed);
        assert_eq!(t, Some((0.25, 0.5)));
    }

    #[test]
    fn square_partly_outside_is_cut_to_the_rect() {
        let square = [(-0.5, 0.25), (0.5, 0.25), (0.5, 0.75), (-0.5, 0.75)];
        assert_eq!(
            clip_ring_to_rect(&square, UNIT),
            [(0.0, 0.25), (0.5, 0.25), (0.5, 0.75), (0.0, 0.75)]
        );
        let corner = [(0.5, 0.5), (1.5, 0.5), (1.5, 1.5), (0.5, 1.5)];
        assert_eq!(
            clip_ring_to_rect(&corner, UNIT),
            [(0.5, 1.0), (0.5, 0.5), (1.0, 0.5), (1.0, 1.0)]
        );
    }

    #[test]
    fn rings_inside_or_outside_the_rect_are_kept_or_dropped_whole() {
        let inside = [(0.2, 0.2), (0.8, 0.2), (0.5, 0.8)];
        assert_eq!(clip_ring_to_rect(&inside, UNIT), inside.to_vec());
        let outside = [(2.0, 2.0), (3.0, 2.0), (2.5, 3.0)];
        assert!(clip_ring_to_rect(&outside, UNIT).is_empty());
        assert!(clip_ring_to_rect(&[], UNIT).is_empty());
    }

    #[test]
    fn half_plane_pass_inserts_the_crossings() {
        let ring = [0.0f64, 3.0, -1.0];
        let kept = clip_ring(&ring, |v| v >= 1.0, |_, _| 1.0);
        assert_eq!(kept, [1.0, 3.0, 1.0]);
    }

    #[test]
    fn point_in_polygon_inside_outside_and_concave() {
        let square = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)];
        assert!(contains(&square, (1.0, 1.0)));
        assert!(!contains(&square, (3.0, 1.0)));
        assert!(!contains(&square, (1.0, -0.5)));
        let notched = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (2.0, 2.0), (0.0, 4.0)];
        assert!(contains(&notched, (0.5, 3.0)));
        assert!(contains(&notched, (3.5, 3.0)));
        assert!(!contains(&notched, (2.0, 3.0)));
        let screen = [(0.0f32, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(contains(&screen, (5.0, 5.0)));
        assert!(!contains(&screen, (15.0, 5.0)));
    }

    #[test]
    fn degenerate_rings_hold_nothing() {
        assert!(!contains::<f64>(&[], (0.0, 0.0)));
        assert!(!contains(&[(0.0, 0.0), (1.0, 1.0)], (0.5, 0.5)));
    }
}
