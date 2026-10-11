use std::f64::consts::PI;

use tiny_skia::{Pixmap, PremultipliedColorU8};

use super::dem::Dem;
use crate::geo::camera::{Camera, NEAR};
use crate::geo::mercator::{lat_to_pixel, lon_to_pixel, TILE_SIZE};
use crate::render::sample::Image;
use crate::render::Relief;

/// Most grid cells along either side of a terrain mesh; larger areas use
/// coarser cells to bound the triangle count.
const MAX_CELLS: f64 = 64.0;
/// Smallest grid cell in world pixels.
const MIN_CELL_PX: f64 = 12.0;
/// Fraction of the area's shorter side over which heights ease to zero at
/// the border, so the mesh meets the flat map around it without a step.
const TAPER: f64 = 0.15;

/// Ground relief around a route: a grid of heights from a [`Dem`] with the
/// map texture draped over it, ready to draw under a perspective [`Camera`].
pub struct Terrain {
    zoom: f64,
    origin: (f64, f64),
    cell: (f64, f64),
    cols: usize,
    rows: usize,
    heights: Vec<f64>,
    texture: Pixmap,
}

impl Terrain {
    /// Build terrain for the world-pixel rectangle at `origin` covered by
    /// `map` (one texel per world pixel at `zoom`, the camera's zoom).
    ///
    /// Heights follow `relief` (same base and exaggeration as the track), are
    /// clamped at the base, and ease to zero toward the border. The map is
    /// hill-shaded from the heights. Grid points the DEM doesn't cover sit at
    /// the base.
    pub fn build(dem: &Dem, relief: &Relief, zoom: f64, origin: (f64, f64), map: Pixmap) -> Self {
        let (width, height) = (map.width() as f64, map.height() as f64);
        let target = (width.max(height) / MAX_CELLS).max(MIN_CELL_PX);
        let cols = (width / target).round().max(1.0) as usize;
        let rows = (height / target).round().max(1.0) as usize;
        let cell = (width / cols as f64, height / rows as f64);
        let to_dem = 2f64.powf(dem.zoom() as f64 - zoom);
        let taper = TAPER * width.min(height);
        let mut heights = Vec::with_capacity((cols + 1) * (rows + 1));
        for r in 0..=rows {
            for c in 0..=cols {
                let (x, y) = (c as f64 * cell.0, r as f64 * cell.1);
                let (wx, wy) = (origin.0 + x, origin.1 + y);
                let meters = dem
                    .elevation_at_pixel(wx * to_dem, wy * to_dem)
                    .map_or(0.0, |e| relief.height_m(Some(e)));
                let edge = x.min(y).min(width - x).min(height - y).max(0.0);
                let ease = smoothstep((edge / taper.max(1.0)).min(1.0));
                heights.push(meters_to_px(meters, wy, zoom) * ease);
            }
        }
        let mut terrain = Self {
            zoom,
            origin,
            cell,
            cols,
            rows,
            heights,
            texture: map,
        };
        terrain.shade();
        terrain
    }

    /// Zoom level of the world pixels the terrain is laid out in.
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// Terrain height in world pixels under a coordinate (`0` off the mesh),
    /// interpolated within the same triangles [`draw_terrain`] draws.
    pub fn height_at(&self, lat: f64, lon: f64) -> f64 {
        let x = lon_to_pixel(lon, self.zoom) - self.origin.0;
        let y = lat_to_pixel(lat, self.zoom) - self.origin.1;
        self.height_at_local(x, y)
    }

    /// Terrain height in world pixels at a world point at [`Terrain::zoom`]
    /// (`0` off the mesh).
    pub fn height_at_world(&self, x: f64, y: f64) -> f64 {
        self.height_at_local(x - self.origin.0, y - self.origin.1)
    }

    fn height_at_local(&self, x: f64, y: f64) -> f64 {
        let (gx, gy) = (x / self.cell.0, y / self.cell.1);
        if gx < 0.0 || gy < 0.0 || gx > self.cols as f64 || gy > self.rows as f64 {
            return 0.0;
        }
        let (c, r) = (
            (gx.floor() as usize).min(self.cols - 1),
            (gy.floor() as usize).min(self.rows - 1),
        );
        let (fx, fy) = (gx - c as f64, gy - r as f64);
        let h = |c: usize, r: usize| self.heights[r * (self.cols + 1) + c];
        let (h00, h11) = (h(c, r), h(c + 1, r + 1));
        if fx >= fy {
            h00 + fx * (h(c + 1, r) - h00) + fy * (h11 - h(c + 1, r))
        } else {
            h00 + fy * (h(c, r + 1) - h00) + fx * (h11 - h(c, r + 1))
        }
    }

    /// Whether the mesh blocks the line of sight from `camera` to the world
    /// point `(x, y)` raised `h` pixels, found by stepping along the line
    /// and comparing heights (no depth map needed).
    pub fn hides_point(&self, camera: &Camera, x: f64, y: f64, h: f64) -> bool {
        let (ex, ey, eh) = camera.eye();
        let length = (ex - x).hypot(ey - y);
        let steps = (length / self.cell.0.min(self.cell.1))
            .ceil()
            .clamp(1.0, MAX_SIGHT_STEPS);
        (1..steps as usize).any(|i| {
            let t = i as f64 / steps;
            let (px, py, ph) = (x + (ex - x) * t, y + (ey - y) * t, h + (eh - h) * t);
            self.height_at_world(px, py) > ph + SIGHT_CLEARANCE_PX
        })
    }

    /// Highest point of the mesh in world pixels.
    pub fn max_height(&self) -> f64 {
        self.heights.iter().copied().fold(0.0, f64::max)
    }

    /// Darken slopes facing away from a northwest light and brighten those
    /// facing it, interpolating shade between grid points.
    fn shade(&mut self) {
        let stride = self.cols + 1;
        let h = |c: usize, r: usize| self.heights[r * stride + c];
        let (az, alt) = (315f64.to_radians(), 45f64.to_radians());
        let light = (az.sin() * alt.cos(), -az.cos() * alt.cos(), alt.sin());
        let mut factors = Vec::with_capacity(self.heights.len());
        for r in 0..=self.rows {
            for c in 0..=self.cols {
                let dx = (h((c + 1).min(self.cols), r) - h(c.saturating_sub(1), r))
                    / (self.cell.0 * ((c + 1).min(self.cols) - c.saturating_sub(1)) as f64);
                let dy = (h(c, (r + 1).min(self.rows)) - h(c, r.saturating_sub(1)))
                    / (self.cell.1 * ((r + 1).min(self.rows) - r.saturating_sub(1)) as f64);
                let (nx, ny, nz) = (-dx, -dy, 1.0);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let lit = (nx * light.0 + ny * light.1 + nz * light.2) / len;
                factors.push((0.45 + 0.75 * lit).clamp(0.35, 1.3));
            }
        }
        let width = self.texture.width() as usize;
        let cell = self.cell;
        for (i, px) in self.texture.pixels_mut().iter_mut().enumerate() {
            let (gx, gy) = (
                ((i % width) as f64 + 0.5) / cell.0,
                ((i / width) as f64 + 0.5) / cell.1,
            );
            let (c, r) = (
                (gx.floor() as usize).min(self.cols - 1),
                (gy.floor() as usize).min(self.rows - 1),
            );
            let (fx, fy) = (gx - c as f64, gy - r as f64);
            let f = |c: usize, r: usize| factors[r * stride + c];
            let top = f(c, r) + (f(c + 1, r) - f(c, r)) * fx;
            let bottom = f(c, r + 1) + (f(c + 1, r + 1) - f(c, r + 1)) * fx;
            *px = scale(*px, (top + (bottom - top) * fy) as f32);
        }
    }
}

/// How far each pixel of a drawn terrain is from the camera, for hiding what
/// lies behind the hills.
#[derive(Debug, Clone)]
pub struct DepthMap {
    width: usize,
    height: usize,
    /// `1 / depth` per pixel (`0` where no terrain was drawn), which is
    /// affine in screen space and so interpolates exactly across triangles.
    inverse: Vec<f32>,
}

/// Most height comparisons along one line of sight in
/// [`Terrain::hides_point`].
const MAX_SIGHT_STEPS: f64 = 256.0;
/// How far above a line of sight the ground must rise to block it, in world
/// pixels; keeps points lying on the ground from blocking themselves.
const SIGHT_CLEARANCE_PX: f64 = 1.0;

/// Fraction of its depth by which terrain must be nearer than a point to
/// hide it; absorbs the mesh's linear interpolation of the heights.
const OCCLUSION_TOLERANCE: f64 = 0.01;

impl DepthMap {
    /// Whether terrain covers the screen point `(x, y)` nearer than `depth`
    /// (camera-space, as in [`crate::geo::camera::ViewPoint::depth`]).
    ///
    /// Every pixel around the point must be nearer: at grazing angles the
    /// ground's depth jumps from one pixel to the next, and a point lying on
    /// the ground would otherwise flicker behind it.
    pub fn hides(&self, x: f64, y: f64, depth: f64) -> bool {
        let inside =
            x >= 1.0 && y >= 1.0 && x < (self.width as f64 - 1.0) && y < (self.height as f64 - 1.0);
        if !inside || depth.is_nan() || depth <= 0.0 {
            return false;
        }
        let (px, py) = (x as usize, y as usize);
        (py - 1..=py + 1).all(|ny| {
            (px - 1..=px + 1).all(|nx| {
                let terrain = self.inverse[ny * self.width + nx] as f64;
                terrain * (1.0 - OCCLUSION_TOLERANCE) * depth > 1.0
            })
        })
    }
}

/// Draw the terrain mesh as seen by `camera` (which must use the terrain's
/// zoom), far triangles first so nearer slopes cover them, and return how
/// far each drawn pixel is.
pub fn draw_terrain(pixmap: &mut Pixmap, camera: &Camera, terrain: &Terrain) -> DepthMap {
    debug_assert_eq!(camera.zoom, terrain.zoom, "camera and terrain zoom differ");
    let stride = terrain.cols + 1;
    let vertex = |c: usize, r: usize| {
        let (x, y) = (c as f64 * terrain.cell.0, r as f64 * terrain.cell.1);
        let h = terrain.heights[r * stride + c];
        let (wx, wy) = (terrain.origin.0 + x, terrain.origin.1 + y);
        let view = camera.view(wx, wy, h);
        (view.depth >= NEAR).then(|| ((x, y), camera.to_screen(view), view.depth))
    };
    let projected: Vec<_> = (0..=terrain.rows)
        .flat_map(|r| (0..=terrain.cols).map(move |c| (c, r)))
        .map(|(c, r)| vertex(c, r))
        .collect();
    let at = |c: usize, r: usize| projected[r * stride + c];

    let mut triangles = Vec::with_capacity(terrain.cols * terrain.rows * 2);
    for r in 0..terrain.rows {
        for c in 0..terrain.cols {
            let corners = [at(c, r), at(c + 1, r), at(c + 1, r + 1), at(c, r + 1)];
            for tri in [[0, 1, 2], [0, 2, 3]] {
                let pts: Option<Vec<_>> = tri.iter().map(|&i| corners[i]).collect();
                if let Some(pts) = pts {
                    let depth = pts.iter().map(|p| p.2).sum::<f64>() / 3.0;
                    triangles.push((depth, [pts[0], pts[1], pts[2]]));
                }
            }
        }
    }
    let mut order: Vec<(f64, u32)> = triangles
        .iter()
        .enumerate()
        .map(|(i, t)| (t.0, i as u32))
        .collect();
    order.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    let texture = Image::new(&terrain.texture);
    let size = (pixmap.width() as usize, pixmap.height() as usize);
    let mut depth = DepthMap {
        width: size.0,
        height: size.1,
        inverse: vec![0.0; size.0 * size.1],
    };
    let pixels = pixmap.pixels_mut();
    for &(_, i) in &order {
        let tri = &triangles[i as usize].1;
        fill_triangle(pixels, &mut depth.inverse, size, tri, &texture);
    }
    depth
}

/// Subpixel steps per pixel that triangle vertices snap to.
const SUB: f64 = 256.0;
/// Largest snapped coordinate (a million pixels off screen); triangles
/// reaching farther are near-plane slivers and are skipped.
const MAX_SNAPPED: f64 = 1e6 * SUB;

type Vertex = ((f64, f64), (f64, f64), f64);

/// Fill the pixels whose centers lie in a triangle's screen points with its
/// texture, mapped affinely and sampled bilinearly (edges clamp), blended
/// over what is there. Shared edges follow a top-left rule and are evaluated
/// identically by both triangles, so neighbours neither overlap nor leave
/// gaps along them.
fn fill_triangle(
    pixels: &mut [PremultipliedColorU8],
    depth: &mut [f32],
    (width, height): (usize, usize),
    tri: &[Vertex; 3],
    texture: &Image,
) {
    let [(t0, s0, d0), (t1, s1, d1), (t2, s2, d2)] = *tri;
    let inverse = [1.0 / d0, 1.0 / d1, 1.0 / d2];
    // Coverage is decided on vertices snapped to 1/SUB pixel with exact
    // integer edge functions, so every pixel center along a shared edge or
    // at a shared vertex belongs to exactly one triangle.
    let snap = |p: (f64, f64)| -> Option<(i128, i128)> {
        let (x, y) = (p.0 * SUB, p.1 * SUB);
        (x.abs() <= MAX_SNAPPED && y.abs() <= MAX_SNAPPED)
            .then(|| (x.round() as i128, y.round() as i128))
    };
    let (Some(p0), Some(p1), Some(p2)) = (snap(s0), snap(s1), snap(s2)) else {
        return;
    };
    let area = (p1.0 - p0.0) * (p2.1 - p0.1) - (p1.1 - p0.1) * (p2.0 - p0.0);
    if area == 0 {
        return;
    }
    let orient = area.signum();
    // Edge i is opposite vertex i: e(p) = a·x + b·y + c ≥ 0 inside. Taking the
    // endpoints in a fixed order gives both triangles of an edge the same
    // numbers up to sign.
    let edge = |p: (i128, i128), q: (i128, i128)| {
        let (first, second, flip) = if p <= q {
            (p, q, orient)
        } else {
            (q, p, -orient)
        };
        let a = (first.1 - second.1) * flip;
        let b = (second.0 - first.0) * flip;
        let c = (first.0 * second.1 - first.1 * second.0) * flip;
        // Top-left rule: pixels exactly on a top or left edge are inside.
        let owns = a > 0 || (a == 0 && b > 0);
        (a, b, c, owns)
    };
    let edges = [edge(p1, p2), edge(p2, p0), edge(p0, p1)];
    let min_x = s0.0.min(s1.0).min(s2.0).floor().max(0.0);
    let max_x = s0.0.max(s1.0).max(s2.0).ceil().min(width as f64);
    let min_y = s0.1.min(s1.1).min(s2.1).floor().max(0.0);
    let max_y = s0.1.max(s1.1).max(s2.1).ceil().min(height as f64);
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    // Texture coordinates are affine in the pixel center (barycentric
    // weights of the unsnapped vertices).
    let norm = (s1.0 - s0.0) * (s2.1 - s0.1) - (s1.1 - s0.1) * (s2.0 - s0.0);
    let weights = |x: f64, y: f64| {
        let w = |p: (f64, f64), q: (f64, f64)| {
            ((q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0)) / norm
        };
        [w(s1, s2), w(s2, s0), w(s0, s1)]
    };
    let at = |w: [f64; 3]| {
        (
            w[0] * t0.0 + w[1] * t1.0 + w[2] * t2.0 - 0.5,
            w[0] * t0.1 + w[1] * t1.1 + w[2] * t2.1 - 0.5,
        )
    };
    // Texture positions step in 1/65536 pixels along a row.
    const ONE: f64 = 65536.0;
    let max_fx = (texture.max_x * ONE) as i64;
    let max_fy = (texture.max_y * ONE) as i64;
    let near = |w: [f64; 3]| w[0] * inverse[0] + w[1] * inverse[1] + w[2] * inverse[2];
    let step_near = near(weights(1.5, 0.5)) - near(weights(0.5, 0.5));
    let (base, next) = (at(weights(0.5, 0.5)), at(weights(1.5, 0.5)));
    let (step_x, step_y) = (
        ((next.0 - base.0) * ONE) as i64,
        ((next.1 - base.1) * ONE) as i64,
    );
    let half = SUB as i128 / 2;
    for y in min_y as usize..max_y as usize {
        let cy = y as i128 * SUB as i128 + half;
        // Each edge bounds the row's pixel centers from one side; keep the
        // first and last integer x whose center passes all three.
        let (mut lo, mut hi) = (min_x as i128, max_x as i128 - 1);
        for &(a, b, c, owns) in &edges {
            let rest = b * cy + c;
            if a == 0 {
                if rest < 0 || (rest == 0 && !owns) {
                    lo = i128::MAX;
                }
                continue;
            }
            // Center x is inside when a·(SUB·x + SUB/2) + rest ≥ 0 (> 0 off
            // the owned edges), i.e. x against n / d:
            let (n, d) = (-rest - a * half, a * SUB as i128);
            if a > 0 {
                lo = lo.max(if owns {
                    ceil_div(n, d)
                } else {
                    n.div_euclid(d) + 1
                });
            } else {
                // Dividing by a negative d flips the inequality.
                let (n, d) = (-n, -d);
                hi = hi.min(if owns {
                    n.div_euclid(d)
                } else {
                    ceil_div(n, d) - 1
                });
            }
        }
        if lo > hi {
            continue;
        }
        let (x0, x1) = (lo as usize, hi as usize);
        let first = weights(x0 as f64 + 0.5, y as f64 + 0.5);
        let (tx, ty) = at(first);
        let (mut fx, mut fy) = ((tx * ONE) as i64, (ty * ONE) as i64);
        let mut inv = near(first);
        let row = y * width + x0..=y * width + x1;
        for d in &mut depth[row.clone()] {
            *d = inv as f32;
            inv += step_near;
        }
        for px in &mut pixels[row] {
            let sx = ((fx.clamp(0, max_fx) + 128) >> 8) as usize;
            let sy = ((fy.clamp(0, max_fy) + 128) >> 8) as usize;
            *px = over(texture.sample_fixed(sx, sy), *px);
            fx = fx.wrapping_add(step_x);
            fy = fy.wrapping_add(step_y);
        }
    }
}

/// `n / d` rounded up, for `d > 0`.
fn ceil_div(n: i128, d: i128) -> i128 {
    -(-n).div_euclid(d)
}

/// `src` drawn over `dst`, both premultiplied.
fn over(src: PremultipliedColorU8, dst: PremultipliedColorU8) -> PremultipliedColorU8 {
    match src.alpha() {
        0xff => src,
        0 => dst,
        a => {
            let keep = 255 - a as u32;
            let mix = |s: u8, d: u8| (s as u32 + (d as u32 * keep + 127) / 255).min(255) as u8;
            PremultipliedColorU8::from_rgba(
                mix(src.red(), dst.red()),
                mix(src.green(), dst.green()),
                mix(src.blue(), dst.blue()),
                mix(src.alpha(), dst.alpha()),
            )
            .unwrap_or(src)
        }
    }
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// World pixels at `zoom` spanning `meters` at world y `wy`.
fn meters_to_px(meters: f64, wy: f64, zoom: f64) -> f64 {
    const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
    let world = TILE_SIZE * 2f64.powf(zoom);
    let lat = (PI * (1.0 - 2.0 * wy / world)).sinh().atan();
    meters * world / (EARTH_CIRCUMFERENCE_M * lat.cos())
}

fn scale(px: PremultipliedColorU8, factor: f32) -> PremultipliedColorU8 {
    let a = px.alpha();
    let c = |v: u8| ((v as f32 * factor).round() as u32).min(a as u32) as u8;
    PremultipliedColorU8::from_rgba(c(px.red()), c(px.green()), c(px.blue()), a).unwrap_or(px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fill a grid of `n`×`n` cells (two triangles each) whose corners come
    /// from `corner`, with a half-transparent texture, and count pixels
    /// drawn more than once and pixel centers inside the grid left empty.
    fn seams(n: usize, corner: impl Fn(usize, usize) -> (f64, f64)) -> (usize, usize) {
        let mut texture = Pixmap::new(4, 4).unwrap();
        texture.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 128));
        let image = Image::new(&texture);
        let mut target = Pixmap::new(200, 200).unwrap();
        let size = (200, 200);
        let v = |c: usize, r: usize| ((c as f64, r as f64), corner(c, r), 0.0);
        for r in 0..n {
            for c in 0..n {
                let q = [v(c, r), v(c + 1, r), v(c + 1, r + 1), v(c, r + 1)];
                fill_triangle(
                    target.pixels_mut(),
                    &mut vec![0.0; 200 * 200],
                    size,
                    &[q[0], q[1], q[2]],
                    &image,
                );
                fill_triangle(
                    target.pixels_mut(),
                    &mut vec![0.0; 200 * 200],
                    size,
                    &[q[0], q[2], q[3]],
                    &image,
                );
            }
        }
        let single = target.pixels().iter().map(|p| p.alpha()).max().unwrap_or(0);
        let doubled = target.pixels().iter().filter(|p| p.alpha() > 128).count();
        // Pixel centers strictly inside the grid's outline must be drawn.
        let outline: Vec<(f32, f32)> = (0..n)
            .map(|i| corner(i, 0))
            .chain((0..n).map(|i| corner(n, i)))
            .chain((0..n).map(|i| corner(n - i, n)))
            .chain((0..n).map(|i| corner(0, n - i)))
            .map(|(x, y)| (x as f32, y as f32))
            .collect();
        let mut gaps = 0;
        for y in 0..200 {
            for x in 0..200 {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                let near_edge = outline
                    .iter()
                    .zip(outline.iter().cycle().skip(1))
                    .any(|(a, b)| segment_distance(p, *a, *b) < 1e-3);
                if !near_edge && inside(&outline, p) && target.pixel(x, y).unwrap().alpha() == 0 {
                    gaps += 1;
                }
            }
        }
        assert!(single <= 128 || doubled > 0);
        (doubled, gaps)
    }

    fn inside(poly: &[(f32, f32)], (px, py): (f32, f32)) -> bool {
        let mut inside = false;
        for (&(ax, ay), &(bx, by)) in poly.iter().zip(poly.iter().cycle().skip(1)) {
            if (ay > py) != (by > py) && px < ax + (py - ay) / (by - ay) * (bx - ax) {
                inside = !inside;
            }
        }
        inside
    }

    fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx * dx + dy * dy;
        let t = if len > 0.0 {
            (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
    }

    #[test]
    fn shared_edges_are_drawn_exactly_once() {
        // Corners on half pixels put pixel centers exactly on the edges.
        let snapped = |c: usize, r: usize| {
            let jitter = ((c * 7 + r * 3) % 5) as f64 * 0.5;
            (
                10.5 + c as f64 * 14.0 + jitter,
                12.5 + r as f64 * 13.0 - jitter,
            )
        };
        assert_eq!(seams(12, snapped), (0, 0));
        // A quarter turn computed in floating point leaves edges a hair off
        // the pixel-center rows.
        let (sin, cos) = std::f64::consts::FRAC_PI_2.sin_cos();
        let turned = |c: usize, r: usize| {
            let (x, y) = (c as f64 * 12.0 - 66.0, r as f64 * 12.0 - 66.0);
            (100.5 + x * cos - y * sin, 100.5 + x * sin + y * cos)
        };
        assert_eq!(seams(11, turned), (0, 0));
    }

    #[test]
    fn slivers_are_skipped() {
        let texture = Pixmap::new(4, 4).unwrap();
        let image = Image::new(&texture);
        let mut target = Pixmap::new(8, 8).unwrap();
        let sliver = [
            ((0.0, 0.0), (1.0, 1.0), 0.0),
            ((3.0, 0.0), (f64::MAX / 4.0, 1.0 + 1e-300), 0.0),
            ((0.0, 3.0), (6.0, 1.0), 0.0),
        ];
        fill_triangle(target.pixels_mut(), &mut [0.0; 64], (8, 8), &sliver, &image);
    }
    use crate::geo::camera::Lens;
    use crate::tiles::TileId;
    use tiny_skia::Color;

    const ZOOM: f64 = 14.0;

    /// One DEM tile at zoom 13 holding a 400 m hill centered on the tile.
    fn hill_dem() -> (Dem, (f64, f64)) {
        let (tx, ty) = (6985, 3172);
        let mut rgba = Vec::with_capacity(256 * 256 * 4);
        for y in 0..256 {
            for x in 0..256 {
                let d = ((x as f64 - 128.0).powi(2) + (y as f64 - 128.0).powi(2)).sqrt();
                let v = (400.0 * (1.0 - d / 128.0)).max(0.0) + 32768.0;
                let r = (v / 256.0).floor();
                let g = (v - r * 256.0).floor();
                rgba.extend_from_slice(&[r as u8, g as u8, 0, 0xff]);
            }
        }
        let mut dem = Dem::new(13);
        dem.add_terrarium(
            TileId {
                zoom: 13,
                x: tx,
                y: ty,
            },
            &rgba,
        );
        let origin = (tx as f64 * 512.0, ty as f64 * 512.0);
        (dem, origin)
    }

    fn terrain() -> Terrain {
        let (dem, origin) = hill_dem();
        let relief = Relief {
            base_m: 0.0,
            exaggeration: 1.0,
        };
        let mut map = Pixmap::new(512, 512).unwrap();
        map.fill(Color::from_rgba8(0x40, 0x80, 0x40, 0xff));
        Terrain::build(&dem, &relief, ZOOM, origin, map)
    }

    #[test]
    fn heights_peak_in_the_middle_and_ease_to_zero_at_the_border() {
        let t = terrain();
        let center = t.height_at_local(256.0, 256.0);
        assert!(center > 10.0, "center {center}");
        assert!(t.height_at_local(0.0, 0.0).abs() < 1e-9);
        assert!(t.height_at_local(512.0, 300.0).abs() < 1e-9);
        assert!(t.max_height() >= center);
        assert_eq!(t.height_at_local(-5.0, 10.0), 0.0);
    }

    #[test]
    fn hillshade_lights_northwest_slopes() {
        let t = terrain();
        let lit = t.texture.pixel(200, 200).unwrap().green();
        let shaded = t.texture.pixel(312, 312).unwrap().green();
        assert!(lit > shaded, "lit {lit} vs shaded {shaded}");
    }

    #[test]
    fn the_hill_hides_the_ground_behind_it() {
        let t = terrain();
        let n = 2f64.powi(13);
        let lon = (6985.5 / n) * 360.0 - 180.0;
        let lat = (PI * (1.0 - 2.0 * 3172.5 / n)).sinh().atan().to_degrees();
        let mut cam = Camera::new(
            lat,
            lon,
            ZOOM,
            320,
            240,
            Lens {
                pitch_deg: 75.0,
                ..Lens::default()
            },
        );
        cam.distance *= 1.5;
        let mut pixmap = Pixmap::new(320, 240).unwrap();
        let depth = draw_terrain(&mut pixmap, &cam, &t);
        // Facing north: ground just past the summit is behind it, ground on
        // the near side is not, and neither is the summit itself.
        let (cx, cy) = (t.origin.0 + 256.0, t.origin.1 + 256.0);
        let hidden = |x: f64, y: f64, h: f64| {
            let v = cam.view(x, y, h);
            let (sx, sy) = cam.to_screen(v);
            depth.hides(sx, sy, v.depth)
        };
        assert!(
            t.hides_point(&cam, cx, cy - 60.0, 0.0),
            "ray: behind the hill"
        );
        let front = t.height_at_local(256.0, 316.0);
        assert!(!t.hides_point(&cam, cx, cy + 60.0, front), "ray: in front");
        let top = t.height_at_local(256.0, 256.0);
        assert!(!t.hides_point(&cam, cx, cy, top), "ray: summit");
        assert!(hidden(cx, cy - 60.0, 0.0), "behind the hill");
        assert!(
            !hidden(cx, cy + 60.0, t.height_at_local(256.0, 316.0)),
            "in front"
        );
        assert!(!hidden(cx, cy, t.height_at_local(256.0, 256.0)), "summit");
    }

    #[test]
    fn draws_the_mesh() {
        let t = terrain();
        let n = 2f64.powi(13);
        let lon = (6985.5 / n) * 360.0 - 180.0;
        let lat = (PI * (1.0 - 2.0 * 3172.5 / n)).sinh().atan().to_degrees();
        let mut cam = Camera::new(
            lat,
            lon,
            ZOOM,
            320,
            240,
            Lens {
                pitch_deg: 55.0,
                ..Lens::default()
            },
        );
        cam.distance *= 1.5;
        let mut pixmap = Pixmap::new(320, 240).unwrap();
        pixmap.fill(Color::BLACK);
        draw_terrain(&mut pixmap, &cam, &t);
        let drawn = pixmap.pixels().iter().filter(|p| p.green() > 0).count();
        assert!(drawn > 320 * 240 / 4, "{drawn}");
    }
}
