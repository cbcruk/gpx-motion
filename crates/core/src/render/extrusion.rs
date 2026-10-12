//! Extruded buildings drawn through a perspective camera on the CPU.

use tiny_skia::{Pixmap, PremultipliedColorU8};

use crate::geo::camera::{Camera, Projector, ViewPoint, NEAR};
use crate::terrain::DepthMap;
use crate::vector::extrusion::Facet;
use crate::vector::render::clip_ring;

/// Samples per pixel side; edges are antialiased by averaging them.
const SAMPLES: usize = 2;
/// Output rows drawn at a time, bounding the sample buffers.
const BAND_ROWS: usize = 32;

/// A facet on screen, set up for filling samples: `x` and `y` in sample
/// units.
struct Triangle {
    /// Edge functions `ex·x + ey·y + e0`, non-negative inside.
    edges: [(f64, f64, f64); 3],
    /// `1 / depth` as `a·x + b·y + c`.
    plane: (f64, f64, f64),
    /// Sample rows `[lo, hi)` it may cover.
    rows: (usize, usize),
    /// Opaque `0xffrrggbb`.
    packed: u32,
}

impl Triangle {
    /// The triangle through `points` with `1 / depth` values `inverse`, or
    /// `None` when it has no area.
    fn new(points: [(f64, f64); 3], inverse: [f64; 3], rgb: [u8; 3]) -> Option<Self> {
        let [a, b, c] = points;
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area == 0.0 || !area.is_finite() {
            return None;
        }
        // Each edge function is the barycentric weight of the opposite vertex.
        let edge = |p: (f64, f64), q: (f64, f64)| {
            let (ex, ey) = ((p.1 - q.1) / area, (q.0 - p.0) / area);
            (ex, ey, -(ex * p.0 + ey * p.1))
        };
        let edges = [edge(b, c), edge(c, a), edge(a, b)];
        let mix = |f: fn(&(f64, f64, f64)) -> f64| {
            edges
                .iter()
                .zip(inverse)
                .map(|(e, i)| f(e) * i)
                .sum::<f64>()
        };
        let plane = (mix(|e| e.0), mix(|e| e.1), mix(|e| e.2));
        let lo = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
        let hi = a.1.max(b.1).max(c.1).ceil().max(0.0) as usize;
        let packed =
            0xff00_0000 | u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
        Some(Self {
            edges,
            plane,
            rows: (lo, hi),
            packed,
        })
    }
}

/// Draw each layer of building `facets` (with its opacity) as `camera`
/// sees them, hidden behind whatever `depth` already holds nearer (the
/// terrain), and record their own depth there for what is drawn later.
///
/// A layer is drawn opaque and then blended at its opacity, so its faces
/// never show through one another; later layers are hidden by earlier ones.
pub fn draw_buildings(
    pixmap: &mut Pixmap,
    camera: &Camera,
    layers: &[(&[Facet], f32)],
    depth: &mut DepthMap,
) {
    let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
    if depth.width != width || depth.height != height {
        return;
    }
    let grid = SampleGrid {
        projector: camera.projector(),
        eye: camera.eye(),
        width,
        height,
        bands: height.div_ceil(BAND_ROWS),
    };
    let layers: Vec<BuildingLayer> = layers
        .iter()
        .filter_map(|&(facets, opacity)| grid.layer(facets, opacity))
        .collect();
    if layers.is_empty() {
        return;
    }
    let (sw, band_h) = (width * SAMPLES, BAND_ROWS * SAMPLES);
    let mut buffers = BandBuffers {
        near: vec![0f32; sw * band_h],
        color: vec![0u32; sw * band_h],
    };
    let pixels = pixmap.pixels_mut();
    for band in 0..grid.bands {
        grid.draw_band(band, &layers, pixels, depth, &mut buffers);
    }
}

/// One layer of building triangles on screen, and per band the ones
/// reaching into it.
struct BuildingLayer {
    tris: Vec<Triangle>,
    /// Indices into `tris` per band of [`BAND_ROWS`] output rows.
    binned: Vec<Vec<u32>>,
    opacity: f32,
}

/// One band's sample buffers: nearest `1 / depth` and opaque color.
struct BandBuffers {
    near: Vec<f32>,
    color: Vec<u32>,
}

/// The camera and the output pixmap's samples, in bands of [`BAND_ROWS`]
/// rows, that [`draw_buildings`] draws onto.
struct SampleGrid {
    projector: Projector,
    eye: (f64, f64, f64),
    /// Output size in pixels.
    width: usize,
    height: usize,
    bands: usize,
}

impl SampleGrid {
    /// The triangles of `facets` on screen, binned into bands, or `None`
    /// when none shows.
    fn layer(&self, facets: &[Facet], opacity: f32) -> Option<BuildingLayer> {
        let band_h = BAND_ROWS * SAMPLES;
        let mut tris = Vec::new();
        let mut binned = vec![Vec::new(); self.bands];
        for facet in facets {
            self.triangulate(facet, |tri| {
                let index = tris.len() as u32;
                let first_band = (tri.rows.0 / band_h).min(self.bands - 1);
                let last_band = (tri.rows.1.saturating_sub(1) / band_h).min(self.bands - 1);
                for band in &mut binned[first_band..=last_band] {
                    band.push(index);
                }
                tris.push(tri);
            });
        }
        (!tris.is_empty()).then(|| BuildingLayer {
            tris,
            binned,
            opacity: opacity.clamp(0.0, 1.0),
        })
    }

    /// Pass each on-screen triangle of `facet` to `emit`, unless it is a
    /// wall seen from behind: clipped at the near plane and fanned out from
    /// its first corner.
    fn triangulate(&self, facet: &Facet, mut emit: impl FnMut(Triangle)) {
        let (projector, eye) = (&self.projector, self.eye);
        let (sw, sh) = (self.width * SAMPLES, self.height * SAMPLES);
        if let Some([nx, ny]) = facet.outward {
            let [x, y, _] = facet.corners[0];
            if (eye.0 - x) * nx + (eye.1 - y) * ny <= 0.0 {
                return;
            }
        }
        let view = facet.corners.map(|[x, y, h]| projector.view(x, y, h));
        let clipped;
        let ring: &[ViewPoint] = if view.iter().all(|v| v.depth >= NEAR) {
            &view
        } else {
            clipped = clip_ring(&view);
            &clipped
        };
        if ring.len() < 3 {
            return;
        }
        let rgb = {
            let c = facet.color.to_color_u8();
            [c.red(), c.green(), c.blue()]
        };
        let at = |v: ViewPoint| {
            let (x, y) = projector.to_screen(v);
            let scale = SAMPLES as f64;
            ((x * scale, y * scale), 1.0 / v.depth.max(NEAR))
        };
        let first = at(ring[0]);
        for pair in ring[1..].windows(2) {
            let (b, c) = (at(pair[0]), at(pair[1]));
            let points = [first.0, b.0, c.0];
            let outside = points.iter().all(|p| p.0 < 0.0)
                || points.iter().all(|p| p.0 > sw as f64)
                || points.iter().all(|p| p.1 < 0.0)
                || points.iter().all(|p| p.1 > sh as f64);
            if outside {
                continue;
            }
            if let Some(tri) = Triangle::new(points, [first.1, b.1, c.1], rgb) {
                emit(tri);
            }
        }
    }

    /// Draw every layer's triangles in output band `band` onto `pixels`,
    /// hidden behind and recorded in `depth`.
    fn draw_band(
        &self,
        band: usize,
        layers: &[BuildingLayer],
        pixels: &mut [PremultipliedColorU8],
        depth: &mut DepthMap,
        buffers: &mut BandBuffers,
    ) {
        let (width, sw) = (self.width, self.width * SAMPLES);
        let rows = band * BAND_ROWS..((band + 1) * BAND_ROWS).min(self.height);
        let sample_rows = rows.start * SAMPLES..rows.end * SAMPLES;
        for (sy, line) in sample_rows.clone().zip(buffers.near.chunks_exact_mut(sw)) {
            let row = &depth.inverse[(sy / SAMPLES) * width..][..width];
            for (sample, &d) in line.iter_mut().zip(row.iter().flat_map(|d| [d; SAMPLES])) {
                *sample = d;
            }
        }
        for layer in layers {
            buffers.color.fill(0);
            for &i in &layer.binned[band] {
                fill(
                    &layer.tris[i as usize],
                    &mut buffers.near,
                    &mut buffers.color,
                    sw,
                    sample_rows.clone(),
                );
            }
            resolve(
                pixels,
                &mut depth.inverse,
                &buffers.near,
                &buffers.color,
                width,
                rows.clone(),
                layer.opacity,
            );
        }
    }
}

/// Fill `tri`'s samples in `rows` (sample rows; the buffers hold just
/// those, `stride` samples each) that lie nearer than what `near` holds.
fn fill(
    tri: &Triangle,
    near: &mut [f32],
    color: &mut [u32],
    stride: usize,
    rows: std::ops::Range<usize>,
) {
    let (a, b, c) = tri.plane;
    for y in tri.rows.0.max(rows.start)..tri.rows.1.min(rows.end) {
        let cy = y as f64 + 0.5;
        // Sample centers `x + 0.5` inside every edge: an interval of x.
        let (mut lo, mut hi) = (0.0f64, stride as f64 - 1.0);
        for &(ex, ey, e0) in &tri.edges {
            let rest = ey * cy + e0;
            if ex == 0.0 {
                if rest < 0.0 {
                    hi = -1.0;
                }
                continue;
            }
            let bound = -rest / ex - 0.5;
            if ex > 0.0 {
                lo = lo.max(bound.ceil());
            } else {
                hi = hi.min(bound.floor());
            }
        }
        if lo > hi {
            continue;
        }
        let (x0, x1) = (lo as usize, hi as usize);
        let offset = (y - rows.start) * stride;
        let mut inv = a * (x0 as f64 + 0.5) + b * cy + c;
        for i in offset + x0..=offset + x1 {
            let v = inv as f32;
            if v > near[i] {
                near[i] = v;
                color[i] = tri.packed;
            }
            inv += a;
        }
    }
}

/// Blend one layer's samples (`color`, `0` where it drew nothing) over the
/// output `rows` at `opacity`, and keep the nearest depth of each covered
/// pixel.
fn resolve(
    pixels: &mut [PremultipliedColorU8],
    depth: &mut [f32],
    near: &[f32],
    color: &[u32],
    width: usize,
    rows: std::ops::Range<usize>,
    opacity: f32,
) {
    let stride = width * SAMPLES;
    let full = (SAMPLES * SAMPLES) as f32;
    for (band_y, y) in rows.enumerate() {
        for x in 0..width {
            let (mut sum, mut count, mut nearest) = ([0u32; 3], 0u32, 0f32);
            for sy in 0..SAMPLES {
                let row = (band_y * SAMPLES + sy) * stride + x * SAMPLES;
                for i in row..row + SAMPLES {
                    let c = color[i];
                    if c == 0 {
                        continue;
                    }
                    count += 1;
                    sum[0] += (c >> 16) & 0xff;
                    sum[1] += (c >> 8) & 0xff;
                    sum[2] += c & 0xff;
                    nearest = nearest.max(near[i]);
                }
            }
            if count == 0 {
                continue;
            }
            let i = y * width + x;
            depth[i] = depth[i].max(nearest);
            let alpha = count as f32 / full * opacity;
            let keep = 1.0 - alpha;
            let dst = pixels[i];
            let mix = |s: u32, d: u8| {
                (s as f32 / count as f32 * alpha + f32::from(d) * keep)
                    .round()
                    .min(255.0) as u8
            };
            let a = (alpha * 255.0 + f32::from(dst.alpha()) * keep)
                .round()
                .min(255.0) as u8;
            let (r, g, b) = (
                mix(sum[0], dst.red()),
                mix(sum[1], dst.green()),
                mix(sum[2], dst.blue()),
            );
            pixels[i] =
                PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a).unwrap_or(dst);
        }
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Color;

    use super::*;
    use crate::geo::camera::Lens;

    /// A 64 px camera looking straight down, and the world point at the
    /// middle of the screen.
    fn camera() -> (Camera, (f64, f64)) {
        let lens = Lens {
            pitch_deg: 0.0,
            ..Lens::default()
        };
        let camera = Camera::new(37.5, 127.0, 16.0, 64, 64, lens);
        let target = camera.target();
        (camera, target)
    }

    /// A red triangle 5 world px up, from 10 px left of the middle to 10 px
    /// right and 10 px down: its long edge runs diagonally.
    fn triangle((x, y): (f64, f64)) -> Facet {
        Facet {
            corners: [
                [x - 10.0, y, 5.0],
                [x + 10.0, y, 5.0],
                [x + 10.0, y + 10.0, 5.0],
            ],
            color: Color::from_rgba8(255, 0, 0, 255),
            outward: None,
            prism: 0,
        }
    }

    fn white() -> Pixmap {
        let mut pixmap = Pixmap::new(64, 64).unwrap();
        pixmap.fill(Color::WHITE);
        pixmap
    }

    #[test]
    fn facets_blend_at_their_layer_opacity_and_leave_their_depth() {
        let (camera, middle) = camera();
        let mut pixmap = white();
        let mut depth = DepthMap::empty(64, 64);
        let facets = [triangle(middle)];
        draw_buildings(&mut pixmap, &camera, &[(&facets, 0.5)], &mut depth);
        let inside = pixmap.pixel(38, 34).unwrap();
        assert_eq!((inside.red(), inside.green()), (255, 128));
        assert!(depth.inverse[34 * 64 + 38] > 0.0);
        let outside = pixmap.pixel(20, 50).unwrap();
        assert_eq!(outside.green(), 255);
        assert_eq!(depth.inverse[50 * 64 + 20], 0.0);
        // The diagonal edge is antialiased.
        let edge = (0..64 * 64)
            .filter_map(|i| pixmap.pixels().get(i))
            .any(|p| (129..255).contains(&p.green()));
        assert!(edge);
    }

    #[test]
    fn nearer_ground_hides_facets() {
        let (camera, middle) = camera();
        let mut pixmap = white();
        let mut depth = DepthMap::empty(64, 64);
        depth.inverse.fill(1.0);
        let facets = [triangle(middle)];
        draw_buildings(&mut pixmap, &camera, &[(&facets, 1.0)], &mut depth);
        assert!(pixmap.pixels().iter().all(|p| p.green() == 255));
    }

    #[test]
    fn nearer_facets_cover_farther_ones_within_a_layer() {
        let (camera, middle) = camera();
        let mut pixmap = white();
        let mut depth = DepthMap::empty(64, 64);
        let high = triangle(middle);
        let mut low = high;
        low.corners.iter_mut().for_each(|c| c[2] = 1.0);
        low.color = Color::from_rgba8(0, 0, 255, 255);
        // Drawn after the nearer one, the farther one still loses.
        let facets = [high, low];
        draw_buildings(&mut pixmap, &camera, &[(&facets, 1.0)], &mut depth);
        let inside = pixmap.pixel(38, 34).unwrap();
        assert_eq!((inside.red(), inside.blue()), (255, 0));
    }

    #[test]
    fn triangles_fill_across_bands_without_seams() {
        let lens = Lens {
            pitch_deg: 0.0,
            ..Lens::default()
        };
        let camera = Camera::new(37.5, 127.0, 16.0, 64, 128, lens);
        let (x, y) = camera.target();
        let red = Color::from_rgba8(255, 0, 0, 255);
        let corner = |dx: f64, dy: f64| [x + dx, y + dy, 0.0];
        let facets = [
            Facet {
                corners: [
                    corner(-20.0, -50.0),
                    corner(20.0, -50.0),
                    corner(20.0, 50.0),
                ],
                color: red,
                outward: None,
                prism: 0,
            },
            Facet {
                corners: [
                    corner(-20.0, -50.0),
                    corner(20.0, 50.0),
                    corner(-20.0, 50.0),
                ],
                color: red,
                outward: None,
                prism: 0,
            },
        ];
        let mut pixmap = Pixmap::new(64, 128).unwrap();
        pixmap.fill(Color::WHITE);
        let mut depth = DepthMap::empty(64, 128);
        draw_buildings(&mut pixmap, &camera, &[(&facets, 1.0)], &mut depth);
        for row in 16..112 {
            for col in [14, 32, 50] {
                assert_eq!(pixmap.pixel(col, row).unwrap().green(), 0, "{col}, {row}");
            }
        }
    }

    #[test]
    fn facets_reaching_behind_the_camera_are_cut_at_the_near_plane() {
        let camera = Camera::new(37.5, 127.0, 16.0, 64, 64, Lens::default());
        let (x, y) = camera.target();
        let (_, eye_y, _) = camera.eye();
        // A roof from ahead of the target to well behind the eye.
        let behind = eye_y + 2.0 * (eye_y - y);
        let facet = Facet {
            corners: [
                [x - 30.0, y - 30.0, 5.0],
                [x + 30.0, y - 30.0, 5.0],
                [x, behind, 5.0],
            ],
            color: Color::from_rgba8(255, 0, 0, 255),
            outward: None,
            prism: 0,
        };
        let mut pixmap = white();
        let mut depth = DepthMap::empty(64, 64);
        draw_buildings(&mut pixmap, &camera, &[(&[facet], 1.0)], &mut depth);
        assert_eq!(
            pixmap.pixel(32, 63).unwrap().green(),
            0,
            "reaches the bottom"
        );
        assert_eq!(pixmap.pixel(32, 0).unwrap().green(), 255);
    }
}
