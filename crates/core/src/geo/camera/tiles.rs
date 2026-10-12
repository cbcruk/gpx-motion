//! Which map tiles cover the ground a [`Camera`] shows, at one tile zoom
//! within a tile budget.

use super::{Camera, FAR_FACTOR};
use crate::geo::mercator::TILE_SIZE;
use crate::tiles::{TileId, TilePlacement};

/// Upper bound on tiles per 3D view; farther views drop to coarser zooms.
const MAX_GROUND_TILES: usize = 64;
/// How many times [`MAX_GROUND_TILES`] a fading pair of tile zooms may use
/// (see [`Camera::ground_tiles_of_zoom`]).
const FADE_BUDGET: usize = 4;

impl Camera {
    /// The whole tile zoom this camera draws with; see
    /// [`crate::geo::mercator::tile_zoom_for`].
    pub fn tile_zoom(&self) -> u32 {
        crate::geo::mercator::tile_zoom_for(self.detail_zoom)
    }

    /// Tiles for the ground this camera shows, with the far cutoff and tile
    /// budget the 3D renderers share; see [`Camera::ground_tiles_with`].
    ///
    /// All tiles share one zoom; for a quadtree mixing zooms by distance from
    /// the eye, see [`ground_tile_tree`](crate::geo::ground_tile_tree). The
    /// flat 2D counterpart is
    /// [`Projection::visible_tiles`](crate::geo::Projection::visible_tiles).
    pub fn ground_tiles(&self, max_zoom: u32) -> Vec<TilePlacement> {
        self.ground_tiles_with(max_zoom, FAR_FACTOR, MAX_GROUND_TILES)
    }

    /// Tiles covering the visible ground, as world-pixel placements (`dx`/`dy`
    /// are the tile's world origin at the camera zoom, not screen offsets).
    ///
    /// Ground beyond `far_factor × distance` is ignored. The tile zoom starts
    /// at `max_zoom` (capped to [`Camera::tile_zoom`]) and drops until at most
    /// `max_tiles` cover the footprint. [`Camera::ground_tiles`] is this with
    /// the renderers' shared cutoff and budget.
    pub fn ground_tiles_with(
        &self,
        max_zoom: u32,
        far_factor: f64,
        max_tiles: usize,
    ) -> Vec<TilePlacement> {
        self.tiles_from(max_zoom.min(self.tile_zoom()), far_factor, max_tiles)
    }

    /// Tiles from `tile_zoom` covering the visible ground, finer than
    /// [`Camera::tile_zoom`] too (e.g. one fading in; see
    /// [`crate::geo::mercator::tile_levels`]). Meant for a zoom at most
    /// [`Camera::ground_budget_zoom`] plus a level, so the tile budget is
    /// `FADE_BUDGET` (4) times the usual one; past it, levels still drop.
    pub fn ground_tiles_of_zoom(&self, tile_zoom: u32) -> Vec<TilePlacement> {
        self.tiles_from(tile_zoom, FAR_FACTOR, MAX_GROUND_TILES * FADE_BUDGET)
    }

    /// The fractional tile zoom at which tiles over the visible ground
    /// would just fill the tile budget of [`Camera::ground_tiles`], judged
    /// by the footprint's area. It changes smoothly with the view, unlike
    /// the whole zoom the budget picks, so a fade can follow it.
    pub fn ground_budget_zoom(&self) -> f64 {
        let (min_x, min_y, max_x, max_y) = self.footprint(FAR_FACTOR);
        let area = ((max_x - min_x) * (max_y - min_y)).max(1.0);
        let side = (area / MAX_GROUND_TILES as f64).sqrt();
        self.zoom - (side / TILE_SIZE).log2()
    }

    /// Tiles covering the footprint from `tile_zoom`, dropping a level at a
    /// time until at most `max_tiles` cover it.
    fn tiles_from(&self, tile_zoom: u32, far_factor: f64, max_tiles: usize) -> Vec<TilePlacement> {
        let (min_x, min_y, max_x, max_y) = self.footprint(far_factor);
        let mut tile_zoom = tile_zoom;
        loop {
            let size = TILE_SIZE * 2f64.powf(self.zoom - tile_zoom as f64);
            let (x0, x1) = ((min_x / size).floor() as i64, (max_x / size).floor() as i64);
            let (y0, y1) = ((min_y / size).floor() as i64, (max_y / size).floor() as i64);
            let count = ((x1 - x0 + 1) * (y1 - y0 + 1)).max(0) as usize;
            if count <= max_tiles || tile_zoom == 0 {
                let n = 2i64.pow(tile_zoom);
                let mut tiles = Vec::with_capacity(count);
                for ty in y0.max(0)..=y1.min(n - 1) {
                    for tx in x0.max(0)..=x1.min(n - 1) {
                        tiles.push(TilePlacement {
                            id: TileId {
                                zoom: tile_zoom,
                                x: tx as u32,
                                y: ty as u32,
                            },
                            dx: tx as f64 * size,
                            dy: ty as f64 * size,
                            size,
                        });
                    }
                }
                return tiles;
            }
            tile_zoom -= 1;
        }
    }

    /// World-pixel bounding box of the ground seen through the viewport edges.
    fn footprint(&self, far_factor: f64) -> (f64, f64, f64, f64) {
        let far = self.distance * far_factor;
        let (sin_p, cos_p) = self.pitch.sin_cos();
        let (sin_b, cos_b) = self.bearing.sin_cos();
        let mut bounds = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );

        const STEPS: usize = 8;
        for k in 0..=STEPS {
            let t = k as f64 / STEPS as f64;
            for (sx, sy) in [
                (t * self.width, 0.0),
                (t * self.width, self.height),
                (0.0, t * self.height),
                (self.width, t * self.height),
            ] {
                let vx = (sx - self.width / 2.0) / self.focal;
                let vy = (sy - self.height / 2.0) / self.focal;
                // Ray in the bearing-rotated frame: right, screen-down, forward.
                let dir_x = vx;
                let dir_y = vy * cos_p - sin_p;
                let dir_h = -vy * sin_p - cos_p;
                let (cam_y, cam_h) = (self.distance * sin_p, self.distance * cos_p);
                let t_hit = if dir_h < -1e-9 {
                    cam_h / -dir_h
                } else {
                    f64::INFINITY
                };
                let (mut gx, mut gy) = (dir_x * t_hit, cam_y + dir_y * t_hit);
                let reach = gx.hypot(gy);
                if !reach.is_finite() || reach > far {
                    let len = dir_x.hypot(dir_y).max(1e-9);
                    gx = dir_x / len * far;
                    gy = dir_y / len * far;
                }
                let wx = self.target_x + cos_b * gx - sin_b * gy;
                let wy = self.target_y + sin_b * gx + cos_b * gy;
                bounds.0 = bounds.0.min(wx);
                bounds.1 = bounds.1.min(wy);
                bounds.2 = bounds.2.max(wx);
                bounds.3 = bounds.3.max(wy);
            }
        }
        bounds
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::lens;
    use super::*;
    use crate::geo::mercator::{lat_to_pixel, lon_to_pixel};

    #[test]
    fn the_budget_zoom_follows_the_distance_and_fits_the_budget() {
        let cam = Camera::new(37.5, 127.0, 16.0, 1920, 1080, lens(55.0, 0.0));
        let zoom = cam.ground_budget_zoom();
        let far = Camera {
            distance: cam.distance * 2.0,
            ..cam
        };
        assert!((far.ground_budget_zoom() - (zoom - 1.0)).abs() < 1e-9);
        // The zoom below it fits the usual budget; a level above it does not.
        let count = |z: f64| cam.tiles_from(z as u32, FAR_FACTOR, usize::MAX).len();
        assert!(
            count(zoom.floor()) <= MAX_GROUND_TILES,
            "{}",
            count(zoom.floor())
        );
        assert!(count(zoom.ceil() + 1.0) > MAX_GROUND_TILES);
        // A fading pair at most a level above it stays within the fade budget.
        let fading = cam.ground_tiles_of_zoom(zoom.round() as u32 + 1);
        assert_eq!(fading[0].id.zoom, zoom.round() as u32 + 1);
    }

    #[test]
    fn ground_tiles_with_respect_budget_and_cover_target() {
        let cam = Camera::new(37.5, 127.0, 16.0, 1280, 720, lens(60.0, 20.0));
        let tiles = cam.ground_tiles_with(14, 4.0, 64);
        assert!(!tiles.is_empty() && tiles.len() <= 64);
        let (tx, ty) = (lon_to_pixel(127.0, 16.0), lat_to_pixel(37.5, 16.0));
        assert!(tiles
            .iter()
            .any(|t| tx >= t.dx && tx < t.dx + t.size && ty >= t.dy && ty < t.dy + t.size));
    }
}
