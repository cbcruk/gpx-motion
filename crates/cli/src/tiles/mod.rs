pub mod fetcher;
pub mod pmtiles;
pub mod vector;

pub use fetcher::TileFetcher;
pub use vector::VectorTileFetcher;

use gpx_motion_core::geo::mercator::Projection;
use gpx_motion_core::{
    compose, draw_vector_map, draw_vector_map_3d, place_labels_3d, place_labels_flat, Camera,
    Keepout, LabelStyle, MapStyle, PlacedLabel,
};
use tiny_skia::{Color, Pixmap};

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

    /// Fetch the vector tiles `camera` will see, so later draws can share the
    /// source read-only (e.g. across render threads).
    ///
    /// Raster tiles can't be drawn in perspective, so they are an error here.
    pub fn prefetch_3d(&mut self, camera: &Camera) -> Result<(), String> {
        match self {
            MapSource::Raster(_) => {
                Err("the 3D view needs --vector-tiles; raster tiles can't be tilted".to_string())
            }
            MapSource::Vector { tiles, .. } => {
                tiles.prefetch(&camera.ground_tiles(tiles.max_zoom));
                Ok(())
            }
        }
    }

    /// Fail early when the source can't be drawn in perspective (raster
    /// tiles can't be tilted).
    pub fn check_3d(&self) -> Result<(), String> {
        match self {
            MapSource::Raster(_) => {
                Err("the 3D view needs --vector-tiles; raster tiles can't be tilted".to_string())
            }
            MapSource::Vector { .. } => Ok(()),
        }
    }

    /// Draw the prefetched ground under `camera` (no fog; see
    /// [`MapSource::fog_color`]).
    pub fn draw_3d(&self, target: &mut Pixmap, camera: &Camera, opacity: f32) {
        if let MapSource::Vector { tiles, style } = self {
            let decoded = tiles.cached(&camera.ground_tiles(tiles.max_zoom));
            draw_vector_map_3d(target, &decoded, style, camera, opacity);
        }
    }

    /// Labels for the vector map under `proj` (none for raster maps), kept
    /// clear of `keepout`. Call after [`MapSource::draw`] fetched the tiles.
    pub fn labels_2d(
        &self,
        proj: &Projection,
        canvas: (u32, u32),
        style: &LabelStyle,
        keepout: &Keepout,
    ) -> Vec<PlacedLabel> {
        let MapSource::Vector { tiles, .. } = self else {
            return Vec::new();
        };
        let decoded = tiles.cached(&proj.visible_tiles_at(tiles.max_zoom, canvas.0, canvas.1));
        place_labels_flat(&decoded, style, canvas, keepout)
    }

    /// Labels for the prefetched vector ground under `camera`, standing on
    /// `ground` (world-pixel height at a world point), smaller with distance
    /// and dropped toward the horizon fog.
    pub fn labels_3d(
        &self,
        camera: &Camera,
        canvas: (u32, u32),
        style: &LabelStyle,
        keepout: &Keepout,
        ground: impl Fn(f64, f64) -> f64,
    ) -> Vec<PlacedLabel> {
        let MapSource::Vector { tiles, .. } = self else {
            return Vec::new();
        };
        let decoded = tiles.cached(&camera.ground_tiles(tiles.max_zoom));
        place_labels_3d(&decoded, camera, style, canvas, keepout, ground)
    }

    /// Color the far ground fades into: the vector style's background, so the
    /// edge of the fetched tiles never shows.
    pub fn fog_color(&self) -> Option<Color> {
        match self {
            MapSource::Vector { style, .. } => Some(style.background),
            MapSource::Raster(_) => None,
        }
    }
}
