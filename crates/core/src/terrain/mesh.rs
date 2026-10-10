use std::f64::consts::PI;

use tiny_skia::{
    FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, PremultipliedColorU8, SpreadMode,
    Transform,
};

use super::dem::Dem;
use crate::geo::camera::{Camera, NEAR};
use crate::geo::mercator::{lat_to_pixel, lon_to_pixel, TILE_SIZE};
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
    zoom: u32,
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
    pub fn build(dem: &Dem, relief: &Relief, zoom: u32, origin: (f64, f64), map: Pixmap) -> Self {
        let (width, height) = (map.width() as f64, map.height() as f64);
        let target = (width.max(height) / MAX_CELLS).max(MIN_CELL_PX);
        let cols = (width / target).round().max(1.0) as usize;
        let rows = (height / target).round().max(1.0) as usize;
        let cell = (width / cols as f64, height / rows as f64);
        let to_dem = 2f64.powi(dem.zoom() as i32 - zoom as i32);
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
    pub fn zoom(&self) -> u32 {
        self.zoom
    }

    /// Terrain height in world pixels under a coordinate (`0` off the mesh),
    /// interpolated within the same triangles [`draw_terrain`] draws.
    pub fn height_at(&self, lat: f64, lon: f64) -> f64 {
        let x = lon_to_pixel(lon, self.zoom) - self.origin.0;
        let y = lat_to_pixel(lat, self.zoom) - self.origin.1;
        self.height_at_local(x, y)
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

/// Draw the terrain mesh as seen by `camera` (which must use the terrain's
/// zoom), far triangles first so nearer slopes cover them.
pub fn draw_terrain(pixmap: &mut Pixmap, camera: &Camera, terrain: &Terrain) {
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
    triangles.sort_by(|a, b| b.0.total_cmp(&a.0));

    for (_, tri) in &triangles {
        let Some(transform) = affine(tri) else {
            continue;
        };
        let mut builder = PathBuilder::new();
        builder.move_to(tri[0].1 .0 as f32, tri[0].1 .1 as f32);
        builder.line_to(tri[1].1 .0 as f32, tri[1].1 .1 as f32);
        builder.line_to(tri[2].1 .0 as f32, tri[2].1 .1 as f32);
        builder.close();
        let Some(path) = builder.finish() else {
            continue;
        };
        let paint = Paint {
            shader: Pattern::new(
                terrain.texture.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bilinear,
                1.0,
                transform,
            ),
            anti_alias: false,
            ..Default::default()
        };
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

type Vertex = ((f64, f64), (f64, f64), f64);

/// The affine map taking a triangle's texture points to its screen points.
fn affine(tri: &[Vertex; 3]) -> Option<Transform> {
    let [(t0, s0, _), (t1, s1, _), (t2, s2, _)] = *tri;
    let (a, b) = ((t1.0 - t0.0, t1.1 - t0.1), (t2.0 - t0.0, t2.1 - t0.1));
    let (p, q) = ((s1.0 - s0.0, s1.1 - s0.1), (s2.0 - s0.0, s2.1 - s0.1));
    let det = a.0 * b.1 - a.1 * b.0;
    if det.abs() < f64::EPSILON {
        return None;
    }
    let m00 = (p.0 * b.1 - q.0 * a.1) / det;
    let m01 = (q.0 * a.0 - p.0 * b.0) / det;
    let m10 = (p.1 * b.1 - q.1 * a.1) / det;
    let m11 = (q.1 * a.0 - p.1 * b.0) / det;
    let tx = s0.0 - m00 * t0.0 - m01 * t0.1;
    let ty = s0.1 - m10 * t0.0 - m11 * t0.1;
    Some(Transform::from_row(
        m00 as f32, m10 as f32, m01 as f32, m11 as f32, tx as f32, ty as f32,
    ))
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// World pixels at `zoom` spanning `meters` at world y `wy`.
fn meters_to_px(meters: f64, wy: f64, zoom: u32) -> f64 {
    const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
    let world = TILE_SIZE * 2f64.powi(zoom as i32);
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
    use crate::geo::camera::Lens;
    use crate::tiles::TileId;
    use tiny_skia::Color;

    const ZOOM: u32 = 14;

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
