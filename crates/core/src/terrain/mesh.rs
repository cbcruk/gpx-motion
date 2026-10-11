use std::f64::consts::PI;

use tiny_skia::{Pixmap, PremultipliedColorU8};

use super::dem::Dem;
use super::grid::{lines, locate};
use crate::geo::camera::{Camera, NEAR};
use crate::geo::mercator::{lat_to_pixel, lon_to_pixel, TILE_SIZE};
use crate::render::sample::{lerp, mip_chain, Image};
use crate::render::Relief;

/// Fraction of the area's shorter side over which heights ease to zero at
/// the border, so the mesh meets the flat map around it without a step.
const TAPER: f64 = 0.15;

/// Camera altitude kept above the highest terrain, as a multiple of it.
pub const TERRAIN_CLEARANCE: f64 = 1.2;

/// Ground relief around a route: a grid of heights from a [`Dem`] with the
/// map texture draped over it, ready to draw under a perspective [`Camera`].
pub struct Terrain {
    zoom: f64,
    origin: (f64, f64),
    /// Grid line offsets from the origin, across and down.
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// Shortest grid cell side, for stepping along lines of sight.
    finest: f64,
    heights: Vec<f64>,
    /// Hill-shading factor of each grid point (`1` is unshaded).
    light: Vec<f64>,
    /// Map textures, the main area's first; each triangle takes the first
    /// that holds it.
    drapes: Vec<Drape>,
    /// Highest point of the main area (without the surroundings).
    core_peak: f64,
}

/// A map texture over part of a [`Terrain`].
struct Drape {
    /// Corners in world pixels from the terrain's origin.
    start: (f64, f64),
    end: (f64, f64),
    /// Texels per world pixel.
    scale: f64,
    texture: Pixmap,
    /// The texture's smaller mip levels, for drawing it from afar.
    mips: Vec<Pixmap>,
}

impl Drape {
    fn holds(&self, (x, y): (f64, f64)) -> bool {
        const SLACK: f64 = 1e-6;
        x >= self.start.0 - SLACK
            && y >= self.start.1 - SLACK
            && x <= self.end.0 + SLACK
            && y <= self.end.1 + SLACK
    }
}

/// The ground around a terrain's main area, meshed and mapped more coarsely
/// (see [`Terrain::with_surroundings`]).
pub struct Surroundings<'a> {
    /// Elevation over the whole area.
    pub dem: &'a Dem,
    /// Top-left corner in world pixels at the terrain's zoom.
    pub origin: (f64, f64),
    /// Width and height in world pixels at the terrain's zoom.
    pub size: (f64, f64),
    /// The map over the whole area, stretched across `size`.
    pub map: Pixmap,
}

/// Elevation in meters at a world pixel, `None` where no DEM covers it.
type Elevation<'a> = &'a dyn Fn(f64, f64) -> Option<f64>;

impl Terrain {
    /// Build terrain for the world-pixel rectangle at `origin` covered by
    /// `map` (one texel per world pixel at `zoom`, the camera's zoom).
    ///
    /// Heights follow `relief` (same base and exaggeration as the track), are
    /// clamped at the base, and ease to zero toward the border. The map is
    /// hill-shaded from the heights. Grid points the DEM doesn't cover sit at
    /// the base.
    pub fn build(dem: &Dem, relief: &Relief, zoom: f64, origin: (f64, f64), map: Pixmap) -> Self {
        let size = (map.width() as f64, map.height() as f64);
        let elevation = |x: f64, y: f64| elevation_at(dem, zoom, x, y);
        let mut terrain = Self::mesh(relief, zoom, origin, size, None, &elevation);
        terrain.drape(origin, size, map);
        terrain
    }

    /// [`Terrain::build`] over a wider area: the main area at `origin`
    /// (mapped by `map`) as usual, and `around` it a sparser mesh draped
    /// with a coarser map, for views that see far past the main area (e.g.
    /// zooming out). Heights ease to zero only at the outer border.
    pub fn with_surroundings(
        dem: &Dem,
        relief: &Relief,
        zoom: f64,
        origin: (f64, f64),
        map: Pixmap,
        around: Surroundings,
    ) -> Self {
        let size = (map.width() as f64, map.height() as f64);
        let start = (origin.0.min(around.origin.0), origin.1.min(around.origin.1));
        let end = (
            (origin.0 + size.0).max(around.origin.0 + around.size.0),
            (origin.1 + size.1).max(around.origin.1 + around.size.1),
        );
        let inner = (
            origin.0 - start.0,
            origin.1 - start.1,
            origin.0 + size.0 - start.0,
            origin.1 + size.1 - start.1,
        );
        let elevation = |x: f64, y: f64| {
            let (lx, ly) = (x - start.0, y - start.1);
            let main = lx >= inner.0 && ly >= inner.1 && lx <= inner.2 && ly <= inner.3;
            let fine = || elevation_at(dem, zoom, x, y);
            let coarse = || elevation_at(around.dem, zoom, x, y);
            if main {
                fine().or_else(coarse)
            } else {
                coarse().or_else(fine)
            }
        };
        let outer = (end.0 - start.0, end.1 - start.1);
        let mut terrain = Self::mesh(relief, zoom, start, outer, Some(inner), &elevation);
        terrain.drape(origin, size, map);
        terrain.drape(around.origin, around.size, around.map);
        terrain
    }

    /// Terrain for the `size` world-pixel rectangle at `origin` without a
    /// map, for renderers that drape their own (e.g. one texture per map
    /// tile on a GPU) and shade it with [`Terrain::light_at_world`].
    /// [`draw_terrain`] draws it in one flat color.
    pub fn bare(
        dem: &Dem,
        relief: &Relief,
        zoom: f64,
        origin: (f64, f64),
        size: (u32, u32),
    ) -> Self {
        let size = (size.0 as f64, size.1 as f64);
        let elevation = |x: f64, y: f64| elevation_at(dem, zoom, x, y);
        let mut terrain = Self::mesh(relief, zoom, origin, size, None, &elevation);
        terrain.drapes.push(Drape {
            start: (0.0, 0.0),
            end: size,
            scale: 1.0,
            texture: Pixmap::new(1, 1).expect("a 1×1 pixmap is valid"),
            mips: Vec::new(),
        });
        terrain
    }

    /// The heights and shading of a `size` rectangle at `origin`, with
    /// denser grid lines over the `inner` rectangle (`x0, y0, x1, y1` from
    /// the origin) when given.
    fn mesh(
        relief: &Relief,
        zoom: f64,
        origin: (f64, f64),
        size: (f64, f64),
        inner: Option<(f64, f64, f64, f64)>,
        elevation: Elevation,
    ) -> Self {
        let (width, height) = size;
        let longest = width.max(height);
        let inner_longest = inner.map_or(0.0, |r| (r.2 - r.0).max(r.3 - r.1));
        let xs = lines(width, longest, inner.map(|r| (r.0, r.2, inner_longest)));
        let ys = lines(height, longest, inner.map(|r| (r.1, r.3, inner_longest)));
        let taper = TAPER * width.min(height);
        let in_core =
            |x: f64, y: f64| inner.is_none_or(|r| x >= r.0 && y >= r.1 && x <= r.2 && y <= r.3);
        let mut heights = Vec::with_capacity(xs.len() * ys.len());
        let mut core_peak: f64 = 0.0;
        for &y in &ys {
            for &x in &xs {
                let (wx, wy) = (origin.0 + x, origin.1 + y);
                let meters = elevation(wx, wy).map_or(0.0, |e| relief.height_m(Some(e)));
                let edge = x.min(y).min(width - x).min(height - y).max(0.0);
                let ease = smoothstep((edge / taper.max(1.0)).min(1.0));
                let h = meters_to_px(meters, wy, zoom) * ease;
                if in_core(x, y) {
                    core_peak = core_peak.max(h);
                }
                heights.push(h);
            }
        }
        let gaps = |lines: &[f64]| {
            lines
                .windows(2)
                .map(|w| w[1] - w[0])
                .fold(f64::INFINITY, f64::min)
        };
        let finest = gaps(&xs).min(gaps(&ys));
        let mut terrain = Self {
            zoom,
            origin,
            xs,
            ys,
            finest,
            heights,
            light: Vec::new(),
            drapes: Vec::new(),
            core_peak,
        };
        terrain.light = terrain.light_factors();
        terrain
    }

    /// Add `map` over the `size` world-pixel rectangle at world `origin`,
    /// hill-shaded from the heights.
    fn drape(&mut self, origin: (f64, f64), size: (f64, f64), mut map: Pixmap) {
        let start = (origin.0 - self.origin.0, origin.1 - self.origin.1);
        let scale = map.width() as f64 / size.0.max(1.0);
        self.shade(&mut map, start, scale);
        let mips = mip_chain(&map);
        self.drapes.push(Drape {
            start,
            end: (start.0 + size.0, start.1 + size.1),
            scale,
            texture: map,
            mips,
        });
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
        let (Some((c, fx)), Some((r, fy))) = (locate(&self.xs, x), locate(&self.ys, y)) else {
            return 0.0;
        };
        let stride = self.xs.len();
        let h = |c: usize, r: usize| self.heights[r * stride + c];
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
        let steps = (length / self.finest).ceil().clamp(1.0, MAX_SIGHT_STEPS);
        let top = self.max_height();
        for i in 1..steps as usize {
            let t = i as f64 / steps;
            let (px, py, ph) = (x + (ex - x) * t, y + (ey - y) * t, h + (eh - h) * t);
            // The line only climbs toward the eye; past the highest point
            // nothing can block it.
            if ph > top {
                return false;
            }
            if self.height_at_world(px, py) > ph + SIGHT_CLEARANCE_PX {
                return true;
            }
        }
        false
    }

    /// Top-left corner of the mesh in world pixels at [`Terrain::zoom`].
    pub fn origin(&self) -> (f64, f64) {
        self.origin
    }

    /// Offsets in world pixels from [`Terrain::origin`] of the grid lines
    /// across (`x`) and down (`y`), from `0` to the mesh's size.
    pub fn grid_lines(&self) -> (&[f64], &[f64]) {
        (&self.xs, &self.ys)
    }

    /// Height in world pixels of every grid point, row by row from the
    /// top-left. Each cell is split into two triangles along the diagonal
    /// from its top-left to its bottom-right point, as
    /// [`Terrain::height_at`] interpolates.
    pub fn heights(&self) -> &[f64] {
        &self.heights
    }

    /// Hill-shading factor at a world point at [`Terrain::zoom`], to
    /// multiply the map's color by: below `1` on slopes facing away from a
    /// northwest light, above on those facing it, interpolated between grid
    /// points as [`Terrain::build`] shades its map. Flat ground off the mesh
    /// gets the factor of level ground.
    pub fn light_at_world(&self, x: f64, y: f64) -> f64 {
        let x = locate(&self.xs, x - self.origin.0);
        let y = locate(&self.ys, y - self.origin.1);
        match (x, y) {
            (Some(x), Some(y)) => self.light_in_grid(&self.light, x, y),
            _ => Self::flat_light(),
        }
    }

    /// Hill-shading factor of level ground (as off the mesh).
    pub fn flat_light() -> f64 {
        light_factor(0.0, 0.0)
    }

    /// Bilinear `factors` (one per grid point) at a cell and the fractions
    /// across it (see [`locate`]).
    fn light_in_grid(&self, factors: &[f64], (c, fx): (usize, f64), (r, fy): (usize, f64)) -> f64 {
        let stride = self.xs.len();
        let f = |c: usize, r: usize| factors[r * stride + c];
        let top = f(c, r) + (f(c + 1, r) - f(c, r)) * fx;
        let bottom = f(c, r + 1) + (f(c + 1, r + 1) - f(c, r + 1)) * fx;
        top + (bottom - top) * fy
    }

    /// The hill-shaded map draped over the main area, one texel per world
    /// pixel (a single blank texel for [`Terrain::bare`] terrain).
    pub fn texture(&self) -> &Pixmap {
        &self.drapes[0].texture
    }

    /// Highest point of the mesh in world pixels.
    pub fn max_height(&self) -> f64 {
        self.heights.iter().copied().fold(0.0, f64::max)
    }

    /// Highest point of the main area in world pixels: the whole mesh's,
    /// except that [`Terrain::with_surroundings`] leaves the surroundings
    /// out (so cameras near the main area keep their height).
    pub fn core_max_height(&self) -> f64 {
        self.core_peak
    }

    /// Shading factor of each grid point from the slope around it.
    fn light_factors(&self) -> Vec<f64> {
        let (xs, ys) = (&self.xs, &self.ys);
        let (cols, rows) = (xs.len() - 1, ys.len() - 1);
        let stride = cols + 1;
        let h = |c: usize, r: usize| self.heights[r * stride + c];
        let mut factors = Vec::with_capacity(self.heights.len());
        for r in 0..=rows {
            for c in 0..=cols {
                let (left, right) = (c.saturating_sub(1), (c + 1).min(cols));
                let (up, down) = (r.saturating_sub(1), (r + 1).min(rows));
                let dx = (h(right, r) - h(left, r)) / (xs[right] - xs[left]);
                let dy = (h(c, down) - h(c, up)) / (ys[down] - ys[up]);
                factors.push(light_factor(dx, dy));
            }
        }
        factors
    }

    /// Darken the slopes of `map` (covering the mesh from `start`, `scale`
    /// texels per world pixel) facing away from a northwest light and
    /// brighten those facing it, interpolating shade between grid points.
    fn shade(&self, map: &mut Pixmap, start: (f64, f64), scale: f64) {
        let width = map.width() as usize;
        let at =
            |lines: &[f64], offset: f64, i: usize| locate(lines, offset + (i as f64 + 0.5) / scale);
        let columns: Vec<_> = (0..width).map(|i| at(&self.xs, start.0, i)).collect();
        for (r, row) in map.pixels_mut().chunks_mut(width).enumerate() {
            let y = at(&self.ys, start.1, r);
            for (px, x) in row.iter_mut().zip(&columns) {
                let light = match (*x, y) {
                    (Some(x), Some(y)) => self.light_in_grid(&self.light, x, y),
                    _ => Self::flat_light(),
                };
                *px = scale_px(*px, light as f32);
            }
        }
    }
}

/// Elevation in meters from `dem` at a world pixel at `zoom`.
fn elevation_at(dem: &Dem, zoom: f64, x: f64, y: f64) -> Option<f64> {
    let to_dem = 2f64.powf(dem.zoom() as f64 - zoom);
    dem.elevation_at_pixel(x * to_dem, y * to_dem)
}

/// Shading factor of ground sloping by `dx`, `dy` (height per world pixel
/// east and south) under a northwest light 45° up.
fn light_factor(dx: f64, dy: f64) -> f64 {
    let (az, alt) = (315f64.to_radians(), 45f64.to_radians());
    let light = (az.sin() * alt.cos(), -az.cos() * alt.cos(), alt.sin());
    let (nx, ny, nz) = (-dx, -dy, 1.0);
    let len = (nx * nx + ny * ny + nz * nz).sqrt();
    let lit = (nx * light.0 + ny * light.1 + nz * light.2) / len;
    (0.45 + 0.75 * lit).clamp(0.35, 1.3)
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
    let (xs, ys) = (&terrain.xs, &terrain.ys);
    let stride = xs.len();
    let vertex = |c: usize, r: usize| {
        let (x, y) = (xs[c], ys[r]);
        let h = terrain.heights[r * stride + c];
        let (wx, wy) = (terrain.origin.0 + x, terrain.origin.1 + y);
        let view = camera.view(wx, wy, h);
        (view.depth >= NEAR).then(|| ((x, y), camera.to_screen(view), view.depth))
    };
    let projected: Vec<_> = (0..ys.len())
        .flat_map(|r| (0..stride).map(move |c| (c, r)))
        .map(|(c, r)| vertex(c, r))
        .collect();
    let at = |c: usize, r: usize| projected[r * stride + c];

    let (cols, rows) = (stride - 1, ys.len() - 1);
    let last = terrain.drapes.len().saturating_sub(1);
    let mut triangles = Vec::with_capacity(cols * rows * 2);
    for r in 0..rows {
        for c in 0..cols {
            let corners = [at(c, r), at(c + 1, r), at(c + 1, r + 1), at(c, r + 1)];
            let middle = ((xs[c] + xs[c + 1]) / 2.0, (ys[r] + ys[r + 1]) / 2.0);
            let drape = terrain.drapes.iter().position(|d| d.holds(middle));
            let drape = drape.unwrap_or(last);
            for tri in [[0, 1, 2], [0, 2, 3]] {
                let pts: Option<Vec<_>> = tri.iter().map(|&i| corners[i]).collect();
                if let Some(pts) = pts {
                    let depth = pts.iter().map(|p| p.2).sum::<f64>() / 3.0;
                    triangles.push((depth, drape, [pts[0], pts[1], pts[2]]));
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

    let textures: Vec<Vec<Image>> = terrain
        .drapes
        .iter()
        .map(|d| {
            std::iter::once(&d.texture)
                .chain(&d.mips)
                .map(Image::new)
                .collect()
        })
        .collect();
    let size = (pixmap.width() as usize, pixmap.height() as usize);
    let mut depth = DepthMap {
        width: size.0,
        height: size.1,
        inverse: vec![0.0; size.0 * size.1],
    };
    let pixels = pixmap.pixels_mut();
    for &(_, i) in &order {
        let (_, d, tri) = &triangles[i as usize];
        let Some(drape) = terrain.drapes.get(*d) else {
            continue;
        };
        // Texture coordinates in the drape's texels.
        let tri = tri.map(|((x, y), screen, depth)| {
            let texel = (
                (x - drape.start.0) * drape.scale,
                (y - drape.start.1) * drape.scale,
            );
            (texel, screen, depth)
        });
        fill_triangle(pixels, &mut depth.inverse, size, &tri, &textures[*d]);
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
/// over what is there. Where the triangle shrinks the texture, the two mip
/// levels (`textures`, finest first) nearest its scale are blended. Shared edges follow a top-left rule and are evaluated
/// identically by both triangles, so neighbours neither overlap nor leave
/// gaps along them.
fn fill_triangle(
    pixels: &mut [PremultipliedColorU8],
    depth: &mut [f32],
    (width, height): (usize, usize),
    tri: &[Vertex; 3],
    textures: &[Image],
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
            w[0] * t0.0 + w[1] * t1.0 + w[2] * t2.0,
            w[0] * t0.1 + w[1] * t1.1 + w[2] * t2.1,
        )
    };
    let base = at(weights(0.5, 0.5));
    let (across, down) = (at(weights(1.5, 0.5)), at(weights(0.5, 1.5)));
    let step = (across.0 - base.0, across.1 - base.1);
    let texels = |p: (f64, f64)| (p.0 - base.0).powi(2) + (p.1 - base.1).powi(2);
    let lod = 0.5 * texels(across).max(texels(down)).log2();
    let last = textures.len().saturating_sub(1);
    let (fine, coarse) = match lod {
        lod if lod > 0.0 && (lod as usize) < last => {
            let level = lod as usize;
            let weight = ((lod - level as f64) * 256.0) as u32;
            (level, Some((level + 1, weight)))
        }
        lod if lod > 0.0 => (last, None),
        _ => (0, None),
    };
    let Some(texture) = textures.get(fine) else {
        return;
    };
    let mut fine = Walk::new(texture, fine, step);
    let mut coarse =
        coarse.map(|(level, weight)| (Walk::new(&textures[level], level, step), weight));
    let near = |w: [f64; 3]| w[0] * inverse[0] + w[1] * inverse[1] + w[2] * inverse[2];
    let step_near = near(weights(1.5, 0.5)) - near(weights(0.5, 0.5));
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
        fine.start(at(first));
        if let Some((walk, _)) = &mut coarse {
            walk.start(at(first));
        }
        let mut inv = near(first);
        let row = y * width + x0..=y * width + x1;
        for d in &mut depth[row.clone()] {
            *d = inv as f32;
            inv += step_near;
        }
        for px in &mut pixels[row] {
            let texel = match &mut coarse {
                Some((walk, weight)) => lerp(fine.next(), walk.next(), *weight),
                None => fine.next(),
            };
            *px = over(texel, *px);
        }
    }
}

/// Texture positions step in 1/65536 texels along a row.
const ONE: f64 = 65536.0;

/// One mip level of a texture, sampled pixel by pixel along a row.
struct Walk<'a> {
    image: &'a Image<'a>,
    /// Texels of this level per finest-level texel.
    scale: f64,
    /// Texels moved per pixel, in 1/[`ONE`] texels.
    step: (i64, i64),
    max: (i64, i64),
    at: (i64, i64),
}

impl<'a> Walk<'a> {
    /// Mip `level` of a texture (`image`), moving `step` finest-level texels
    /// per pixel.
    fn new(image: &'a Image<'a>, level: usize, step: (f64, f64)) -> Self {
        let scale = 0.5f64.powi(level as i32);
        Self {
            image,
            scale,
            step: ((step.0 * scale * ONE) as i64, (step.1 * scale * ONE) as i64),
            max: ((image.max_x * ONE) as i64, (image.max_y * ONE) as i64),
            at: (0, 0),
        }
    }

    /// Start a row at finest-level texture position `p` (texel corners on
    /// whole numbers).
    fn start(&mut self, p: (f64, f64)) {
        let texel = |v: f64| ((v * self.scale - 0.5) * ONE) as i64;
        self.at = (texel(p.0), texel(p.1));
    }

    /// The texture at the current pixel, then step to the next.
    fn next(&mut self) -> PremultipliedColorU8 {
        let sx = ((self.at.0.clamp(0, self.max.0) + 128) >> 8) as usize;
        let sy = ((self.at.1.clamp(0, self.max.1) + 128) >> 8) as usize;
        self.at = (
            self.at.0.wrapping_add(self.step.0),
            self.at.1.wrapping_add(self.step.1),
        );
        self.image.sample_fixed(sx, sy)
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

fn scale_px(px: PremultipliedColorU8, factor: f32) -> PremultipliedColorU8 {
    let a = px.alpha();
    let c = |v: u8| ((v as f32 * factor).round() as u32).min(a as u32) as u8;
    PremultipliedColorU8::from_rgba(c(px.red()), c(px.green()), c(px.blue()), a).unwrap_or(px)
}

#[cfg(test)]
pub(crate) mod tests {
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
                    std::slice::from_ref(&image),
                );
                fill_triangle(
                    target.pixels_mut(),
                    &mut vec![0.0; 200 * 200],
                    size,
                    &[q[0], q[2], q[3]],
                    std::slice::from_ref(&image),
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
    fn shrunk_textures_average_instead_of_aliasing() {
        // A one-texel checkerboard squeezed 8× must come out gray, not in
        // whichever black or white texels the pixel centers hit.
        let mut texture = Pixmap::new(256, 256).unwrap();
        for (i, px) in texture.pixels_mut().iter_mut().enumerate() {
            let v = if (i % 256 + i / 256) % 2 == 0 { 255 } else { 0 };
            *px = PremultipliedColorU8::from_rgba(v, v, v, 255).unwrap();
        }
        let mips = mip_chain(&texture);
        let textures: Vec<Image> = std::iter::once(&texture)
            .chain(&mips)
            .map(Image::new)
            .collect();
        let mut target = Pixmap::new(32, 32).unwrap();
        let corner = |t: f64, s: f64| ((t + 0.5, s + 0.5), (t / 8.0, s / 8.0), 1.0);
        for tri in [
            [corner(0.0, 0.0), corner(256.0, 0.0), corner(256.0, 256.0)],
            [corner(0.0, 0.0), corner(256.0, 256.0), corner(0.0, 256.0)],
        ] {
            fill_triangle(
                target.pixels_mut(),
                &mut [0.0; 1024],
                (32, 32),
                &tri,
                &textures,
            );
        }
        let reds: Vec<u8> = target.pixels().iter().map(|p| p.red()).collect();
        assert!(reds.iter().all(|r| r.abs_diff(128) <= 8), "{reds:?}");
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
        fill_triangle(
            target.pixels_mut(),
            &mut [0.0; 64],
            (8, 8),
            &sliver,
            &[image],
        );
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

    /// A 512 px square of terrain at zoom 14 with a 400 m hill in the
    /// middle, and its world origin.
    pub(crate) fn terrain() -> Terrain {
        let (dem, origin) = hill_dem();
        let relief = Relief {
            base_m: 0.0,
            exaggeration: 1.0,
        };
        let mut map = Pixmap::new(512, 512).unwrap();
        map.fill(Color::from_rgba8(0x40, 0x80, 0x40, 0xff));
        Terrain::build(&dem, &relief, ZOOM, origin, map)
    }

    /// A camera facing north at the hill's summit from `pitch` degrees.
    pub(crate) fn hill_camera(pitch_deg: f64) -> Camera {
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
                pitch_deg,
                ..Lens::default()
            },
        );
        cam.distance *= 1.5;
        cam
    }

    /// World origin of [`terrain`].
    pub(crate) fn hill_origin() -> (f64, f64) {
        hill_dem().1
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

    /// [`terrain`]'s hill with a red `size` px main area `at` world pixels
    /// from its origin, surrounded by the whole 512 px square mapped in
    /// blue at half resolution.
    fn surrounded(at: f64, size: u32) -> Terrain {
        let (dem, origin) = hill_dem();
        let relief = Relief {
            base_m: 0.0,
            exaggeration: 1.0,
        };
        let mut map = Pixmap::new(size, size).unwrap();
        map.fill(Color::from_rgba8(0xff, 0, 0, 0xff));
        let mut wide = Pixmap::new(256, 256).unwrap();
        wide.fill(Color::from_rgba8(0, 0, 0xff, 0xff));
        let around = Surroundings {
            dem: &dem,
            origin,
            size: (512.0, 512.0),
            map: wide,
        };
        let main = (origin.0 + at, origin.1 + at);
        Terrain::with_surroundings(&dem, &relief, ZOOM, main, map, around)
    }

    #[test]
    fn surroundings_carry_the_relief_past_the_main_area() {
        let t = surrounded(192.0, 128);
        let alone = terrain();
        assert_eq!(t.origin(), alone.origin());
        // The main area's border no longer eases to zero; the outer one does.
        let edge = t.height_at_local(192.0, 256.0);
        assert!(edge > 0.5 * alone.height_at_local(192.0, 256.0), "{edge}");
        assert!(t.height_at_local(0.0, 0.0).abs() < 1e-9);
        let center = t.height_at_local(256.0, 256.0);
        let summit = alone.height_at_local(256.0, 256.0);
        assert!(
            (center - summit).abs() < 0.03 * summit,
            "{center} vs {summit}"
        );
        // Dense lines over the main area, sparse around it.
        let (xs, _) = t.grid_lines();
        assert!(xs.contains(&192.0) && xs.contains(&320.0));
        assert!(xs.windows(2).all(|w| w[1] > w[0]));
        let inside = xs.iter().filter(|&&x| (192.0..=320.0).contains(&x)).count();
        assert_eq!(inside, 12);
        assert!(xs.len() < 12 + 2 * 20);
        assert_eq!(t.core_max_height(), t.max_height());
        let corner = surrounded(0.0, 128);
        assert!(corner.core_max_height() < 0.5 * corner.max_height());
    }

    #[test]
    fn each_triangle_takes_the_main_map_where_it_has_one() {
        let t = surrounded(192.0, 128);
        let mut camera = hill_camera(0.0);
        camera.distance *= 3.0;
        let mut pixmap = Pixmap::new(320, 240).unwrap();
        draw_terrain(&mut pixmap, &camera, &t);
        let px = pixmap.pixel(160, 120).unwrap();
        assert!(px.red() > 0 && px.blue() == 0, "main map in the middle");
        let px = pixmap.pixel(160, 80).unwrap();
        assert!(px.blue() > 0 && px.red() == 0, "coarse map around it");
    }

    #[test]
    fn bare_terrain_has_the_same_heights_and_shading_without_a_map() {
        let full = terrain();
        let (dem, origin) = hill_dem();
        let relief = Relief {
            base_m: 0.0,
            exaggeration: 1.0,
        };
        let bare = Terrain::bare(&dem, &relief, ZOOM, origin, (512, 512));
        assert_eq!(bare.heights, full.heights);
        assert_eq!(bare.texture().width(), 1);
        let (ox, oy) = origin;
        let (lit, shaded) = (
            bare.light_at_world(ox + 200.0, oy + 200.0),
            bare.light_at_world(ox + 312.0, oy + 312.0),
        );
        assert!(lit > Terrain::flat_light() && shaded < Terrain::flat_light());
        // The light matches the shading baked into the full terrain's map.
        let green = |t: &Terrain| t.texture().pixel(200, 200).unwrap().green() as f64;
        assert!((green(&full) - (0x80 as f64 * lit).min(255.0)).abs() <= 2.0);
        assert_eq!(bare.light_at_world(ox - 10.0, oy), Terrain::flat_light());
    }

    #[test]
    fn hillshade_lights_northwest_slopes() {
        let t = terrain();
        let lit = t.texture().pixel(200, 200).unwrap().green();
        let shaded = t.texture().pixel(312, 312).unwrap().green();
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
