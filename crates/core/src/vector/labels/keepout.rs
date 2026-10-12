//! Screen regions labels keep off, and a grid for fast overlap tests.

use tiny_skia::Rect;

use crate::geo::clip::{clip_segment, contains, Edges};

/// What labels must not cover.
#[derive(Debug, Clone, Default)]
pub struct Keepout {
    /// Screen rectangles (overlays, panels).
    pub rects: Vec<Rect>,
    /// Track polylines in screen points, widened by `track_half_width`.
    pub tracks: Vec<Vec<(f32, f32)>>,
    /// Half the drawn track width plus a margin, in pixels.
    pub track_half_width: f32,
    /// Filled screen polygons (e.g. 3D curtains under the track); a label
    /// may neither cross their edges nor sit inside them.
    pub areas: Vec<Vec<(f32, f32)>>,
}

impl Keepout {
    /// Keep labels off overlay `rects`, the drawn `tracks` (screen polylines
    /// `track_width` wide, plus a margin), and filled `areas` such as 3D
    /// curtains.
    pub fn around(
        rects: &[Rect],
        tracks: Vec<Vec<(f32, f32)>>,
        areas: Vec<Vec<(f32, f32)>>,
        track_width: f32,
        pixel_ratio: f32,
    ) -> Self {
        Self {
            rects: rects.to_vec(),
            tracks,
            track_half_width: track_width / 2.0 + 2.0 * pixel_ratio,
            areas,
        }
    }
}

pub(super) fn center(r: &Rect) -> (f32, f32) {
    ((r.left() + r.right()) / 2.0, (r.top() + r.bottom()) / 2.0)
}

pub(super) fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.left() < b.right() && b.left() < a.right() && a.top() < b.bottom() && b.top() < a.bottom()
}

/// Edge length in pixels of the [`KeepoutGrid`] cells.
const CELL_PX: f32 = 32.0;

/// A keepout's track segments and areas bucketed by the canvas cells their
/// bounding boxes cover (segments grown by the track half-width), so each
/// label tests only the shapes near it. For finite screen coordinates of a
/// sane size it gives the same answers as testing every shape, since a shape
/// can only touch a label its bounding box overlaps.
pub(super) struct KeepoutGrid<'a> {
    keepout: &'a Keepout,
    cols: usize,
    rows: usize,
    /// Per cell: `(track, segment start)` pairs.
    segments: Vec<Vec<(usize, usize)>>,
    /// Per cell: area indices.
    areas: Vec<Vec<usize>>,
}

impl<'a> KeepoutGrid<'a> {
    pub(super) fn new(keepout: &'a Keepout, canvas: (f32, f32)) -> Self {
        let cols = ((canvas.0 / CELL_PX).ceil() as usize).max(1);
        let rows = ((canvas.1 / CELL_PX).ceil() as usize).max(1);
        let mut grid = Self {
            keepout,
            cols,
            rows,
            segments: vec![Vec::new(); cols * rows],
            areas: vec![Vec::new(); cols * rows],
        };
        let w = keepout.track_half_width;
        for (t, track) in keepout.tracks.iter().enumerate() {
            for (i, s) in track.windows(2).enumerate() {
                let (a, b) = (s[0], s[1]);
                let bbox = (
                    a.0.min(b.0) - w,
                    a.1.min(b.1) - w,
                    a.0.max(b.0) + w,
                    a.1.max(b.1) + w,
                );
                for cell in grid.cells(bbox) {
                    grid.segments[cell].push((t, i));
                }
            }
        }
        for (i, area) in keepout.areas.iter().enumerate() {
            let bbox = area.iter().fold(
                (
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ),
                |(x0, y0, x1, y1), &(x, y)| (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            );
            for cell in grid.cells(bbox) {
                grid.areas[cell].push(i);
            }
        }
        grid
    }

    /// Indices of the cells overlapping the box `(x0, y0, x1, y1)`, clamped
    /// to the canvas: none for a box off it, every cell along an axis with
    /// a NaN corner (conservative).
    fn cells(&self, (x0, y0, x1, y1): (f32, f32, f32, f32)) -> Vec<usize> {
        let span = |lo: f32, hi: f32, n: usize| {
            let first = (lo / CELL_PX).floor().max(0.0);
            let last = (hi / CELL_PX).floor().min(n as f32 - 1.0);
            (first <= last).then_some((first as usize, last as usize))
        };
        let (Some((c0, c1)), Some((r0, r1))) = (span(x0, x1, self.cols), span(y0, y1, self.rows))
        else {
            return Vec::new();
        };
        (r0..=r1)
            .flat_map(|r| (c0..=c1).map(move |c| r * self.cols + c))
            .collect()
    }

    /// Whether a track (grown by its half-width) or an area touches `bounds`.
    pub(super) fn touches(&self, bounds: &Rect) -> bool {
        let w = self.keepout.track_half_width;
        let grown = (
            bounds.left() - w,
            bounds.top() - w,
            bounds.right() + w,
            bounds.bottom() + w,
        );
        let rect = (bounds.left(), bounds.top(), bounds.right(), bounds.bottom());
        self.cells(rect).into_iter().any(|cell| {
            self.segments[cell].iter().any(|&(t, i)| {
                let track = &self.keepout.tracks[t];
                crosses(grown, track[i], track[i + 1])
            }) || self.areas[cell]
                .iter()
                .any(|&a| touches_area(bounds, &self.keepout.areas[a]))
        })
    }
}

/// Whether `bounds` crosses the polygon's edges or lies inside it.
fn touches_area(bounds: &Rect, area: &[(f32, f32)]) -> bool {
    if area.len() < 3 {
        return false;
    }
    let rect = (bounds.left(), bounds.top(), bounds.right(), bounds.bottom());
    let mut edges = area.iter().zip(area.iter().cycle().skip(1));
    edges.any(|(&a, &b)| crosses(rect, a, b)) || contains(area, center(bounds))
}

/// Whether segment `a`–`b` passes through or touches the rectangle
/// `(x0, y0, x1, y1)` (Liang–Barsky clipping, [`Edges::Closed`]).
fn crosses((x0, y0, x1, y1): (f32, f32, f32, f32), a: (f32, f32), b: (f32, f32)) -> bool {
    clip_segment(a, b, [x0, y0, x1, y1], Edges::Closed).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepout_grid_matches_testing_every_shape() {
        // A small deterministic generator, so the test needs no dependency.
        let mut seed = 0x2545_f491_u32;
        let mut next = move |lo: f32, hi: f32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            lo + (seed as f32 / u32::MAX as f32) * (hi - lo)
        };
        let canvas = (300.0, 200.0);
        let tracks: Vec<Vec<(f32, f32)>> = (0..3)
            .map(|_| {
                (0..40)
                    .map(|_| (next(-50.0, 350.0), next(-50.0, 250.0)))
                    .collect()
            })
            .collect();
        let areas: Vec<Vec<(f32, f32)>> = (0..8)
            .map(|_| {
                let (x, y) = (next(-20.0, 320.0), next(-20.0, 220.0));
                (0..4)
                    .map(|_| (x + next(-15.0, 15.0), y + next(-15.0, 15.0)))
                    .collect()
            })
            .collect();
        let keepout = Keepout::around(&[], tracks, areas, 1.0, 0.5);
        let grid = KeepoutGrid::new(&keepout, canvas);
        let brute = |b: &Rect| {
            let w = keepout.track_half_width;
            let grown = (b.left() - w, b.top() - w, b.right() + w, b.bottom() + w);
            keepout
                .tracks
                .iter()
                .flat_map(|t| t.windows(2))
                .any(|s| crosses(grown, s[0], s[1]))
                || keepout.areas.iter().any(|a| touches_area(b, a))
        };
        let (mut hits, mut misses) = (0, 0);
        for _ in 0..2000 {
            let (w, h) = (next(1.0, 12.0), next(1.0, 8.0));
            let Some(b) = Rect::from_xywh(next(0.0, 300.0 - w), next(0.0, 200.0 - h), w, h) else {
                continue;
            };
            let expected = brute(&b);
            assert_eq!(grid.touches(&b), expected, "{b:?}");
            if expected {
                hits += 1;
            } else {
                misses += 1;
            }
        }
        assert!(hits > 100 && misses > 100, "{hits} hits, {misses} misses");
    }
}
