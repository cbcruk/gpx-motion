//! Browser (`wasm32`) bindings for the gpx-motion render core.
//!
//! The same I/O-free core that backs the native CLI compiles to WebAssembly
//! here: the browser drives the timeline (e.g. via `requestAnimationFrame`),
//! calls [`GpxRenderer::render_frame`], and blits the returned RGBA bytes onto a
//! `<canvas>` with `putImageData`. Tile fetching and video encoding stay on the
//! JS side (fetch + WebCodecs), mirroring the native split: the core says which
//! tiles cover the viewport ([`GpxRenderer::visible_tiles`]), JS hands back the
//! fetched MVT bytes ([`GpxRenderer::add_vector_tile`]), and the core renders
//! the basemap ([`GpxRenderer::render_map`]).

mod scene;

pub use scene::SceneRenderer;

use gpx_motion_core::geo::mercator::Projection;
use gpx_motion_core::{
    draw_overlay, draw_vector_map, parse, FrameStyle, MapStyle, TileId, TilePlacement, Timeline,
    VectorTile,
};
use tiny_skia::{Color, Pixmap};
use wasm_bindgen::prelude::*;

/// A map tile covering part of the viewport, with its top-left corner in
/// viewport pixels.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub struct Tile {
    pub zoom: u32,
    pub x: u32,
    pub y: u32,
    pub dx: f64,
    pub dy: f64,
    /// Edge length in viewport pixels: 256, or larger when overzoomed.
    pub size: f64,
}

impl From<TilePlacement> for Tile {
    fn from(t: TilePlacement) -> Self {
        Self {
            zoom: t.id.zoom,
            x: t.id.x,
            y: t.id.y,
            dx: t.dx,
            dy: t.dy,
            size: t.size,
        }
    }
}

impl From<&Tile> for TilePlacement {
    fn from(t: &Tile) -> Self {
        Self {
            id: TileId {
                zoom: t.zoom,
                x: t.x,
                y: t.y,
            },
            dx: t.dx,
            dy: t.dy,
            size: t.size,
        }
    }
}

/// A parsed GPX track ready to render frames at a fixed viewport.
#[wasm_bindgen]
pub struct GpxRenderer {
    timeline: Timeline,
    projection: Projection,
    style: FrameStyle,
    vector_tiles: Vec<(TilePlacement, VectorTile)>,
}

#[wasm_bindgen]
impl GpxRenderer {
    /// Parse GPX text and fit the whole track into a `width`×`height` viewport.
    #[wasm_bindgen(constructor)]
    pub fn new(gpx: &str, width: u32, height: u32) -> Result<GpxRenderer, JsError> {
        let points = parse(gpx).map_err(|e| JsError::new(&e.to_string()))?;
        let timeline = Timeline::build(&points)
            .ok_or_else(|| JsError::new("track needs two distinct points"))?;
        let style = FrameStyle {
            width,
            height,
            ..Default::default()
        };
        let projection = Projection::fit(&points, width, height, style.padding)
            .ok_or_else(|| JsError::new("could not fit track to viewport"))?;
        Ok(Self {
            timeline,
            projection,
            style,
            vector_tiles: Vec::new(),
        })
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.style.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.style.height
    }

    /// Real track duration in ms, or `undefined` for an untimed track.
    #[wasm_bindgen(getter, js_name = durationMs)]
    pub fn duration_ms(&self) -> Option<f64> {
        self.timeline.duration_ms
    }

    /// Map tiles (XYZ scheme) that cover the fitted viewport.
    ///
    /// Pass a source's `maxZoom` to get its deepest tiles overzoomed when the
    /// viewport zoom is higher; omit it for one 256px tile per grid cell.
    #[wasm_bindgen(js_name = visibleTiles)]
    pub fn visible_tiles(&self, max_zoom: Option<u32>) -> Vec<Tile> {
        let zoom = max_zoom.unwrap_or(self.projection.zoom);
        self.projection
            .visible_tiles_at(zoom, self.style.width, self.style.height)
            .into_iter()
            .map(Tile::from)
            .collect()
    }

    /// Decode an MVT tile fetched for `tile` and keep it for [`GpxRenderer::render_map`].
    #[wasm_bindgen(js_name = addVectorTile)]
    pub fn add_vector_tile(&mut self, tile: &Tile, bytes: &[u8]) -> Result<(), JsError> {
        let decoded = VectorTile::decode(bytes).map_err(|e| JsError::new(&e.to_string()))?;
        self.vector_tiles.push((TilePlacement::from(tile), decoded));
        Ok(())
    }

    /// Render the added vector tiles as a label-free basemap (`light` or
    /// `dark`), returning straight RGBA bytes for `putImageData`.
    #[wasm_bindgen(js_name = renderMap)]
    pub fn render_map(&self, style: &str) -> Result<Vec<u8>, JsError> {
        let style = MapStyle::preset(style)
            .ok_or_else(|| JsError::new(&format!("unknown map style `{style}`")))?;
        let tiles: Vec<(TilePlacement, &VectorTile)> =
            self.vector_tiles.iter().map(|(p, t)| (*p, t)).collect();
        let mut pixmap = self.blank();
        draw_vector_map(&mut pixmap, &tiles, &style, 1.0);
        Ok(to_straight_rgba(&pixmap))
    }

    /// Override the tail color (RGB, `0..=255`).
    #[wasm_bindgen(js_name = setTailColor)]
    pub fn set_tail_color(&mut self, r: u8, g: u8, b: u8) {
        self.style.tail_color = Color::from_rgba8(r, g, b, 0xff);
    }

    /// `[lat, lon]` of the marker at progress `u` (`0..=1`).
    #[wasm_bindgen(js_name = markerPosition)]
    pub fn marker_position(&self, u: f64) -> Vec<f64> {
        let (lat, lon) = self.timeline.position_at(u);
        vec![lat, lon]
    }

    /// Render a full frame (solid background + tail + marker) at progress `u`,
    /// returning straight (non-premultiplied) RGBA bytes for `putImageData`.
    #[wasm_bindgen(js_name = renderFrame)]
    pub fn render_frame(&self, u: f64, tail_u: f64) -> Vec<u8> {
        let mut pixmap = self.blank();
        pixmap.fill(self.style.background);
        draw_overlay(
            &mut pixmap,
            &self.timeline,
            &self.projection,
            u,
            tail_u,
            &self.style,
        );
        to_straight_rgba(&pixmap)
    }

    /// Render only the tail + marker over a transparent canvas, so the browser
    /// can composite it on top of its own map layer.
    #[wasm_bindgen(js_name = renderOverlay)]
    pub fn render_overlay(&self, u: f64, tail_u: f64) -> Vec<u8> {
        let mut pixmap = self.blank();
        draw_overlay(
            &mut pixmap,
            &self.timeline,
            &self.projection,
            u,
            tail_u,
            &self.style,
        );
        to_straight_rgba(&pixmap)
    }

    fn blank(&self) -> Pixmap {
        Pixmap::new(self.style.width, self.style.height).expect("viewport must be non-zero")
    }
}

/// Convert a premultiplied tiny-skia pixmap into straight RGBA bytes as expected
/// by the Canvas `ImageData` API.
fn to_straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity((pixmap.width() * pixmap.height() * 4) as usize);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}
