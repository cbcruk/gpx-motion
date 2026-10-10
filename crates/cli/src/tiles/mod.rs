pub mod fetcher;
pub mod vector;

pub use fetcher::TileFetcher;
pub use vector::VectorTileFetcher;

use gpx_motion_core::geo::mercator::Projection;
use gpx_motion_core::{compose, draw_vector_map, MapStyle};
use tiny_skia::Pixmap;

/// Where the background map comes from.
pub enum MapSource {
    /// Pre-rendered image tiles (TMS/XYZ).
    Raster(TileFetcher),
    /// MVT tiles rendered locally with a label-free style.
    Vector {
        tiles: VectorTileFetcher,
        style: MapStyle,
    },
}

impl MapSource {
    /// Draw the map covering `proj`'s viewport onto `target` at `opacity`.
    pub fn draw(&mut self, target: &mut Pixmap, proj: &Projection, opacity: f32) {
        let (width, height) = (target.width(), target.height());
        match self {
            MapSource::Raster(fetcher) => {
                let tiles = fetcher.fetch_all(&proj.visible_tiles(width, height));
                compose(target, &tiles, opacity);
            }
            MapSource::Vector { tiles, style } => {
                let placements = proj.visible_tiles_at(tiles.max_zoom, width, height);
                let decoded = tiles.fetch_all(&placements);
                draw_vector_map(target, &decoded, style, opacity);
            }
        }
    }
}
