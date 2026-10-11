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

pub use scene::{GlLabels, GlTerrain, SceneRenderer};

use gpx_motion_core::geo::mercator::Projection;
use gpx_motion_core::tiles::pmtiles::{Archive, Step, TileType, PREFIX_LEN};
use gpx_motion_core::{
    draw_labels, draw_overlay, draw_vector_map, parse, place_labels_flat, FrameStyle, Keepout,
    LabelStyle, MapStyle, TextFont, TileId, TilePlacement, Timeline, TrackPoint, VectorTile,
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
                .unwrap_or(DEFAULT_LABEL_SIZE)
                .clamp(4.0, 96.0),
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

/// Label text size in CSS pixels when none is given.
const DEFAULT_LABEL_SIZE: f32 = 12.0;

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
}

#[wasm_bindgen]
impl GpxRenderer {
    /// Parse GPX text and fit the whole track into a `width`×`height` viewport
    /// of device pixels. `pixelRatio` (device pixels per CSS pixel, default
    /// `1`) scales the track, marker, padding, and map line widths, e.g.
    /// `devicePixelRatio` for a canvas sized in device pixels.
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
        let points = parse(gpx).map_err(|e| JsError::new(&e.to_string()))?;
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
    /// viewport zoom is higher; omit it for tiles at the nearest whole zoom,
    /// each `256·2^(zoom − tileZoom)` pixels across.
    #[wasm_bindgen(js_name = visibleTiles)]
    pub fn visible_tiles(&self, max_zoom: Option<u32>) -> Vec<Tile> {
        let zoom = max_zoom.unwrap_or(self.projection.tile_zoom());
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

    /// Render the added vector tiles as a basemap, returning straight RGBA
    /// bytes for `putImageData`. `style` is a preset name (`light`, `dark`)
    /// or a JSON style document. Place, peak, and water names are drawn too
    /// after [`GpxRenderer::set_labels`], kept off the route.
    #[wasm_bindgen(js_name = renderMap)]
    pub fn render_map(&self, style: &str) -> Result<Vec<u8>, JsError> {
        let style = MapStyle::resolve(style)
            .map_err(|e| JsError::new(&e.to_string()))?
            .with_pixel_ratio(self.pixel_ratio);
        let tiles: Vec<(TilePlacement, &VectorTile)> =
            self.vector_tiles.iter().map(|(p, t)| (*p, t)).collect();
        let mut pixmap = self.blank();
        draw_vector_map(&mut pixmap, &tiles, &style, 1.0);
        if let Some(labels) = &self.labels {
            let label_style = labels.style(style.background, self.pixel_ratio);
            let route = self
                .points
                .iter()
                .map(|p| {
                    let (x, y) = self.projection.project(p.lat, p.lon);
                    (x as f32, y as f32)
                })
                .collect();
            let keepout = Keepout::around(
                &[],
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
    #[wasm_bindgen(js_name = rootRange)]
    pub fn root_range(header: &[u8]) -> Result<Vec<f64>, JsError> {
        let (offset, length) = Archive::root_range(header).map_err(js_error)?;
        Ok(vec![offset as f64, length as f64])
    }

    /// Open an archive of MVT tiles from its header bytes and its
    /// compressed root directory.
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
        self.archive.header().max_zoom as u32
    }

    /// The next step for tile `z/x/y`, as `[kind, offset, length, key]`:
    /// kind `0` means the archive lacks the tile; `1`, fetch the byte range
    /// and pass it to [`PmTiles::tile`]; `2`, fetch the leaf directory at the
    /// range, pass it to [`PmTiles::add_leaf`] with `key`, and ask again.
    pub fn locate(&self, z: u32, x: u32, y: u32) -> Result<Vec<f64>, JsError> {
        Ok(match self.archive.locate(z, x, y).map_err(js_error)? {
            Step::Missing => vec![0.0, 0.0, 0.0, 0.0],
            Step::Tile { offset, length } => vec![1.0, offset as f64, length as f64, 0.0],
            Step::Leaf {
                offset,
                length,
                key,
            } => vec![2.0, offset as f64, length as f64, key as f64],
        })
    }

    /// Keep a leaf directory that [`PmTiles::locate`] asked for.
    #[wasm_bindgen(js_name = addLeaf)]
    pub fn add_leaf(&mut self, key: f64, bytes: &[u8]) -> Result<(), JsError> {
        self.archive.add_leaf(key as u64, bytes).map_err(js_error)
    }

    /// The decompressed MVT bytes from a tile's fetched range.
    pub fn tile(&self, bytes: &[u8]) -> Result<Vec<u8>, JsError> {
        self.archive.tile(bytes).map_err(js_error)
    }
}

/// What a map style (a preset name or JSON) leaves out when used with
/// `renderMap` or `setMapStyle`: for an imported MapLibre style, the layers
/// and properties it skips or approximates; empty for gpx-motion styles.
#[wasm_bindgen(js_name = mapStyleNotes)]
pub fn map_style_notes(style: &str) -> Result<Vec<String>, JsError> {
    MapStyle::resolve_with_notes(style)
        .map(|(_, notes)| notes)
        .map_err(js_error)
}

fn js_error(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
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
