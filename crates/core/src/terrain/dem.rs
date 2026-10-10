use std::collections::HashMap;

use crate::tiles::TileId;

/// Edge length of a DEM tile in samples.
const TILE: usize = 256;

/// A digital elevation model assembled from Terrarium-encoded tiles at one
/// zoom level.
///
/// Terrarium PNGs store meters as `R·256 + G + B/256 − 32768` (the format of
/// the open AWS Terrain Tiles). Fetching and PNG decoding happen outside the
/// core; [`Dem::add_terrarium`] takes the decoded pixels.
#[derive(Debug, Clone, Default)]
pub struct Dem {
    zoom: u32,
    tiles: HashMap<(u32, u32), Vec<f32>>,
}

impl Dem {
    /// An empty model whose tiles are all at `zoom`.
    pub fn new(zoom: u32) -> Self {
        Self {
            zoom,
            tiles: HashMap::new(),
        }
    }

    /// Zoom level of the model's tiles.
    pub fn zoom(&self) -> u32 {
        self.zoom
    }

    /// Add a decoded 256×256 Terrarium tile as RGBA bytes (straight or
    /// premultiplied; the tiles are opaque). Tiles at another zoom or of the
    /// wrong size are ignored, and `false` is returned.
    pub fn add_terrarium(&mut self, id: TileId, rgba: &[u8]) -> bool {
        if id.zoom != self.zoom || rgba.len() != TILE * TILE * 4 {
            return false;
        }
        let heights = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&[r, g, b, _]| r as f32 * 256.0 + g as f32 + b as f32 / 256.0 - 32768.0)
            .collect();
        self.tiles.insert((id.x, id.y), heights);
        true
    }

    /// Elevation in meters at a global Web Mercator pixel position at the
    /// model's zoom, bilinear between samples, or `None` where a needed tile
    /// is missing.
    pub fn elevation_at_pixel(&self, x: f64, y: f64) -> Option<f64> {
        let (x, y) = (x - 0.5, y - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.sample(x0, y0)?;
        let b = self.sample(x0 + 1, y0)?;
        let c = self.sample(x0, y0 + 1)?;
        let d = self.sample(x0 + 1, y0 + 1)?;
        let top = a + (b - a) * fx;
        let bottom = c + (d - c) * fx;
        Some(top + (bottom - top) * fy)
    }

    fn sample(&self, x: i64, y: i64) -> Option<f64> {
        let max = (TILE as i64) << self.zoom;
        let (x, y) = (x.rem_euclid(max), y.clamp(0, max - 1));
        let (tx, ty) = ((x / TILE as i64) as u32, (y / TILE as i64) as u32);
        let tile = self.tiles.get(&(tx, ty))?;
        let i = (y as usize % TILE) * TILE + x as usize % TILE;
        Some(tile[i] as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terrarium(meters: impl Fn(usize, usize) -> f64) -> Vec<u8> {
        let mut out = Vec::with_capacity(TILE * TILE * 4);
        for y in 0..TILE {
            for x in 0..TILE {
                let v = meters(x, y) + 32768.0;
                let r = (v / 256.0).floor();
                let g = (v - r * 256.0).floor();
                let b = ((v - r * 256.0 - g) * 256.0).round();
                out.extend_from_slice(&[r as u8, g as u8, b as u8, 0xff]);
            }
        }
        out
    }

    #[test]
    fn decodes_terrarium_and_interpolates() {
        let mut dem = Dem::new(1);
        let id = TileId {
            zoom: 1,
            x: 0,
            y: 0,
        };
        assert!(dem.add_terrarium(id, &terrarium(|x, _| x as f64 * 2.0 - 10.0)));
        assert_eq!(dem.elevation_at_pixel(10.5, 3.5), Some(10.0));
        let mid = dem.elevation_at_pixel(11.0, 3.5).unwrap();
        assert!((mid - 11.0).abs() < 1e-3, "{mid}");
    }

    #[test]
    fn missing_tiles_and_wrong_zoom_are_none() {
        let mut dem = Dem::new(2);
        let wrong = TileId {
            zoom: 3,
            x: 0,
            y: 0,
        };
        assert!(!dem.add_terrarium(wrong, &terrarium(|_, _| 0.0)));
        assert_eq!(dem.elevation_at_pixel(10.0, 10.0), None);
    }

    #[test]
    fn interpolates_across_tile_edges() {
        let mut dem = Dem::new(1);
        for x in 0..2 {
            let id = TileId { zoom: 1, x, y: 0 };
            dem.add_terrarium(id, &terrarium(|_, _| x as f64 * 100.0));
        }
        let edge = dem.elevation_at_pixel(256.0, 10.0).unwrap();
        assert!((edge - 50.0).abs() < 1e-3, "{edge}");
    }
}
