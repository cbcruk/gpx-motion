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
//!
//! # Naming
//!
//! Methods are named, in JS, for what they do:
//!
//! - `gl*`: everything [`SceneRenderer`] hands the 3D view each frame —
//!   GPU buffers (`glCamera`, `glTrack`, `glLabels`, …) and the CPU-drawn
//!   profile strip shown beside it (`glProfile`) — and the drawing
//!   constants that go with them (`glTailWidth`, `glBackground`, …).
//! - `render*`: an RGBA image drawn on the CPU by the 2D renderer or for a
//!   ground texture ([`GpxRenderer::render_frame`],
//!   [`GroundTiles::render_tile`], …); the scene's own image is `glProfile`,
//!   as part of the 3D view's frame.
//! - `set*`, `add*`, `clear*`: change the renderer's state.
//! - Anything else reads the state or lists what to fetch (`durationMs`,
//!   `followZoom`, `visibleTiles`, `labelTiles`, …).

mod ground;
pub mod layout;
mod scene;

pub use ground::GroundTiles;
pub use scene::{GlLabels, SceneRenderer};

use gpx_motion_core::mercator::Projection;
use gpx_motion_core::pmtiles::{Archive, Step, TileType, PREFIX_LEN};
use gpx_motion_core::{
    draw_labels, draw_overlay, draw_vector_map, parse, place_labels_flat, FrameStyle, Keepout,
    LabelStyle, MapStyle, TextFont, TileId, TilePlacement, Timeline, TrackPoint, VectorTile,
};
use tiny_skia::{Color, Pixmap, Rect};
use wasm_bindgen::prelude::*;

use layout::{StepField, StepKind, STEP_LEN};

/// A map tile covering part of the viewport, with its top-left corner in
/// viewport pixels.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub struct Tile {
    /// Zoom level.
    pub zoom: u32,
    /// Column, from the west edge.
    pub x: u32,
    /// Row, from the north edge.
    pub y: u32,
    /// Left edge in viewport pixels.
    pub dx: f64,
    /// Top edge in viewport pixels.
    pub dy: f64,
    /// Edge length in viewport pixels: 256, or larger when overzoomed.
    pub size: f64,
}

#[wasm_bindgen]
impl Tile {
    /// A tile placed with its top-left corner at `dx`, `dy` and `size`
    /// pixels across, e.g. to hand one from [`GpxRenderer::visible_tiles`]
    /// to a renderer in another thread.
    #[wasm_bindgen(constructor)]
    pub fn new(zoom: u32, x: u32, y: u32, dx: f64, dy: f64, size: f64) -> Tile {
        Self {
            zoom,
            x,
            y,
            dx,
            dy,
            size,
        }
    }
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

/// The font and options map labels are drawn with; colors follow the map
/// style at draw time.
#[derive(Clone)]
struct LabelFont {
    font: TextFont,
    size: f32,
    language: Option<String>,
}

impl LabelFont {
    /// Parse a TrueType/OpenType font (the first face of a collection).
    fn new(font: Vec<u8>, size: Option<f32>, language: Option<String>) -> Result<Self, JsError> {
        let font = TextFont::from_bytes(font, 0)
            .ok_or_else(|| JsError::new("not a usable TrueType/OpenType font"))?;
        Ok(Self {
            font,
            size: size
                .filter(|s| s.is_finite())
                .unwrap_or(LabelStyle::BROWSER_DEFAULT_SIZE)
                .clamp(LabelStyle::MIN_SIZE, LabelStyle::MAX_SIZE),
            language: language.filter(|l| !l.is_empty()),
        })
    }

    fn style(&self, background: Color, pixel_ratio: f32) -> LabelStyle {
        LabelStyle::contrasting(
            self.font.clone(),
            self.size,
            background,
            self.language.clone(),
            pixel_ratio,
        )
    }
}

/// A parsed GPX track ready to render frames at a fixed viewport.
#[wasm_bindgen]
pub struct GpxRenderer {
    timeline: Timeline,
    points: Vec<TrackPoint>,
    projection: Projection,
    style: FrameStyle,
    pixel_ratio: f32,
    vector_tiles: Vec<(TilePlacement, VectorTile)>,
    labels: Option<LabelFont>,
    /// Screen rectangles labels keep off (e.g. HTML over the canvas).
    label_keepout: Vec<Rect>,
}

#[wasm_bindgen]
impl GpxRenderer {
    /// Parse GPX text and fit the whole track into a `width`×`height` viewport
    /// of device pixels. `pixelRatio` (device pixels per CSS pixel, default
    /// `1`) scales the track, marker, padding, and map line widths, e.g.
    /// `devicePixelRatio` for a canvas sized in device pixels.
    ///
    /// # Errors
    ///
    /// Throws when `gpx` doesn't parse or holds fewer than two distinct points,
    /// or the track can't be fitted to the viewport.
    #[wasm_bindgen(constructor)]
    pub fn new(
        gpx: &str,
        width: u32,
        height: u32,
        pixel_ratio: Option<f32>,
    ) -> Result<GpxRenderer, JsError> {
        let ratio = pixel_ratio
            .filter(|r| r.is_finite() && *r > 0.0)
            .unwrap_or(1.0);
        let points = parse(gpx).map_err(js_error)?;
        let timeline = Timeline::build(&points)
            .ok_or_else(|| JsError::new("track needs two distinct points"))?;
        let style = FrameStyle {
            width,
            height,
            ..Default::default()
        }
        .scaled(ratio);
        let projection = Projection::fit(&points, width, height, style.padding)
            .ok_or_else(|| JsError::new("could not fit track to viewport"))?;
        Ok(Self {
            timeline,
            points,
            projection,
            style,
            pixel_ratio: ratio,
            vector_tiles: Vec::new(),
            labels: None,
            label_keepout: Vec::new(),
        })
    }

    /// Keep map labels off these rectangles (e.g. an attribution laid over
    /// the canvas): `x`, `y`, `width`, `height` each, in device pixels.
    /// Replaces the previous ones; empty clears them.
    #[wasm_bindgen(js_name = setLabelKeepout)]
    pub fn set_label_keepout(&mut self, rects: &[f32]) {
        self.label_keepout = keepout_rects(rects);
    }

    /// Canvas width in device pixels.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.style.width
    }

    /// Canvas height in device pixels.
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
    /// viewport zoom is higher; omit it for tiles at the nearest whole zoom,
    /// each `256·2^(zoom − tileZoom)` pixels across. With the `style` the
    /// map will be drawn in (as for [`GpxRenderer::render_map`]), a style
    /// imported from MapLibre gets the tile zoom MapLibre would use.
    #[wasm_bindgen(js_name = visibleTiles)]
    pub fn visible_tiles(&self, max_zoom: Option<u32>, style: Option<String>) -> Vec<Tile> {
        let data_zoom = style
            .and_then(|s| MapStyle::resolve(&s).ok())
            .and_then(|s| {
                s.with_pixel_ratio(self.pixel_ratio)
                    .data_zoom(self.projection.zoom)
            });
        let zoom = max_zoom.unwrap_or(self.projection.tile_zoom());
        let zoom = data_zoom.map_or(zoom, |d| zoom.min(d));
        self.projection
            .visible_tiles_at(zoom, self.style.width, self.style.height)
            .into_iter()
            .map(Tile::from)
            .collect()
    }

    /// Decode an MVT tile fetched for `tile` and keep it for [`GpxRenderer::render_map`].
    ///
    /// # Errors
    ///
    /// Throws when `bytes` isn't a valid MVT tile.
    #[wasm_bindgen(js_name = addVectorTile)]
    pub fn add_vector_tile(&mut self, tile: &Tile, bytes: &[u8]) -> Result<(), JsError> {
        let decoded = VectorTile::decode(bytes).map_err(js_error)?;
        self.vector_tiles.push((TilePlacement::from(tile), decoded));
        Ok(())
    }

    /// Render the added vector tiles as a basemap, returning straight RGBA
    /// bytes for `putImageData`. `style` is a preset name (`light`, `dark`)
    /// or a JSON style document. Place, peak, and water names are drawn too
    /// after [`GpxRenderer::set_labels`], kept off the route.
    ///
    /// # Errors
    ///
    /// Throws when `style` is neither a preset nor a valid style document.
    #[wasm_bindgen(js_name = renderMap)]
    pub fn render_map(&self, style: &str) -> Result<Vec<u8>, JsError> {
        let style = MapStyle::resolve(style)
            .map_err(js_error)?
            .with_pixel_ratio(self.pixel_ratio);
        let tiles: Vec<(TilePlacement, &VectorTile)> =
            self.vector_tiles.iter().map(|(p, t)| (*p, t)).collect();
        let mut pixmap = self.blank();
        draw_vector_map(&mut pixmap, &tiles, None, &style, 1.0);
        if let Some(labels) = &self.labels {
            let label_style = labels.style(style.background, self.pixel_ratio);
            let route = self.projection.project_path(&self.points);
            let keepout = Keepout::around(
                &self.label_keepout,
                vec![route],
                Vec::new(),
                self.style.tail_width,
                self.pixel_ratio,
            );
            let canvas = (self.style.width, self.style.height);
            let placed = place_labels_flat(&tiles, &label_style, canvas, &keepout);
            draw_labels(&mut pixmap, &placed, &label_style);
        }
        Ok(to_straight_rgba(&pixmap))
    }

    /// Draw map labels with this font (TrueType/OpenType bytes with glyphs
    /// for the map's script, e.g. Korean) on later [`GpxRenderer::render_map`]
    /// calls. `size` is in CSS pixels (default 12); `language` prefers
    /// `name:<language>` names (e.g. `en`) over the local `name`.
    ///
    /// # Errors
    ///
    /// Throws when `font` isn't a usable TrueType/OpenType font.
    #[wasm_bindgen(js_name = setLabels)]
    pub fn set_labels(
        &mut self,
        font: Vec<u8>,
        size: Option<f32>,
        language: Option<String>,
    ) -> Result<(), JsError> {
        self.labels = Some(LabelFont::new(font, size, language)?);
        Ok(())
    }

    /// Stop drawing map labels.
    #[wasm_bindgen(js_name = clearLabels)]
    pub fn clear_labels(&mut self) {
        self.labels = None;
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

/// A PMTiles v3 archive of MVT tiles read over HTTP range requests (or any
/// byte source) by JS: the archive names each byte range it needs, JS
/// fetches it and hands it back.
///
/// Open with the archive's first [`PmTiles::prefix_length`] bytes and its
/// root directory (usually inside them; see [`PmTiles::root_range`]), then
/// for each tile follow [`PmTiles::locate`].
#[wasm_bindgen]
pub struct PmTiles {
    archive: Archive,
}

#[wasm_bindgen]
impl PmTiles {
    /// Bytes to fetch first: the header and, normally, the root directory.
    #[wasm_bindgen(js_name = prefixLength)]
    pub fn prefix_length() -> f64 {
        PREFIX_LEN as f64
    }

    /// `[offset, length]` of the root directory, from the header bytes.
    ///
    /// # Errors
    ///
    /// Throws when `header` isn't a `PMTiles` v3 header.
    #[wasm_bindgen(js_name = rootRange)]
    pub fn root_range(header: &[u8]) -> Result<Vec<f64>, JsError> {
        let (offset, length) = Archive::root_range(header).map_err(js_error)?;
        Ok(vec![offset as f64, length as f64])
    }

    /// Open an archive of MVT tiles from its header bytes and its
    /// compressed root directory.
    ///
    /// # Errors
    ///
    /// Throws when the bytes aren't a readable `PMTiles` v3 archive or its tiles
    /// aren't MVT.
    #[wasm_bindgen(constructor)]
    pub fn new(header: &[u8], root: &[u8]) -> Result<PmTiles, JsError> {
        let archive = Archive::new(header, root).map_err(js_error)?;
        if archive.header().tile_type != TileType::Mvt {
            return Err(JsError::new("tiles are not Mapbox Vector Tiles (MVT)"));
        }
        Ok(Self { archive })
    }

    /// Highest zoom stored in the archive.
    #[wasm_bindgen(getter, js_name = maxZoom)]
    pub fn max_zoom(&self) -> u32 {
        u32::from(self.archive.header().max_zoom)
    }

    /// The next step for tile `z/x/y`, laid out by [`StepField`]: its
    /// [`StepKind`], then the byte range and the leaf key it names.
    ///
    /// # Errors
    ///
    /// Throws when leaf directories nest deeper than the format allows.
    pub fn locate(&self, z: u32, x: u32, y: u32) -> Result<Vec<f64>, JsError> {
        let (kind, offset, length, key) = match self.archive.locate(z, x, y).map_err(js_error)? {
            Step::Missing => (StepKind::Missing, 0, 0, 0),
            Step::Tile { offset, length } => (StepKind::Tile, offset, length, 0),
            Step::Leaf {
                offset,
                length,
                key,
            } => (StepKind::Leaf, offset, length, key),
        };
        let mut out = vec![0.0; STEP_LEN];
        out[StepField::Kind as usize] = f64::from(kind as u32);
        out[StepField::Offset as usize] = offset as f64;
        out[StepField::Length as usize] = length as f64;
        out[StepField::Key as usize] = key as f64;
        Ok(out)
    }

    /// Keep a leaf directory that [`PmTiles::locate`] asked for.
    ///
    /// # Errors
    ///
    /// Throws when `bytes` isn't a readable directory.
    #[wasm_bindgen(js_name = addLeaf)]
    pub fn add_leaf(&mut self, key: f64, bytes: &[u8]) -> Result<(), JsError> {
        self.archive.add_leaf(key as u64, bytes).map_err(js_error)
    }

    /// The decompressed MVT bytes from a tile's fetched range.
    ///
    /// # Errors
    ///
    /// Throws when `bytes` can't be decompressed.
    pub fn tile(&self, bytes: &[u8]) -> Result<Vec<u8>, JsError> {
        self.archive.tile(bytes).map_err(js_error)
    }
}

/// What a map style (a preset name or JSON) leaves out when used with
/// `renderMap` or `setMapStyle`: for an imported MapLibre style, the layers
/// and properties it skips or approximates; empty for gpx-motion styles.
///
/// # Errors
///
/// Throws when `style` is neither a preset nor a valid style document.
#[wasm_bindgen(js_name = mapStyleNotes)]
pub fn map_style_notes(style: &str) -> Result<Vec<String>, JsError> {
    MapStyle::resolve_with_notes(style)
        .map(|(_, notes)| notes)
        .map_err(js_error)
}

/// `e` as a JS `Error` carrying its message.
fn js_error(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

/// Rectangles from `x`, `y`, `width`, `height` quadruples (a trailing
/// partial one, or one with no area, is ignored).
fn keepout_rects(values: &[f32]) -> Vec<Rect> {
    let (quads, _) = values.as_chunks::<4>();
    quads
        .iter()
        .filter(|&&[_, _, w, h]| w > 0.0 && h > 0.0)
        .filter_map(|&[x, y, w, h]| Rect::from_xywh(x, y, w, h))
        .collect()
}

/// Convert a premultiplied tiny-skia pixmap into straight RGBA bytes as expected
/// by the Canvas `ImageData` API.
fn to_straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity((pixmap.width() * pixmap.height() * 4) as usize);
    for px in pixmap.pixels() {
        match px.alpha() {
            0 => out.extend_from_slice(&[0, 0, 0, 0]),
            0xff => out.extend_from_slice(&[px.red(), px.green(), px.blue(), 0xff]),
            _ => {
                let c = px.demultiply();
                out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepout_rects_come_in_fours() {
        let rects = keepout_rects(&[10.0, 20.0, 30.0, 40.0, 0.0, 0.0, 0.0, 5.0, 1.0]);
        assert_eq!(rects.len(), 1, "the empty and the partial one are dropped");
        assert_eq!((rects[0].left(), rects[0].bottom()), (10.0, 60.0));
    }

    #[test]
    fn maplibre_styles_pick_maplibre_tile_zooms() {
        let gpx = include_str!("../../../samples/namsan.gpx");
        let Ok(renderer) = GpxRenderer::new(gpx, 640, 640, Some(1.0)) else {
            panic!("renderer should build");
        };
        let zoom = |style: Option<&str>| {
            renderer.visible_tiles(Some(22), style.map(str::to_string))[0].zoom
        };
        let maplibre = r##"{"version": 8, "sources": {}, "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000"}}]}"##;
        let fitted = renderer.projection.zoom;
        assert_eq!(zoom(None), fitted.round() as u32);
        assert_eq!(zoom(Some("dark")), zoom(None));
        // MapLibre's 512 px tiles hold a level less detail at the same scale.
        assert_eq!(zoom(Some(maplibre)), (fitted - 1.0).floor() as u32);
    }
}
