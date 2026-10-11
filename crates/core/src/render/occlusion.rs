//! Pixel-exact hiding of the 3D track behind terrain: each kind of track
//! part has its depth rasterized and compared with the terrain's per pixel,
//! giving a mask to draw that part through.

use tiny_skia::Mask;

use crate::terrain::DepthMap;

/// A screen point with its view depth: `(x, y, depth)`.
pub(crate) type DepthPoint = (f64, f64, f64);

/// The nearest track surface per pixel, as `1 / depth` (`0` where none).
pub(crate) struct NearDepth {
    width: usize,
    height: usize,
    inverse: Vec<f32>,
    /// Pixels covered so far: `x0..x1`, `y0..y1` (empty while `x0 >= x1`).
    bounds: (usize, usize, usize, usize),
}

impl NearDepth {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width as usize, height as usize);
        Self {
            width,
            height,
            inverse: vec![0.0; width * height],
            bounds: (width, height, 0, 0),
        }
    }

    /// Cover the triangle `tri`, keeping each pixel's nearest depth.
    /// `1 / depth` is affine in screen space, so it interpolates exactly.
    pub(crate) fn fill_triangle(&mut self, tri: [DepthPoint; 3]) {
        let [a, b, c] = tri.map(|(x, y, d)| (x, y, 1.0 / d));
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-9 || !area.is_finite() {
            return;
        }
        let clamp_x = |v: f64| v.clamp(0.0, self.width as f64) as usize;
        let clamp_y = |v: f64| v.clamp(0.0, self.height as f64) as usize;
        let (x0, x1) = (
            clamp_x(a.0.min(b.0).min(c.0).floor()),
            clamp_x(a.0.max(b.0).max(c.0).ceil()),
        );
        let (y0, y1) = (
            clamp_y(a.1.min(b.1).min(c.1).floor()),
            clamp_y(a.1.max(b.1).max(c.1).ceil()),
        );
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let (bx0, by0, bx1, by1) = self.bounds;
        self.bounds = (bx0.min(x0), by0.min(y0), bx1.max(x1), by1.max(y1));
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let wa = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) / area;
                let wb = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) / area;
                let wc = 1.0 - wa - wb;
                if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                    continue;
                }
                let inverse = (wa * a.2 + wb * b.2 + wc * c.2) as f32;
                let at = &mut self.inverse[y * self.width + x];
                *at = at.max(inverse);
            }
        }
    }

    /// Cover the quad `corners` (in order around it).
    pub(crate) fn fill_quad(&mut self, corners: [DepthPoint; 4]) {
        self.fill_triangle([corners[0], corners[1], corners[2]]);
        self.fill_triangle([corners[0], corners[2], corners[3]]);
    }

    /// Give each uncovered pixel next to covered ones the nearest depth among
    /// its neighbours, so antialiased edges drawn just outside the covered
    /// shapes are judged with them.
    pub(crate) fn grow(&mut self) {
        let (w, h) = (self.width, self.height);
        let (x0, y0, x1, y1) = self.bounds;
        if x0 >= x1 {
            return;
        }
        let (x0, y0, x1, y1) = (
            x0.saturating_sub(1),
            y0.saturating_sub(1),
            (x1 + 1).min(w),
            (y1 + 1).min(h),
        );
        let source = self.inverse.clone();
        for y in y0..y1 {
            for x in x0..x1 {
                if source[y * w + x] > 0.0 {
                    continue;
                }
                let mut nearest = 0.0f32;
                for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        nearest = nearest.max(source[ny * w + nx]);
                    }
                }
                self.inverse[y * w + x] = nearest;
            }
        }
        self.bounds = (x0, y0, x1, y1);
    }

    /// Masks over the pixels where `terrain` leaves this part in sight and
    /// where it hides it, to draw the part's pixels through (uncovered
    /// pixels count as in sight), or `None` when nothing of it is hidden.
    pub(crate) fn masks(&self, terrain: &DepthMap) -> Option<(Mask, Mask)> {
        let (x0, y0, x1, y1) = self.bounds;
        let hidden_pixels: Vec<usize> = (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter_map(|(x, y)| {
                let i = y * self.width + x;
                let inverse = self.inverse[i] as f64;
                let behind =
                    inverse > 0.0 && terrain.hides(x as f64 + 0.5, y as f64 + 0.5, 1.0 / inverse);
                behind.then_some(i)
            })
            .collect();
        if hidden_pixels.is_empty() {
            return None;
        }
        let (w, h) = (self.width as u32, self.height as u32);
        let (mut seen, mut hidden) = (Mask::new(w, h)?, Mask::new(w, h)?);
        seen.data_mut().fill(0xff);
        for i in hidden_pixels {
            seen.data_mut()[i] = 0;
            hidden.data_mut()[i] = 0xff;
        }
        Some((seen, hidden))
    }

    /// Cover a stroke `width` pixels wide from `a` to `b`, with room for its
    /// round ends and antialiased edges.
    pub(crate) fn fill_stroke(&mut self, a: DepthPoint, b: DepthPoint, width: f64) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = dx.hypot(dy);
        let (ux, uy) = if length > 1e-9 {
            (dx / length, dy / length)
        } else {
            (1.0, 0.0)
        };
        let r = width / 2.0 + 1.5;
        let (nx, ny) = (-uy * r, ux * r);
        let (ex, ey) = (ux * r, uy * r);
        self.fill_quad([
            (a.0 - ex + nx, a.1 - ey + ny, a.2),
            (b.0 + ex + nx, b.1 + ey + ny, b.2),
            (b.0 + ex - nx, b.1 + ey - ny, b.2),
            (a.0 - ex - nx, a.1 - ey - ny, a.2),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(near: &NearDepth, x: usize, y: usize) -> f32 {
        near.inverse[y * near.width + x]
    }

    #[test]
    fn keeps_the_nearest_depth_and_interpolates_across_a_triangle() {
        let mut near = NearDepth::new(20, 20);
        near.fill_quad([
            (0.0, 0.0, 10.0),
            (20.0, 0.0, 10.0),
            (20.0, 20.0, 20.0),
            (0.0, 20.0, 20.0),
        ]);
        assert!((1.0 / at(&near, 5, 0) - 10.0).abs() < 0.5);
        assert!(at(&near, 5, 19) < at(&near, 5, 0), "farther down");
        let before = at(&near, 10, 10);
        near.fill_quad([
            (5.0, 5.0, 100.0),
            (15.0, 5.0, 100.0),
            (15.0, 15.0, 100.0),
            (5.0, 15.0, 100.0),
        ]);
        assert_eq!(
            at(&near, 10, 10),
            before,
            "a farther surface doesn't replace it"
        );
        let below = at(&near, 10, 15);
        near.fill_stroke((2.0, 10.0, 1.0), (18.0, 10.0, 1.0), 2.0);
        assert_eq!(at(&near, 10, 10), 1.0);
        assert_eq!(at(&near, 10, 15), below, "the stroke stays narrow");
    }

    #[test]
    fn growing_reaches_one_pixel_past_the_covered_ones() {
        let mut near = NearDepth::new(10, 10);
        near.inverse[5 * 10 + 5] = 0.5;
        near.bounds = (5, 5, 6, 6);
        near.grow();
        assert_eq!(at(&near, 4, 4), 0.5);
        assert_eq!(at(&near, 6, 5), 0.5);
        assert_eq!(at(&near, 3, 5), 0.0);
    }
}
