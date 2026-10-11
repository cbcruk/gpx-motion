//! Map textures for the tiles a WebGL scene drapes over its ground, drawn
//! from vector tiles (in a worker, typically) one ground tile at a time.

use std::collections::HashMap;

use gpx_motion_core::geo::mercator::Projection;
use gpx_motion_core::{draw_vector_map, MapStyle, TileId, TilePlacement, VectorTile};
use tiny_skia::Pixmap;
use wasm_bindgen::prelude::*;

use crate::to_straight_rgba;

/// Device pixels per CSS pixel of a ground texture: 512 texels per tile,
/// with lines as wide as a 2× map's.
const TEXTURE_RATIO: f64 = 2.0;

/// Texels across a ground tile's texture.
pub const TEXTURE_PX: u32 = (256.0 * TEXTURE_RATIO) as u32;

/// Draws ground tile textures in a map style from the vector tiles given to
/// it.
#[wasm_bindgen]
pub struct GroundTiles {
    style: MapStyle,
    tiles: HashMap<TileId, VectorTile>,
}

#[wasm_bindgen]
impl GroundTiles {
    /// Ground textures in `style`: a preset name (`light`, `dark`) or a JSON
    /// style document.
    #[wasm_bindgen(constructor)]
    pub fn new(style: &str) -> Result<GroundTiles, JsError> {
        let style = MapStyle::resolve(style)
            .map_err(|e| JsError::new(&e.to_string()))?
            .with_pixel_ratio(TEXTURE_RATIO as f32);
        Ok(Self {
            style,
            tiles: HashMap::new(),
        })
    }

    /// Texels across each texture.
    #[wasm_bindgen(getter, js_name = textureSize)]
    pub fn texture_size(&self) -> u32 {
        TEXTURE_PX
    }

    /// Decode and keep MVT tile `zoom/x/y`.
    #[wasm_bindgen(js_name = addTile)]
    pub fn add_tile(&mut self, zoom: u32, x: u32, y: u32, bytes: &[u8]) -> Result<(), JsError> {
        let tile = VectorTile::decode(bytes).map_err(|e| JsError::new(&e.to_string()))?;
        self.tiles.insert(TileId { zoom, x, y }, tile);
        Ok(())
    }

    /// The texture of ground tile `zoom/x/y` as straight RGBA, drawn from the
    /// kept vector tiles at `dataZoom` (at most `zoom`; overzoomed when
    /// lower). Ground the kept tiles don't cover shows the background.
    pub fn render(&self, zoom: u32, x: u32, y: u32, data_zoom: u32) -> Vec<u8> {
        let side = TEXTURE_PX as f64;
        let projection = Projection::from_origin(
            zoom as f64 + TEXTURE_RATIO.log2(),
            x as f64 * side,
            y as f64 * side,
        );
        let mut pixmap = Pixmap::new(TEXTURE_PX, TEXTURE_PX).expect("texture size is valid");
        pixmap.fill(self.style.background);
        let tiles: Vec<(TilePlacement, &VectorTile)> = projection
            .visible_tiles_at(data_zoom.min(zoom), TEXTURE_PX, TEXTURE_PX)
            .into_iter()
            .filter_map(|p| self.tiles.get(&p.id).map(|tile| (p, tile)))
            .collect();
        draw_vector_map(&mut pixmap, &tiles, &self.style, 1.0);
        to_straight_rgba(&pixmap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_fill_with_the_background_without_tiles() {
        let Ok(ground) = GroundTiles::new("dark") else {
            panic!("the dark preset resolves");
        };
        let rgba = ground.render(14, 13970, 6344, 14);
        assert_eq!(rgba.len(), (TEXTURE_PX * TEXTURE_PX * 4) as usize);
        let bg = ground.style.background;
        let first = [
            (bg.red() * 255.0).round() as u8,
            (bg.green() * 255.0).round() as u8,
            (bg.blue() * 255.0).round() as u8,
            255,
        ];
        assert_eq!(&rgba[..4], &first);
    }
}
