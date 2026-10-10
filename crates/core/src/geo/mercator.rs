use crate::gpx::model::TrackPoint;
use crate::tiles::{TileId, TilePlacement};

pub const TILE_SIZE: f64 = 256.0;
pub const MAX_ZOOM: u32 = 22;

/// Global Web Mercator x pixel for a longitude at the given zoom.
pub fn lon_to_pixel(lon: f64, zoom: u32) -> f64 {
    let n = 2f64.powi(zoom as i32);
    (lon + 180.0) / 360.0 * n * TILE_SIZE
}

/// Global Web Mercator y pixel for a latitude at the given zoom.
pub fn lat_to_pixel(lat: f64, zoom: u32) -> f64 {
    let n = 2f64.powi(zoom as i32);
    let lat_rad = lat.to_radians();
    (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n * TILE_SIZE
}

/// Maps WGS84 coordinates into viewport pixel space at a fixed zoom and origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    pub zoom: u32,
    offset_x: f64,
    offset_y: f64,
}

impl Projection {
    /// A projection at a fixed zoom centered on a coordinate — the basis for a
    /// camera that follows the moving marker (viewport smaller than the track).
    pub fn centered(lat: f64, lon: f64, zoom: u32, width: u32, height: u32) -> Self {
        Self {
            zoom,
            offset_x: lon_to_pixel(lon, zoom) - width as f64 / 2.0,
            offset_y: lat_to_pixel(lat, zoom) - height as f64 / 2.0,
        }
    }

    /// A projection at a fixed zoom whose viewport's top-left corner is the
    /// global pixel `(x, y)`.
    pub fn from_origin(zoom: u32, x: f64, y: f64) -> Self {
        Self {
            zoom,
            offset_x: x,
            offset_y: y,
        }
    }

    /// Project a coordinate to viewport pixels (top-left origin).
    pub fn project(&self, lat: f64, lon: f64) -> (f64, f64) {
        (
            lon_to_pixel(lon, self.zoom) - self.offset_x,
            lat_to_pixel(lat, self.zoom) - self.offset_y,
        )
    }

    /// Tiles overlapping the viewport, with each tile's top-left corner placed
    /// in viewport pixel space. Tiles outside the valid `0..2^zoom` grid (e.g.
    /// past the poles) are skipped.
    pub fn visible_tiles(&self, width: u32, height: u32) -> Vec<TilePlacement> {
        self.visible_tiles_at(self.zoom, width, height)
    }

    /// Tiles from `tile_zoom` overlapping the viewport, for sources whose
    /// maximum zoom is below the projection's.
    ///
    /// Each lower-zoom tile is overzoomed to cover `256 * 2^(zoom - tile_zoom)`
    /// viewport pixels (its [`TilePlacement::size`]). A `tile_zoom`
    /// above the projection zoom is clamped to it.
    pub fn visible_tiles_at(&self, tile_zoom: u32, width: u32, height: u32) -> Vec<TilePlacement> {
        let tile_zoom = tile_zoom.min(self.zoom);
        let n = 2i64.pow(tile_zoom);
        let size = TILE_SIZE * 2f64.powi((self.zoom - tile_zoom) as i32);
        let min_tx = (self.offset_x / size).floor() as i64;
        let max_tx = ((self.offset_x + width as f64 - 1.0) / size).floor() as i64;
        let min_ty = (self.offset_y / size).floor() as i64;
        let max_ty = ((self.offset_y + height as f64 - 1.0) / size).floor() as i64;

        let mut tiles = Vec::new();
        for ty in min_ty..=max_ty {
            if ty < 0 || ty >= n {
                continue;
            }
            for tx in min_tx..=max_tx {
                if tx < 0 || tx >= n {
                    continue;
                }
                tiles.push(TilePlacement {
                    id: TileId {
                        zoom: tile_zoom,
                        x: tx as u32,
                        y: ty as u32,
                    },
                    dx: tx as f64 * size - self.offset_x,
                    dy: ty as f64 * size - self.offset_y,
                    size,
                });
            }
        }
        tiles
    }

    /// Choose the highest zoom whose bounding box fits in the viewport (minus
    /// padding on each side), then center the track within it.
    ///
    /// Returns `None` when there are fewer than one point to bound.
    pub fn fit(points: &[TrackPoint], width: u32, height: u32, padding: f64) -> Option<Self> {
        let (min_lat, max_lat, min_lon, max_lon) = bounds(points)?;
        let avail_w = (width as f64 - 2.0 * padding).max(1.0);
        let avail_h = (height as f64 - 2.0 * padding).max(1.0);

        let mut zoom = 0;
        for z in 0..=MAX_ZOOM {
            let span_x = lon_to_pixel(max_lon, z) - lon_to_pixel(min_lon, z);
            // y grows downward: max_lat is the smaller pixel value.
            let span_y = lat_to_pixel(min_lat, z) - lat_to_pixel(max_lat, z);
            if span_x <= avail_w && span_y <= avail_h {
                zoom = z;
            } else {
                break;
            }
        }

        let bbox_min_x = lon_to_pixel(min_lon, zoom);
        let bbox_max_x = lon_to_pixel(max_lon, zoom);
        let bbox_min_y = lat_to_pixel(max_lat, zoom);
        let bbox_max_y = lat_to_pixel(min_lat, zoom);
        let bbox_w = bbox_max_x - bbox_min_x;
        let bbox_h = bbox_max_y - bbox_min_y;

        let offset_x = bbox_min_x - (width as f64 - bbox_w) / 2.0;
        let offset_y = bbox_min_y - (height as f64 - bbox_h) / 2.0;

        Some(Self {
            zoom,
            offset_x,
            offset_y,
        })
    }
}

/// (min_lat, max_lat, min_lon, max_lon)
fn bounds(points: &[TrackPoint]) -> Option<(f64, f64, f64, f64)> {
    let first = points.first()?;
    let mut min_lat = first.lat;
    let mut max_lat = first.lat;
    let mut min_lon = first.lon;
    let mut max_lon = first.lon;
    for p in &points[1..] {
        min_lat = min_lat.min(p.lat);
        max_lat = max_lat.max(p.lat);
        min_lon = min_lon.min(p.lon);
        max_lon = max_lon.max(p.lon);
    }
    Some((min_lat, max_lat, min_lon, max_lon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equator_prime_meridian_is_center() {
        // At zoom 0 the world is one 256px tile; (0,0) maps to its center.
        assert!((lon_to_pixel(0.0, 0) - 128.0).abs() < 1e-9);
        assert!((lat_to_pixel(0.0, 0) - 128.0).abs() < 1e-9);
    }

    #[test]
    fn higher_zoom_doubles_pixel_span() {
        let a = lon_to_pixel(10.0, 5) - lon_to_pixel(0.0, 5);
        let b = lon_to_pixel(10.0, 6) - lon_to_pixel(0.0, 6);
        assert!((b - 2.0 * a).abs() < 1e-6);
    }

    #[test]
    fn fit_keeps_all_points_inside_viewport() {
        let pts = vec![
            TrackPoint::new(37.5665, 126.9780),
            TrackPoint::new(37.5700, 126.9850),
            TrackPoint::new(37.5610, 126.9700),
        ];
        let proj = Projection::fit(&pts, 640, 640, 32.0).unwrap();
        for p in &pts {
            let (x, y) = proj.project(p.lat, p.lon);
            assert!((0.0..=640.0).contains(&x), "x out of range: {x}");
            assert!((0.0..=640.0).contains(&y), "y out of range: {y}");
        }
    }

    #[test]
    fn fit_empty_is_none() {
        assert!(Projection::fit(&[], 640, 640, 0.0).is_none());
    }

    #[test]
    fn centered_places_point_at_viewport_center() {
        let proj = Projection::centered(37.5665, 126.9780, 14, 640, 480);
        let (x, y) = proj.project(37.5665, 126.9780);
        assert!((x - 320.0).abs() < 1e-6, "x was {x}");
        assert!((y - 240.0).abs() < 1e-6, "y was {y}");
    }

    #[test]
    fn visible_tiles_cover_projected_points() {
        let pts = vec![
            TrackPoint::new(37.5665, 126.9780),
            TrackPoint::new(37.5700, 126.9850),
            TrackPoint::new(37.5610, 126.9700),
        ];
        let proj = Projection::fit(&pts, 640, 640, 32.0).unwrap();
        let tiles = proj.visible_tiles(640, 640);
        assert!(!tiles.is_empty());

        for p in &pts {
            let (px, py) = proj.project(p.lat, p.lon);
            let covered = tiles.iter().any(|t| {
                px >= t.dx && px < t.dx + TILE_SIZE && py >= t.dy && py < t.dy + TILE_SIZE
            });
            assert!(covered, "point ({px}, {py}) not covered by any tile");
        }
    }

    #[test]
    fn visible_tiles_share_projection_zoom() {
        let pts = vec![TrackPoint::new(0.0, 0.0), TrackPoint::new(0.1, 0.1)];
        let proj = Projection::fit(&pts, 256, 256, 0.0).unwrap();
        assert!(proj
            .visible_tiles(256, 256)
            .iter()
            .all(|t| t.id.zoom == proj.zoom));
    }

    #[test]
    fn overzoomed_tiles_scale_up_and_cover_viewport() {
        let proj = Projection::centered(37.5665, 126.9780, 16, 640, 640);
        let tiles = proj.visible_tiles_at(14, 640, 640);
        assert!(!tiles.is_empty());
        assert!(tiles.iter().all(|t| t.id.zoom == 14 && t.size == 1024.0));

        let (px, py) = proj.project(37.5665, 126.9780);
        assert!(tiles
            .iter()
            .any(|t| px >= t.dx && px < t.dx + t.size && py >= t.dy && py < t.dy + t.size));
    }

    #[test]
    fn tile_zoom_above_projection_is_clamped() {
        let proj = Projection::centered(0.0, 0.0, 3, 256, 256);
        assert!(proj
            .visible_tiles_at(10, 256, 256)
            .iter()
            .all(|t| t.id.zoom == 3 && t.size == TILE_SIZE));
    }
}
