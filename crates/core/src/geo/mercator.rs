use std::f64::consts::PI;

use crate::gpx::model::TrackPoint;
use crate::tiles::{TileId, TilePlacement};

pub const TILE_SIZE: f64 = 256.0;
pub const MAX_ZOOM: f64 = 22.0;

/// The whole tile zoom to draw a fractional `zoom` with: the nearest level,
/// so tiles are scaled by `0.71..=1.41` rather than up to 2× (which blurs
/// raster labels). Negative zooms map to 0.
pub fn tile_zoom_for(zoom: f64) -> u32 {
    zoom.max(0.0).round() as u32
}

/// Zoom levels over which the map fades from one tile zoom into the next, so
/// a zooming view never swaps its detail in one frame.
pub const LEVEL_FADE: f64 = 0.3;

/// Tile zooms to draw world `zoom` from: the one `level` picks, and near a
/// switch the finer one it switches to, with how far that has faded in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileLevels {
    /// The tile zoom drawn in full.
    pub base: u32,
    /// The finer tile zoom drawn over it, and its opacity (`0..1`).
    pub finer: Option<(u32, f32)>,
}

/// The tile zooms for world `zoom`, where `level` maps a world zoom to the
/// tile zoom drawn there (rising with the zoom, e.g. [`tile_zoom_for`]).
///
/// Within [`LEVEL_FADE`] around a zoom where `level` steps up, both levels
/// are drawn and the finer one eases in across the band, reaching full
/// opacity where `level` alone takes over.
pub fn tile_levels(zoom: f64, level: impl Fn(f64) -> u32) -> TileLevels {
    let half = LEVEL_FADE / 2.0;
    let (lo, hi) = (level(zoom - half), level(zoom + half));
    if hi <= lo || !zoom.is_finite() {
        return TileLevels {
            base: level(zoom),
            finer: None,
        };
    }
    let (mut below, mut above) = (zoom - half, zoom + half);
    for _ in 0..40 {
        let mid = (below + above) / 2.0;
        if level(mid) > lo {
            above = mid;
        } else {
            below = mid;
        }
    }
    let t = ((zoom - above) / LEVEL_FADE + 0.5).clamp(0.0, 1.0);
    TileLevels {
        base: lo,
        finer: Some((hi, (t * t * (3.0 - 2.0 * t)) as f32)),
    }
}

/// Earth's equatorial circumference in meters, as Web Mercator scales it.
pub const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;

/// Width (and height) of the whole world in global pixels at the given
/// (possibly fractional) zoom.
pub fn world_size(zoom: f64) -> f64 {
    TILE_SIZE * 2f64.powf(zoom)
}

/// Global Web Mercator x pixel for a longitude at the given (possibly
/// fractional) zoom.
pub fn lon_to_pixel(lon: f64, zoom: f64) -> f64 {
    (lon + 180.0) / 360.0 * world_size(zoom)
}

/// Global Web Mercator y pixel for a latitude at the given (possibly
/// fractional) zoom.
pub fn lat_to_pixel(lat: f64, zoom: f64) -> f64 {
    let lat_rad = lat.to_radians();
    (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / PI) / 2.0 * world_size(zoom)
}

/// Longitude of a global Web Mercator x pixel; the inverse of
/// [`lon_to_pixel`].
pub fn pixel_to_lon(x: f64, zoom: f64) -> f64 {
    x / world_size(zoom) * 360.0 - 180.0
}

/// Latitude of a global Web Mercator y pixel; the inverse of
/// [`lat_to_pixel`].
pub fn pixel_to_lat(y: f64, zoom: f64) -> f64 {
    (PI * (1.0 - 2.0 * y / world_size(zoom)))
        .sinh()
        .atan()
        .to_degrees()
}

/// Ground meters one world pixel spans at a latitude (in degrees) and zoom.
pub fn meters_per_pixel(lat: f64, zoom: f64) -> f64 {
    EARTH_CIRCUMFERENCE_M * lat.to_radians().cos() / world_size(zoom)
}

/// World pixels one ground meter spans at a latitude (in degrees) and zoom;
/// the reciprocal of [`meters_per_pixel`].
pub fn px_per_meter(lat: f64, zoom: f64) -> f64 {
    world_size(zoom) / (EARTH_CIRCUMFERENCE_M * lat.to_radians().cos())
}

/// Maps WGS84 coordinates into viewport pixel space at a fixed zoom and origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// World zoom; fractional zooms scale the 256px tile grid continuously.
    pub zoom: f64,
    offset_x: f64,
    offset_y: f64,
}

impl Projection {
    /// A projection at a fixed zoom centered on a coordinate — the basis for a
    /// camera that follows the moving marker (viewport smaller than the track).
    pub fn centered(lat: f64, lon: f64, zoom: f64, width: u32, height: u32) -> Self {
        Self {
            zoom,
            offset_x: lon_to_pixel(lon, zoom) - width as f64 / 2.0,
            offset_y: lat_to_pixel(lat, zoom) - height as f64 / 2.0,
        }
    }

    /// A projection at a fixed zoom whose viewport's top-left corner is the
    /// global pixel `(x, y)`.
    pub fn from_origin(zoom: f64, x: f64, y: f64) -> Self {
        Self {
            zoom,
            offset_x: x,
            offset_y: y,
        }
    }

    /// The view `s` of the way (`0..=1`) from `self` to `other`: the zoom
    /// moves linearly and the map zooms about the one point both views keep
    /// at the same screen position, so `0` (or NaN) and `1` give the two
    /// views back.
    pub fn zoom_between(&self, other: &Projection, s: f64) -> Projection {
        let s = if s.is_nan() { 0.0 } else { s.clamp(0.0, 1.0) };
        if s == 0.0 {
            return *self;
        }
        if s == 1.0 {
            return *other;
        }
        let zoom = self.zoom + (other.zoom - self.zoom) * s;
        let w = super::pivot_weight(
            2f64.powf(self.zoom - zoom),
            2f64.powf(self.zoom - other.zoom),
            s,
        );
        let (ax, ay) = (
            self.offset_x / 2f64.powf(self.zoom),
            self.offset_y / 2f64.powf(self.zoom),
        );
        let (bx, by) = (
            other.offset_x / 2f64.powf(other.zoom),
            other.offset_y / 2f64.powf(other.zoom),
        );
        let scale = 2f64.powf(zoom);
        Projection {
            zoom,
            offset_x: (ax + (bx - ax) * w) * scale,
            offset_y: (ay + (by - ay) * w) * scale,
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
        self.visible_tiles_at(self.tile_zoom(), width, height)
    }

    /// The whole tile zoom this projection draws with; see [`tile_zoom_for`].
    pub fn tile_zoom(&self) -> u32 {
        tile_zoom_for(self.zoom)
    }

    /// Tiles from `tile_zoom` overlapping the viewport, for sources whose
    /// maximum zoom is below the projection's.
    ///
    /// Each lower-zoom tile is overzoomed to cover `256 * 2^(zoom - tile_zoom)`
    /// viewport pixels (its [`TilePlacement::size`]). A `tile_zoom`
    /// above the projection's [`Projection::tile_zoom`] is clamped to it.
    pub fn visible_tiles_at(&self, tile_zoom: u32, width: u32, height: u32) -> Vec<TilePlacement> {
        self.tiles_of_zoom(tile_zoom.min(self.tile_zoom()), width, height)
    }

    /// Tiles from exactly `tile_zoom` overlapping the viewport, finer than
    /// [`Projection::tile_zoom`] too (e.g. one fading in; see
    /// [`tile_levels`]).
    pub fn tiles_of_zoom(&self, tile_zoom: u32, width: u32, height: u32) -> Vec<TilePlacement> {
        let n = 2i64.pow(tile_zoom);
        let size = TILE_SIZE * 2f64.powf(self.zoom - tile_zoom as f64);
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

    /// Choose the zoom at which the bounding box exactly fills the viewport
    /// (minus padding on each side) along its tighter axis, then center the
    /// track within it. The zoom is fractional, capped at [`MAX_ZOOM`].
    ///
    /// Returns `None` when there are fewer than one point to bound.
    pub fn fit(points: &[TrackPoint], width: u32, height: u32, padding: f64) -> Option<Self> {
        let (min_lat, max_lat, min_lon, max_lon) = bounds(points)?;
        let avail_w = (width as f64 - 2.0 * padding).max(1.0);
        let avail_h = (height as f64 - 2.0 * padding).max(1.0);

        let span_x = lon_to_pixel(max_lon, 0.0) - lon_to_pixel(min_lon, 0.0);
        // y grows downward: max_lat is the smaller pixel value.
        let span_y = lat_to_pixel(min_lat, 0.0) - lat_to_pixel(max_lat, 0.0);
        let scale = (avail_w / span_x).min(avail_h / span_y);
        let zoom = if scale.is_finite() {
            scale.log2().clamp(0.0, MAX_ZOOM)
        } else {
            MAX_ZOOM
        };

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
        assert!((lon_to_pixel(0.0, 0.0) - 128.0).abs() < 1e-9);
        assert!((lat_to_pixel(0.0, 0.0) - 128.0).abs() < 1e-9);
    }

    #[test]
    fn world_size_doubles_per_zoom() {
        assert_eq!(world_size(0.0), TILE_SIZE);
        assert_eq!(world_size(3.0), 2048.0);
        assert!((world_size(1.5) - 256.0 * 2f64.sqrt() * 2.0).abs() < 1e-9);
    }

    #[test]
    fn pixel_to_lat_lon_invert_the_projection() {
        for zoom in [0.0, 7.3, 15.0] {
            for (lat, lon) in [
                (0.0, 0.0),
                (37.5665, 126.978),
                (-33.86, -151.2),
                (84.0, 179.9),
            ] {
                let back_lon = pixel_to_lon(lon_to_pixel(lon, zoom), zoom);
                let back_lat = pixel_to_lat(lat_to_pixel(lat, zoom), zoom);
                assert!((back_lon - lon).abs() < 1e-9, "{lon} -> {back_lon}");
                assert!((back_lat - lat).abs() < 1e-9, "{lat} -> {back_lat}");
            }
        }
    }

    #[test]
    fn meters_per_pixel_matches_the_web_mercator_scale() {
        assert!((meters_per_pixel(0.0, 0.0) - 156_543.033_93).abs() < 1e-2);
        assert!((meters_per_pixel(60.0, 1.0) - 156_543.033_93 / 4.0).abs() < 1e-2);
        let k = px_per_meter(37.5, 14.0) * meters_per_pixel(37.5, 14.0);
        assert!((k - 1.0).abs() < 1e-12);
    }

    #[test]
    fn higher_zoom_doubles_pixel_span() {
        let a = lon_to_pixel(10.0, 5.0) - lon_to_pixel(0.0, 5.0);
        let b = lon_to_pixel(10.0, 6.0) - lon_to_pixel(0.0, 6.0);
        assert!((b - 2.0 * a).abs() < 1e-6);
    }

    #[test]
    fn zoom_between_keeps_its_ends_and_a_fixed_pivot() {
        let wide = Projection::centered(37.55, 126.98, 11.3, 640, 480);
        let close = Projection::centered(37.57, 127.01, 15.0, 640, 480);
        assert_eq!(wide.zoom_between(&close, 0.0), wide);
        assert_eq!(wide.zoom_between(&close, 1.0), close);
        assert_eq!(wide.zoom_between(&close, f64::NAN), wide);
        // The pivot is the coordinate both ends draw at the same pixel; it
        // must sit there for every step in between too.
        let k = 2f64.powf(wide.zoom - close.zoom);
        let px = (close.offset_x * k - wide.offset_x) / (1.0 - k);
        let py = (close.offset_y * k - wide.offset_y) / (1.0 - k);
        let world = |p: &Projection| {
            let unit = 2f64.powf(p.zoom);
            ((p.offset_x + px) / unit, (p.offset_y + py) / unit)
        };
        let pivot = world(&wide);
        for s in [0.1, 0.37, 0.5, 0.9] {
            let (x, y) = world(&wide.zoom_between(&close, s));
            assert!(
                (x - pivot.0).abs() < 1e-9 && (y - pivot.1).abs() < 1e-9,
                "s {s}"
            );
        }
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
    fn fit_fills_the_tighter_axis_exactly() {
        let pts = vec![
            TrackPoint::new(37.5610, 126.9700),
            TrackPoint::new(37.5700, 126.9850),
        ];
        let proj = Projection::fit(&pts, 640, 480, 40.0).unwrap();
        assert!(proj.zoom.fract() != 0.0);
        let (x0, y0) = proj.project(37.5700, 126.9700);
        let (x1, y1) = proj.project(37.5610, 126.9850);
        let (w, h) = (x1 - x0, y1 - y0);
        assert!(
            (w - 560.0).abs() < 1e-6 || (h - 400.0).abs() < 1e-6,
            "{w} x {h}"
        );
        assert!(w <= 560.0 + 1e-6 && h <= 400.0 + 1e-6);
    }

    #[test]
    fn tile_zoom_rounds_to_the_nearest_level() {
        assert_eq!(tile_zoom_for(14.4), 14);
        assert_eq!(tile_zoom_for(14.6), 15);
        assert_eq!(tile_zoom_for(14.999_999_999_999), 15);
        assert_eq!(tile_zoom_for(-1.0), 0);
        let proj = Projection::centered(37.5, 127.0, 15.95, 640, 640);
        let size = proj.visible_tiles(640, 640)[0].size;
        assert!((size - 256.0 * 2f64.powf(-0.05)).abs() < 1e-6, "{size}");
    }

    #[test]
    fn fit_empty_is_none() {
        assert!(Projection::fit(&[], 640, 640, 0.0).is_none());
    }

    #[test]
    fn centered_places_point_at_viewport_center() {
        let proj = Projection::centered(37.5665, 126.9780, 14.0, 640, 480);
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
            let covered = tiles
                .iter()
                .any(|t| px >= t.dx && px < t.dx + t.size && py >= t.dy && py < t.dy + t.size);
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
            .all(|t| t.id.zoom == proj.tile_zoom()));
    }

    #[test]
    fn overzoomed_tiles_scale_up_and_cover_viewport() {
        let proj = Projection::centered(37.5665, 126.9780, 16.0, 640, 640);
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
        let proj = Projection::centered(0.0, 0.0, 3.0, 256, 256);
        assert!(proj
            .visible_tiles_at(10, 256, 256)
            .iter()
            .all(|t| t.id.zoom == 3 && t.size == TILE_SIZE));
    }

    #[test]
    fn tile_levels_fade_across_a_switch() {
        let at = |z: f64| tile_levels(z, tile_zoom_for);
        assert_eq!(
            at(14.0),
            TileLevels {
                base: 14,
                finer: None
            }
        );
        let half = LEVEL_FADE / 2.0;
        assert_eq!(at(14.5 - half - 0.01).finer, None);
        let mid = at(14.5);
        assert_eq!(mid.base, 14);
        let (finer, mix) = mid.finer.unwrap();
        assert_eq!(finer, 15);
        assert!((mix - 0.5).abs() < 1e-3, "{mix}");
        // Opacity rises through the band and meets the plain level at its ends.
        let mixes: Vec<f32> = (0..=10)
            .map(|i| at(14.5 - half + 0.001 + i as f64 * (LEVEL_FADE - 0.002) / 10.0))
            .map(|levels| levels.finer.unwrap().1)
            .collect();
        assert!(mixes.windows(2).all(|w| w[0] < w[1]), "{mixes:?}");
        assert!(mixes[0] < 0.01 && mixes[10] > 0.99);
        assert_eq!(at(14.5 + half + 0.01).base, 15);
        // A capped level never switches.
        assert_eq!(tile_levels(14.5, |z| tile_zoom_for(z).min(12)).finer, None);
    }
}
