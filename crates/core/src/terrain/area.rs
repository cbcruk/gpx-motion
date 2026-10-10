use crate::geo::mercator::{lat_to_pixel, lon_to_pixel, Projection};
use crate::gpx::TrackPoint;
use crate::tiles::TileId;

/// Map texture around the route, as a multiple of the route's larger extent
/// added on each side.
const MARGIN: f64 = 0.6;
/// Highest zoom Terrarium tiles are published at.
const MAX_DEM_ZOOM: u32 = 15;

/// The world-pixel rectangle around a route that terrain covers at a zoom,
/// with the DEM tiles that span it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainArea {
    /// World zoom of the area (the camera's).
    pub zoom: f64,
    /// Top-left corner in world pixels at `zoom`.
    pub origin: (f64, f64),
    /// Size in world pixels, one texel each in the map texture.
    pub width: u32,
    pub height: u32,
    /// Zoom of the DEM tiles: one level coarser than the map, at most 15.
    pub dem_zoom: u32,
}

impl TerrainArea {
    /// The area around `points` at `zoom`: their bounds plus a margin, which
    /// shrinks to keep the larger side within `max_side` pixels where it
    /// can. `None` without points.
    pub fn around(points: &[TrackPoint], zoom: f64, max_side: f64) -> Option<Self> {
        let first = points.first()?;
        let start = (lon_to_pixel(first.lon, zoom), lat_to_pixel(first.lat, zoom));
        let (x0, y0, x1, y1) = points.iter().fold(
            (start.0, start.1, start.0, start.1),
            |(x0, y0, x1, y1), p| {
                let (x, y) = (lon_to_pixel(p.lon, zoom), lat_to_pixel(p.lat, zoom));
                (x0.min(x), y0.min(y), x1.max(x), y1.max(y))
            },
        );
        let extent = (x1 - x0).max(y1 - y0);
        let margin = (MARGIN * extent)
            .max(256.0)
            .min(((max_side - extent) / 2.0).max(0.0));
        let (x0, y0) = ((x0 - margin).floor(), (y0 - margin).floor());
        let (w, h) = ((x1 + margin).ceil() - x0, (y1 + margin).ceil() - y0);
        Some(Self {
            zoom,
            origin: (x0, y0),
            width: w as u32,
            height: h as u32,
            dem_zoom: (zoom.max(1.0).floor() as u32 - 1).min(MAX_DEM_ZOOM),
        })
    }

    /// Whether both sides are at most `max_side` pixels (a long route at a
    /// close zoom may not be).
    pub fn fits(&self, max_side: f64) -> bool {
        self.width as f64 <= max_side && self.height as f64 <= max_side
    }

    /// Projection that draws the 2D map into the area's texture.
    pub fn projection(&self) -> Projection {
        Projection::from_origin(self.zoom, self.origin.0, self.origin.1)
    }

    /// The DEM tiles covering the area plus one sample on each side, for
    /// bilinear lookups at the border.
    pub fn dem_tiles(&self) -> Vec<TileId> {
        let dem_zoom = self.dem_zoom;
        let scale = 2f64.powf(dem_zoom as f64 - self.zoom);
        let last = (1i64 << dem_zoom) - 1;
        let tile = |px: f64| ((px * scale / 256.0).floor() as i64).clamp(0, last) as u32;
        let (x0, y0) = self.origin;
        let (w, h) = (self.width as f64, self.height as f64);
        let (tx0, ty0) = (tile(x0 - 1.0 / scale), tile(y0 - 1.0 / scale));
        let (tx1, ty1) = (tile(x0 + w + 1.0 / scale), tile(y0 + h + 1.0 / scale));
        (ty0..=ty1)
            .flat_map(|y| {
                (tx0..=tx1).map(move |x| TileId {
                    zoom: dem_zoom,
                    x,
                    y,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_wraps_the_route_with_a_margin() {
        let points = [
            TrackPoint::new(37.55, 126.98),
            TrackPoint::new(37.56, 127.0),
        ];
        let area = TerrainArea::around(&points, 15.0, 4096.0).unwrap();
        let proj = area.projection();
        for p in &points {
            let (x, y) = proj.project(p.lat, p.lon);
            assert!(x > 200.0 && y > 200.0, "{x}, {y}");
            assert!(x < area.width as f64 - 200.0 && y < area.height as f64 - 200.0);
        }
        assert!(area.fits(4096.0));
        assert_eq!(area.dem_zoom, 14);
        let tiles = area.dem_tiles();
        assert!(tiles.iter().all(|t| t.zoom == 14));
        assert!(!tiles.is_empty());
        assert!(TerrainArea::around(&[], 15.0, 4096.0).is_none());
        let far = TerrainArea::around(&points, 22.0, 4096.0).unwrap();
        assert!(!far.fits(4096.0));
    }
}
