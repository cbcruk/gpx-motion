//! Pixel-exact hiding of the 3D track behind terrain: the track's depth is
//! rasterized beside its pixels and compared with the terrain's per pixel.

use tiny_skia::Pixmap;

use crate::terrain::mesh::over;
use crate::terrain::DepthMap;

/// A screen point with its view depth: `(x, y, depth)`.
pub(crate) type DepthPoint = (f64, f64, f64);

/// The nearest track surface per pixel, as `1 / depth` (`0` where none).
pub(crate) struct NearDepth {
    width: usize,
    height: usize,
    inverse: Vec<f32>,
}

impl NearDepth {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width as usize, height as usize);
        Self {
            width,
            height,
            inverse: vec![0.0; width * height],
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

/// Draw `front` onto `target` where the track is in sight and `behind`
/// where `terrain` hides it, deciding per pixel from the track's `near`
/// depth. Pixels without a track depth (stray antialiasing) count as seen.
pub(crate) fn composite(
    target: &mut Pixmap,
    front: &Pixmap,
    behind: &Pixmap,
    near: &NearDepth,
    terrain: &DepthMap,
) {
    let width = target.width() as usize;
    let pixels = target.pixels_mut();
    for (i, (f, b)) in front.pixels().iter().zip(behind.pixels()).enumerate() {
        if f.alpha() == 0 && b.alpha() == 0 {
            continue;
        }
        let inverse = near.inverse[i] as f64;
        let (x, y) = ((i % width) as f64 + 0.5, (i / width) as f64 + 0.5);
        let hidden = inverse > 0.0 && terrain.hides(x, y, 1.0 / inverse);
        pixels[i] = over(if hidden { *b } else { *f }, pixels[i]);
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
}
