//! Map textures for the tiles a WebGL scene drapes over its ground, drawn
//! from vector tiles (in a worker, typically) one ground tile at a time,
//! with the buildings the style raises over each.

use std::collections::HashMap;

use gpx_motion_core::geo::mercator::Projection;
use gpx_motion_core::{
    draw_vector_map, extrude_within, MapStyle, TileId, TilePlacement, VectorTile,
};
use tiny_skia::Pixmap;
use wasm_bindgen::prelude::*;

use crate::layout::{BuildingField, BUILDING_STRIDE};
use crate::{js_error, to_straight_rgba};

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
    /// The style without the extrusions it raises, for the textures.
    ground: MapStyle,
    tiles: HashMap<TileId, VectorTile>,
}

#[wasm_bindgen]
impl GroundTiles {
    /// Ground textures in `style`: a preset name (`light`, `dark`) or a JSON
    /// style document.
    #[wasm_bindgen(constructor)]
    pub fn new(style: &str) -> Result<GroundTiles, JsError> {
        let style = MapStyle::resolve(style)
            .map_err(js_error)?
            .with_pixel_ratio(TEXTURE_RATIO as f32);
        Ok(Self {
            ground: style.without_extrusions(),
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
        let tile = VectorTile::decode(bytes).map_err(js_error)?;
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
        pixmap.fill(self.ground.background);
        let tiles: Vec<(TilePlacement, &VectorTile)> = projection
            .visible_tiles_at(data_zoom.min(zoom), TEXTURE_PX, TEXTURE_PX)
            .into_iter()
            .filter_map(|p| self.tiles.get(&p.id).map(|tile| (p, tile)))
            .collect();
        draw_vector_map(&mut pixmap, &tiles, None, &self.ground, 1.0);
        to_straight_rgba(&pixmap)
    }

    /// The buildings standing on ground tile `zoom/x/y`, from the kept vector
    /// tile at `dataZoom` covering it (cut at the ground tile's edges), as
    /// triangles laid out by [`BuildingField`]: `x` and `y` in world pixels
    /// at `zoom` from the tile's top-left corner, height above the building's
    /// foot in the same pixels, its building's index in place of the foot
    /// (corners of one building come together), and the face's `r`, `g`,
    /// `b`, `a` (`0..=1`, straight). Empty when the style raises nothing here.
    pub fn buildings(&self, zoom: u32, x: u32, y: u32, data_zoom: u32) -> Vec<f32> {
        if !self.style.extrudes() {
            return Vec::new();
        }
        let data_zoom = data_zoom.min(zoom);
        let shift = zoom - data_zoom;
        if shift >= 32 {
            return Vec::new();
        }
        let id = TileId {
            zoom: data_zoom,
            x: x >> shift,
            y: y >> shift,
        };
        let Some(tile) = self.tiles.get(&id) else {
            return Vec::new();
        };
        let size = 256.0 * (1u64 << shift) as f64;
        let placement = TilePlacement {
            id,
            dx: id.x as f64 * size,
            dy: id.y as f64 * size,
            size,
        };
        let (left, top) = (x as f64 * 256.0, y as f64 * 256.0);
        let within = Some([left, top, left + 256.0, top + 256.0]);
        // Rules draw at the zoom the texture is drawn at.
        let texture_zoom = zoom as f64 + TEXTURE_RATIO.log2();
        let layers = extrude_within(&[(placement, tile)], &self.style, texture_zoom, within);
        let mut out = Vec::new();
        let mut first = 0u32;
        for layer in &layers {
            for facet in layer.facets(zoom as f64, |_, _| 0.0) {
                let c = facet.color;
                for [cx, cy, h] in facet.corners {
                    let mut corner = [0.0; BUILDING_STRIDE];
                    corner[BuildingField::X as usize] = (cx - left) as f32;
                    corner[BuildingField::Y as usize] = (cy - top) as f32;
                    corner[BuildingField::Height as usize] = h as f32;
                    corner[BuildingField::Foot as usize] = (first + facet.prism) as f32;
                    corner[BuildingField::R as usize] = c.red();
                    corner[BuildingField::G as usize] = c.green();
                    corner[BuildingField::B as usize] = c.blue();
                    corner[BuildingField::A as usize] = layer.opacity;
                    out.extend(corner);
                }
            }
            first += layer.prisms.len() as u32;
        }
        out
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

    /// An MVT tile with one `building` square from 1024 to 3072 (of 4096),
    /// 20 m tall.
    fn building_tile() -> Vec<u8> {
        fn varint(mut n: u64, out: &mut Vec<u8>) {
            while n >= 0x80 {
                out.push((n as u8) | 0x80);
                n >>= 7;
            }
            out.push(n as u8);
        }
        fn field(tag: u64, payload: &[u8], out: &mut Vec<u8>) {
            varint(tag << 3 | 2, out);
            varint(payload.len() as u64, out);
            out.extend_from_slice(payload);
        }
        let zigzag = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let mut geometry = Vec::new();
        for v in [
            9,
            zigzag(1024),
            zigzag(1024),
            26,
            zigzag(2048),
            0,
            0,
            zigzag(2048),
            zigzag(-2048),
            0,
            15,
        ] {
            varint(v, &mut geometry);
        }
        let mut feature = Vec::new();
        field(2, &[0, 0], &mut feature);
        feature.extend([3 << 3, 3]);
        field(4, &geometry, &mut feature);
        let mut value = Vec::new();
        value.push(4 << 3);
        varint(20, &mut value);
        let mut layer = Vec::new();
        field(1, b"building", &mut layer);
        field(2, &feature, &mut layer);
        field(3, b"render_height", &mut layer);
        field(4, &value, &mut layer);
        layer.extend([15 << 3, 2, 5 << 3]);
        varint(4096, &mut layer);
        let mut tile = Vec::new();
        field(3, &layer, &mut tile);
        tile
    }

    #[test]
    fn buildings_rise_from_extruding_styles_only() {
        let style = r##"{"background": "#000", "rules": [{"layer": "building",
            "fill": "#ccc", "extrude": {"height": "render_height"}}]}"##;
        let Ok(mut ground) = GroundTiles::new(style) else {
            panic!("the style resolves");
        };
        assert!(ground.add_tile(14, 100, 200, &building_tile()).is_ok());
        let data = ground.buildings(14, 100, 200, 14);
        // Four walls of two triangles, and a roof of two.
        assert_eq!(data.len(), 10 * 3 * BUILDING_STRIDE);
        let (corners, _) = data.as_chunks::<BUILDING_STRIDE>();
        assert!(corners
            .iter()
            .all(|c| (64.0..=192.0).contains(&c[0]) && c[3] == 0.0));
        assert!(corners.iter().any(|c| c[2] > 0.0));
        // Overzoomed: only the quarter of the square in each child tile.
        let quarter = ground.buildings(15, 200, 400, 14);
        let (corners, _) = quarter.as_chunks::<BUILDING_STRIDE>();
        assert!(!corners.is_empty());
        assert!(corners.iter().all(|c| (128.0..=256.0).contains(&c[0])));
        // The texture leaves the footprint out, and other styles raise nothing.
        let texture = ground.render(14, 100, 200, 14);
        assert_eq!(texture[(256 * 512 + 256) * 4], 0);
        let Ok(mut flat) = GroundTiles::new("dark") else {
            panic!("the dark preset resolves");
        };
        assert!(flat.add_tile(14, 100, 200, &building_tile()).is_ok());
        assert!(flat.buildings(14, 100, 200, 14).is_empty());
    }
}
