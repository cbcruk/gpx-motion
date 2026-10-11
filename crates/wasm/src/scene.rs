use std::cell::RefCell;
use std::collections::HashMap;

use gpx_motion_core::{
    draw_fog, draw_labels, draw_profile, draw_terrain, draw_track_3d, draw_vector_map,
    draw_vector_map_3d, parse, reproject_ground, track_outline_3d, Camera, Dem, DepthMap,
    FrameStyle, GroundView, Keepout, LabelMemory, LabelSources, Lens, MapStyle, ProfileStyle,
    Relief, SceneView, Terrain, TerrainArea, TileId, TilePlacement, Timeline, TrackPoint,
    VectorTile, LABEL_LAYERS,
};
use tiny_skia::{Color, IntSize, Pixmap, Rect};
use wasm_bindgen::prelude::*;

use crate::{to_straight_rgba, LabelFont, Tile};

mod gl;
mod gl_labels;

pub use gl::GlTerrain;
pub use gl_labels::GlLabels;

/// Degrees between the bearings whose map is rendered exactly; frames between
/// them resample the nearest two.
const KEYFRAME_DEG: f64 = 10.0;

/// Keyframes in a full turn; keys wrap modulo this so the cache stays bounded.
const KEYFRAMES: i64 = (360.0 / KEYFRAME_DEG) as i64;

/// Extra border, as a fraction of the viewport, rendered around each keyframe:
/// turning the camera swings the frame corners outside the keyframe's view.
const KEYFRAME_OVERSCAN: f64 = 0.10;

/// Bearings within this fraction of a step from a keyframe use it alone.
const SNAP: f64 = 1e-6;

/// Largest terrain texture side in pixels (16 MB of RGBA).
const MAX_TERRAIN_TEXTURE: f64 = 2048.0;

/// A parsed GPX track rendered as a tilted 3D scene: the route raised by its
/// elevation over a perspective basemap, with an elevation profile panel.
///
/// The camera can turn up to `orbit` degrees from its start bearing, chosen
/// per frame independently of the track progress. Tile fetching stays on the
/// JS side: [`SceneRenderer::ground_tiles`] lists what the whole orbit will
/// see, and [`SceneRenderer::add_vector_tile`] hands the bytes back.
///
/// Keyframes can also be rendered off the main thread: a second renderer
/// built with the same arguments in a worker calls
/// [`SceneRenderer::render_keyframe`] for each key
/// [`SceneRenderer::missing_keyframes`] lists, and this one takes the result
/// through [`SceneRenderer::add_keyframe`].
#[wasm_bindgen]
pub struct SceneRenderer {
    timeline: Timeline,
    style: FrameStyle,
    profile: ProfileStyle,
    relief: Relief,
    camera: Camera,
    lens: Lens,
    points: Vec<TrackPoint>,
    heights: Vec<f64>,
    padding: f64,
    bottom_inset: f64,
    bearing_deg: f64,
    orbit_deg: f64,
    tail_u: f64,
    max_zoom: u32,
    map_style: MapStyle,
    tiles: HashMap<TileId, VectorTile>,
    ground: RefCell<Option<(f64, Pixmap)>>,
    /// Depth of the terrain in the last ground drawn, for hiding the track.
    ground_depth: RefCell<Option<DepthMap>>,
    keyframes: RefCell<HashMap<i64, GroundView>>,
    defer_keyframes: bool,
    generation: u32,
    labels: Option<LabelFont>,
    label_tiles: HashMap<TileId, VectorTile>,
    /// Label points per keyframe key, read once from its tiles.
    label_sources: RefCell<HashMap<i64, LabelSources>>,
    /// Labels of the last bearing drawn, which the next one keeps first.
    label_memory: RefCell<LabelMemory>,
    /// Label points and lines of every label tile, for the WebGL scene.
    gl_label_sources: RefCell<Option<LabelSources>>,
    /// Labels of the last WebGL frame, which fade over frames.
    gl_label_memory: RefCell<LabelMemory>,
    terrain_area: Option<TerrainArea>,
    dem: Option<Dem>,
    terrain: Option<Terrain>,
    terrain_visible: bool,
    /// Set once tiles were listed: the camera can't change after that.
    tiles_listed: bool,
}

#[wasm_bindgen]
impl SceneRenderer {
    /// Parse GPX text and frame the route for a `width`×`height` viewport seen
    /// at `pitch` degrees from straight down, facing `bearing` degrees, far
    /// enough back to stay in view anywhere along an `orbit`-degree turn.
    /// `pixelRatio` (default `1`) scales strokes, text, and map line widths
    /// for a canvas sized in device pixels.
    #[wasm_bindgen(constructor)]
    pub fn new(
        gpx: &str,
        width: u32,
        height: u32,
        pitch: f64,
        bearing: f64,
        orbit: f64,
        pixel_ratio: Option<f32>,
    ) -> Result<SceneRenderer, JsError> {
        let ratio = pixel_ratio
            .filter(|r| r.is_finite() && *r > 0.0)
            .unwrap_or(1.0);
        let points = parse(gpx).map_err(|e| JsError::new(&e.to_string()))?;
        let timeline = Timeline::build(&points)
            .ok_or_else(|| JsError::new("track needs two distinct points"))?;
        let style = FrameStyle {
            width,
            height,
            color_by_elevation: true,
            ..Default::default()
        }
        .scaled(ratio);
        let profile = ProfileStyle::default().scaled(ratio);
        let relief = Relief::auto(&timeline);
        let lens = Lens {
            pitch_deg: pitch,
            bearing_deg: bearing,
            ..Lens::default()
        };
        let heights = relief.heights_m(&timeline);
        let padding = style.padding;
        let bottom_inset = profile.reserved_height() as f64;
        let camera = Camera::fit(
            &points,
            &heights,
            width,
            height,
            padding,
            bottom_inset,
            lens,
        )
        .ok_or_else(|| JsError::new("could not fit track to viewport"))?
        .widen_for_orbit(&points, &heights, padding, bottom_inset, orbit);
        Ok(Self {
            timeline,
            style,
            profile,
            relief,
            camera,
            lens,
            points,
            heights,
            padding,
            bottom_inset,
            bearing_deg: bearing,
            orbit_deg: orbit,
            tail_u: 1.0,
            max_zoom: camera.tile_zoom(),
            map_style: MapStyle::dark().with_pixel_ratio(ratio),
            tiles: HashMap::new(),
            ground: RefCell::new(None),
            ground_depth: RefCell::new(None),
            keyframes: RefCell::new(HashMap::new()),
            defer_keyframes: false,
            generation: 0,
            labels: None,
            label_tiles: HashMap::new(),
            label_sources: RefCell::new(HashMap::new()),
            label_memory: RefCell::new(LabelMemory::new(1)),
            gl_label_sources: RefCell::new(None),
            gl_label_memory: RefCell::new(LabelMemory::new(gl_labels::FADE_FRAMES)),
            terrain_area: None,
            dem: None,
            terrain: None,
            terrain_visible: true,
            tiles_listed: false,
        })
    }

    /// Prepare for ground relief from DEM tiles: cap the track's height
    /// exaggeration as over terrain and reframe the camera for it. Call
    /// before [`SceneRenderer::ground_tiles`], on every renderer that must
    /// agree on keyframes (a keyframe worker's too). Returns `false`, changing
    /// nothing, when the route is too large for a terrain texture or tiles
    /// were already listed for the current camera.
    #[wasm_bindgen(js_name = enableTerrain)]
    pub fn enable_terrain(&mut self) -> bool {
        if self.tiles_listed {
            return false;
        }
        let relief = self.relief.over_terrain();
        let heights = relief.heights_m(&self.timeline);
        let Some(camera) = Camera::fit(
            &self.points,
            &heights,
            self.style.width,
            self.style.height,
            self.padding,
            self.bottom_inset,
            self.lens,
        ) else {
            return false;
        };
        let camera = camera.widen_for_orbit(
            &self.points,
            &heights,
            self.padding,
            self.bottom_inset,
            self.orbit_deg,
        );
        let Some(area) = TerrainArea::around(&self.points, camera.zoom, MAX_TERRAIN_TEXTURE)
            .filter(|a| a.fits(MAX_TERRAIN_TEXTURE))
        else {
            return false;
        };
        self.relief = relief;
        self.heights = heights;
        self.camera = camera;
        self.max_zoom = camera.tile_zoom();
        self.terrain_area = Some(area);
        self.dem = Some(Dem::new(area.dem_zoom));
        self.terrain = None;
        self.label_sources.borrow_mut().clear();
        self.gl_label_sources.replace(None);
        self.clear_ground();
        true
    }

    /// Terrarium DEM tiles (XYZ PNGs, e.g. the AWS Terrain Tiles) the terrain
    /// needs, after [`SceneRenderer::enable_terrain`]; empty before.
    #[wasm_bindgen(js_name = terrainTiles)]
    pub fn terrain_tiles(&self) -> Vec<Tile> {
        self.terrain_area.map_or_else(Vec::new, |area| {
            area.dem_tiles()
                .into_iter()
                .map(|id| {
                    Tile::from(TilePlacement {
                        id,
                        dx: 0.0,
                        dy: 0.0,
                        size: 256.0,
                    })
                })
                .collect()
        })
    }

    /// Add a decoded 256×256 Terrarium tile (RGBA bytes) listed by
    /// [`SceneRenderer::terrain_tiles`]. Returns whether it was used.
    #[wasm_bindgen(js_name = addTerrainTile)]
    pub fn add_terrain_tile(&mut self, tile: &Tile, rgba: &[u8]) -> bool {
        self.dem
            .as_mut()
            .is_some_and(|dem| dem.add_terrarium(TilePlacement::from(tile).id, rgba))
    }

    /// Vector tiles the terrain's map texture shows, at most `maxZoom` (the
    /// source's); empty before [`SceneRenderer::enable_terrain`]. Give them
    /// to whichever renderer calls [`SceneRenderer::render_terrain_texture`].
    #[wasm_bindgen(js_name = terrainMapTiles)]
    pub fn terrain_map_tiles(&self, max_zoom: u32) -> Vec<Tile> {
        self.terrain_area.map_or_else(Vec::new, |area| {
            area.projection()
                .visible_tiles_at(max_zoom, area.width, area.height)
                .into_iter()
                .map(Tile::from)
                .collect()
        })
    }

    /// Draw the map under the terrain from the added vector tiles, as
    /// premultiplied RGBA bytes for [`SceneRenderer::set_terrain_texture`]
    /// (on this renderer or a twin built with the same arguments).
    #[wasm_bindgen(js_name = renderTerrainTexture)]
    pub fn render_terrain_texture(&self, max_zoom: u32) -> Result<Vec<u8>, JsError> {
        let area = self
            .terrain_area
            .ok_or_else(|| JsError::new("terrain is not enabled"))?;
        let mut texture = Pixmap::new(area.width, area.height)
            .ok_or_else(|| JsError::new("invalid terrain size"))?;
        texture.fill(self.map_style.background);
        let tiles: Vec<(TilePlacement, &VectorTile)> = area
            .projection()
            .visible_tiles_at(max_zoom, area.width, area.height)
            .into_iter()
            .filter_map(|p| self.tiles.get(&p.id).map(|tile| (p, tile)))
            .collect();
        draw_vector_map(&mut texture, &tiles, &self.map_style, 1.0);
        Ok(texture.take())
    }

    /// Build the terrain from the DEM tiles added so far and a map texture
    /// from [`SceneRenderer::render_terrain_texture`]; from then on frames
    /// draw the relief with the track standing on it.
    #[wasm_bindgen(js_name = setTerrainTexture)]
    pub fn set_terrain_texture(&mut self, bytes: Vec<u8>) -> Result<(), JsError> {
        let (Some(area), Some(dem)) = (self.terrain_area, &self.dem) else {
            return Err(JsError::new("terrain is not enabled"));
        };
        let texture = IntSize::from_wh(area.width, area.height)
            .and_then(|size| Pixmap::from_vec(bytes, size))
            .ok_or_else(|| JsError::new("terrain texture size doesn't match"))?;
        self.terrain = Some(Terrain::build(
            dem,
            &self.relief,
            area.zoom,
            area.origin,
            texture,
        ));
        self.ground.replace(None);
        Ok(())
    }

    /// Show or hide the terrain (shown by default once built).
    #[wasm_bindgen(js_name = setTerrainVisible)]
    pub fn set_terrain_visible(&mut self, visible: bool) {
        self.terrain_visible = visible;
        self.ground.replace(None);
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

    /// Every vector tile the camera sees over the whole orbit, without
    /// duplicates. Pass the source's `maxZoom`; later frames use the same cap.
    #[wasm_bindgen(js_name = groundTiles)]
    pub fn ground_tiles(&mut self, max_zoom: u32) -> Vec<Tile> {
        self.max_zoom = max_zoom;
        self.tiles_listed = true;
        let mut seen = HashMap::new();
        for key in self.keyframe_keys() {
            for placement in self.keyframe_tiles(key) {
                seen.entry(placement.id).or_insert(placement);
            }
        }
        seen.into_values().map(Tile::from).collect()
    }

    /// Decode an MVT tile fetched for `tile` and keep it for later frames.
    #[wasm_bindgen(js_name = addVectorTile)]
    pub fn add_vector_tile(&mut self, tile: &Tile, bytes: &[u8]) -> Result<(), JsError> {
        let decoded = VectorTile::decode(bytes).map_err(|e| JsError::new(&e.to_string()))?;
        self.tiles.insert(TilePlacement::from(tile).id, decoded);
        self.clear_ground();
        Ok(())
    }

    /// Keep the named places, peaks, waters, roads, and rivers of an MVT
    /// tile fetched for `tile`, for labels ([`LABEL_LAYERS`]); the rest of
    /// the tile is skipped. Independent of
    /// [`SceneRenderer::add_vector_tile`], so labels work while another
    /// renderer (a keyframe worker) draws the map.
    #[wasm_bindgen(js_name = addLabelTile)]
    pub fn add_label_tile(&mut self, tile: &Tile, bytes: &[u8]) -> Result<(), JsError> {
        let decoded = VectorTile::decode_layers(bytes, &LABEL_LAYERS)
            .map_err(|e| JsError::new(&e.to_string()))?;
        self.label_tiles
            .insert(TilePlacement::from(tile).id, decoded);
        self.label_sources.borrow_mut().clear();
        self.gl_label_sources.replace(None);
        self.ground.replace(None);
        Ok(())
    }

    /// Draw map labels with this font (TrueType/OpenType bytes with glyphs
    /// for the map's script, e.g. Korean), from tiles given to
    /// [`SceneRenderer::add_label_tile`]. Labels are placed in screen space
    /// for each bearing, smaller and fainter with distance, and kept off the
    /// track's curtain and the profile panel. `size` is in CSS pixels
    /// (default 12); `language` prefers `name:<language>` names over the
    /// local `name`.
    #[wasm_bindgen(js_name = setLabels)]
    pub fn set_labels(
        &mut self,
        font: Vec<u8>,
        size: Option<f32>,
        language: Option<String>,
    ) -> Result<(), JsError> {
        self.labels = Some(LabelFont::new(font, size, language)?);
        self.label_sources.borrow_mut().clear();
        self.gl_label_sources.replace(None);
        self.ground.replace(None);
        Ok(())
    }

    /// Stop drawing map labels.
    #[wasm_bindgen(js_name = clearLabels)]
    pub fn clear_labels(&mut self) {
        self.labels = None;
        self.ground.replace(None);
    }

    /// Switch the basemap style: a preset name (`light`, `dark`) or a JSON
    /// style document. Drops a built terrain, whose texture has the old
    /// style; render and set a new one.
    #[wasm_bindgen(js_name = setMapStyle)]
    pub fn set_map_style(&mut self, style: &str) -> Result<(), JsError> {
        self.map_style = MapStyle::resolve(style)
            .map_err(|e| JsError::new(&e.to_string()))?
            .with_pixel_ratio(self.map_style.pixel_ratio);
        self.terrain = None;
        self.clear_ground();
        Ok(())
    }

    /// Fade the track to a tail of this fraction of progress (`0..=1`) instead
    /// of drawing everything so far (the default, `1`).
    #[wasm_bindgen(js_name = setTail)]
    pub fn set_tail(&mut self, fraction: f64) {
        self.tail_u = fraction.clamp(0.0, 1.0);
    }

    /// Override the track color used where elevation is missing (RGB, `0..=255`).
    #[wasm_bindgen(js_name = setTailColor)]
    pub fn set_tail_color(&mut self, r: u8, g: u8, b: u8) {
        self.style.tail_color = Color::from_rgba8(r, g, b, 0xff);
    }

    /// Render the full frame at track progress `u` with the camera turned
    /// `turn` of the way along its orbit (both `0..=1`), as straight RGBA
    /// bytes for `putImageData`.
    ///
    /// The basemap is drawn exactly only at keyframe bearings (every
    /// [`KEYFRAME_DEG`], each once); other bearings resample the two nearest
    /// keyframes, which is fast enough to turn the camera every frame. The
    /// first visit to a new keyframe still pays for a full map render.
    #[wasm_bindgen(js_name = renderFrame)]
    pub fn render_frame(&self, u: f64, turn: f64) -> Vec<u8> {
        let bearing = self.bearing_at(turn);
        let camera = self.camera_at(turn);
        let mut pixmap = self.ground(&camera, bearing);
        let depth = self.ground_depth.borrow();
        let view = SceneView {
            camera: &camera,
            relief: self.relief,
            terrain: self.shown_terrain(),
            occlusion: depth.as_ref(),
        };
        draw_track_3d(
            &mut pixmap,
            &self.timeline,
            &view,
            u,
            self.tail_u,
            &self.style,
        );
        draw_profile(&mut pixmap, &self.timeline, u, &self.profile, &self.style);
        to_straight_rgba(&pixmap)
    }

    /// When on, frames never render a keyframe themselves while another is
    /// cached: they resample the nearest cached keyframes instead, and show
    /// only the background before the first one arrives through
    /// [`SceneRenderer::add_keyframe`]. Off (the default) renders on demand.
    ///
    /// Switching discards the cached keyframes (bumping the generation): ones
    /// rendered here may predate tiles that only the worker received.
    #[wasm_bindgen(js_name = setDeferKeyframes)]
    pub fn set_defer_keyframes(&mut self, on: bool) {
        if self.defer_keyframes != on {
            self.defer_keyframes = on;
            self.clear_ground();
        }
    }

    /// Bumped whenever the tiles or style change, which discards every
    /// keyframe; a keyframe rendered before the bump no longer matches.
    #[wasm_bindgen(getter)]
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Keyframe keys the orbit needs that aren't cached yet, nearest to the
    /// bearing at `turn` (`0..=1`) first.
    #[wasm_bindgen(js_name = missingKeyframes)]
    pub fn missing_keyframes(&self, turn: f64) -> Vec<u32> {
        let bearing = self.bearing_at(turn);
        let cached = self.keyframes.borrow();
        let mut keys: Vec<i64> = self
            .keyframe_keys()
            .into_iter()
            .filter(|k| !cached.contains_key(k))
            .collect();
        keys.sort_by(|a, b| key_distance(*a, bearing).total_cmp(&key_distance(*b, bearing)));
        keys.into_iter().map(|k| k as u32).collect()
    }

    /// Render keyframe `key` (as listed by [`SceneRenderer::missing_keyframes`])
    /// and return its premultiplied RGBA bytes for
    /// [`SceneRenderer::add_keyframe`] on a renderer built with the same
    /// arguments, tiles, and style.
    #[wasm_bindgen(js_name = renderKeyframe)]
    pub fn render_keyframe(&self, key: u32) -> Result<Vec<u8>, JsError> {
        let key = keyframe_key(key)?;
        Ok(self.keyframe(key).pixmap.take())
    }

    /// Cache a keyframe from [`SceneRenderer::render_keyframe`], rendered when
    /// this renderer was at `generation`; a stale one is ignored. Returns
    /// whether it was kept.
    #[wasm_bindgen(js_name = addKeyframe)]
    pub fn add_keyframe(&self, key: u32, generation: u32, bytes: Vec<u8>) -> Result<bool, JsError> {
        let key = keyframe_key(key)?;
        if generation != self.generation {
            return Ok(false);
        }
        let side = 2 * self.keyframe_margin() as u32;
        let pixmap = IntSize::from_wh(self.style.width + side, self.style.height + side)
            .and_then(|size| Pixmap::from_vec(bytes, size))
            .ok_or_else(|| JsError::new("keyframe size doesn't match the viewport"))?;
        let camera = self.keyframe_camera(key);
        self.keyframes
            .borrow_mut()
            .insert(key, GroundView { camera, pixmap });
        self.ground.replace(None);
        Ok(true)
    }

    /// Labels from the tiles of the keyframe nearest `bearing` (the ones
    /// fetched for this part of the orbit), placed for `camera`. They depend
    /// only on the bearing, so they're cached with the ground.
    fn draw_labels(&self, pixmap: &mut Pixmap, camera: &Camera, bearing: f64) {
        let Some(labels) = &self.labels else {
            return;
        };
        let key = ((bearing / KEYFRAME_DEG).round() as i64).rem_euclid(KEYFRAMES);
        let ratio = self.map_style.pixel_ratio;
        let style = labels.style(self.map_style.background, ratio);
        let mut cache = self.label_sources.borrow_mut();
        let sources = cache.entry(key).or_insert_with(|| {
            let tiles: Vec<(TilePlacement, &VectorTile)> = self
                .keyframe_tiles(key)
                .into_iter()
                .filter_map(|p| self.label_tiles.get(&p.id).map(|tile| (p, tile)))
                .collect();
            LabelSources::collect(&tiles, &style)
        });
        let (width, height) = (self.style.width as f32, self.style.height as f32);
        let panel = self.bottom_inset as f32;
        let rects: Vec<Rect> = Rect::from_xywh(0.0, height - panel, width, panel)
            .into_iter()
            .collect();
        let terrain = self.shown_terrain();
        let (lines, areas) = track_outline_3d(camera, &self.points, self.relief, terrain);
        let keepout = Keepout::around(&rects, lines, areas, self.style.tail_width, ratio);
        let canvas = (self.style.width, self.style.height);
        let mut memory = self.label_memory.borrow_mut();
        let placed =
            sources.place_3d_steady(camera, &style, canvas, &keepout, terrain, &mut memory);
        draw_labels(pixmap, &placed, &style);
    }

    /// The terrain frames draw, when built and not hidden.
    fn shown_terrain(&self) -> Option<&Terrain> {
        self.terrain.as_ref().filter(|_| self.terrain_visible)
    }

    /// The terrain mesh over the flat ground, exact for this camera (the
    /// keyframes under it are flat and resample correctly).
    fn draw_relief(&self, pixmap: &mut Pixmap, camera: &Camera) {
        let depth = self
            .shown_terrain()
            .map(|terrain| draw_terrain(pixmap, camera, terrain));
        self.ground_depth.replace(depth);
    }

    fn bearing_at(&self, turn: f64) -> f64 {
        self.bearing_deg + self.orbit_deg * turn.clamp(0.0, 1.0)
    }

    /// Camera turned `turn` of the way along the orbit, re-aimed at the route.
    fn camera_at(&self, turn: f64) -> Camera {
        self.camera_facing(self.bearing_at(turn))
    }

    fn camera_facing(&self, bearing_deg: f64) -> Camera {
        self.camera.with_bearing(bearing_deg).aim(
            &self.points,
            &self.heights,
            self.padding,
            self.bottom_inset,
        )
    }

    /// The map and fog under `camera`, resampled from the keyframes either
    /// side of `bearing`.
    fn ground(&self, camera: &Camera, bearing: f64) -> Pixmap {
        if let Some((cached, pixmap)) = &*self.ground.borrow() {
            if *cached == bearing {
                return pixmap.clone();
            }
        }
        let steps = bearing / KEYFRAME_DEG;
        let below = steps.floor() as i64;
        let frac = steps - below as f64;
        let keys: Vec<i64> = if frac < SNAP {
            vec![below]
        } else if frac > 1.0 - SNAP {
            vec![below + 1]
        } else if frac < 0.5 {
            vec![below, below + 1]
        } else {
            vec![below + 1, below]
        };
        let mut keys: Vec<i64> = keys.into_iter().map(|k| k.rem_euclid(KEYFRAMES)).collect();
        if self.defer_keyframes {
            keys = self.nearest_cached(&keys, bearing);
        }
        for &key in &keys {
            if !self.keyframes.borrow().contains_key(&key) {
                let view = self.keyframe(key);
                self.keyframes.borrow_mut().insert(key, view);
            }
        }
        let keyframes = self.keyframes.borrow();
        if keys.is_empty() {
            let mut pixmap = Pixmap::new(self.style.width, self.style.height)
                .expect("viewport must be non-zero");
            pixmap.fill(self.map_style.background);
            self.draw_relief(&mut pixmap, camera);
            draw_fog(&mut pixmap, camera, self.map_style.background);
            self.draw_labels(&mut pixmap, camera, bearing);
            return pixmap;
        }
        let sources: Vec<&GroundView> = keys.iter().map(|k| &keyframes[k]).collect();
        let mut pixmap =
            Pixmap::new(self.style.width, self.style.height).expect("viewport must be non-zero");
        reproject_ground(&mut pixmap, camera, &sources, self.map_style.background);
        self.draw_relief(&mut pixmap, camera);
        draw_fog(&mut pixmap, camera, self.map_style.background);
        self.draw_labels(&mut pixmap, camera, bearing);
        self.ground.replace(Some((bearing, pixmap.clone())));
        pixmap
    }

    /// `wanted` if all are cached, else the (up to two) cached keyframes
    /// nearest `bearing`, which may leave the frame's corners uncovered.
    fn nearest_cached(&self, wanted: &[i64], bearing: f64) -> Vec<i64> {
        let cached = self.keyframes.borrow();
        if wanted.iter().all(|k| cached.contains_key(k)) {
            return wanted.to_vec();
        }
        let mut keys: Vec<i64> = cached.keys().copied().collect();
        keys.sort_by(|a, b| key_distance(*a, bearing).total_cmp(&key_distance(*b, bearing)));
        keys.truncate(2);
        keys
    }

    /// Keyframe keys (wrapped) whose bearings bracket the whole orbit.
    fn keyframe_keys(&self) -> Vec<i64> {
        let (a, b) = (self.bearing_at(0.0), self.bearing_at(1.0));
        let lo = (a.min(b) / KEYFRAME_DEG).floor() as i64;
        let hi = (a.max(b) / KEYFRAME_DEG).ceil() as i64;
        let mut keys: Vec<i64> = (lo..=hi).map(|k| k.rem_euclid(KEYFRAMES)).collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// Margin in pixels added on every side of a keyframe.
    fn keyframe_margin(&self) -> f64 {
        (KEYFRAME_OVERSCAN * self.style.width.max(self.style.height) as f64).ceil()
    }

    /// The overscanned camera a keyframe is rendered with. Tile prefetching
    /// uses the same cameras, so both agree on tile zoom and coverage.
    fn keyframe_camera(&self, key: i64) -> Camera {
        self.camera_facing(key as f64 * KEYFRAME_DEG)
            .overscanned(self.keyframe_margin())
    }

    /// Tiles a keyframe draws: its overscanned footprint at the zoom the
    /// unpadded camera picks, so the border doesn't coarsen the detail.
    fn keyframe_tiles(&self, key: i64) -> Vec<TilePlacement> {
        let plain = self.camera_facing(key as f64 * KEYFRAME_DEG);
        let zoom = plain
            .ground_tiles(self.max_zoom)
            .first()
            .map_or(self.max_zoom, |t| t.id.zoom);
        self.keyframe_camera(key).ground_tiles_at(zoom)
    }

    /// Render the map (no fog) exactly at keyframe bearing `key · KEYFRAME_DEG`,
    /// overscanned so frames turned up to half a step away stay covered.
    fn keyframe(&self, key: i64) -> GroundView {
        let camera = self.keyframe_camera(key);
        let side = 2 * self.keyframe_margin() as u32;
        let mut pixmap = Pixmap::new(self.style.width + side, self.style.height + side)
            .expect("viewport must be non-zero");
        pixmap.fill(self.map_style.background);
        let tiles: Vec<(TilePlacement, &VectorTile)> = self
            .keyframe_tiles(key)
            .into_iter()
            .filter_map(|p| self.tiles.get(&p.id).map(|tile| (p, tile)))
            .collect();
        draw_vector_map_3d(&mut pixmap, &tiles, &self.map_style, &camera, 1.0);
        GroundView { camera, pixmap }
    }

    fn clear_ground(&mut self) {
        self.ground.replace(None);
        self.keyframes.borrow_mut().clear();
        self.generation = self.generation.wrapping_add(1);
    }
}

fn keyframe_key(key: u32) -> Result<i64, JsError> {
    let key = i64::from(key);
    if (0..KEYFRAMES).contains(&key) {
        Ok(key)
    } else {
        Err(JsError::new("keyframe key out of range"))
    }
}

/// Degrees between keyframe `key`'s bearing and `bearing`, around the circle.
fn key_distance(key: i64, bearing: f64) -> f64 {
    let d = (key as f64 * KEYFRAME_DEG - bearing).rem_euclid(360.0);
    d.min(360.0 - d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fmt::Write;

    /// A straight ~40 km route north, long enough that wider views drop to a
    /// coarser tile zoom under the tile budget.
    fn long_route() -> String {
        let mut gpx = String::from(r#"<gpx version="1.1"><trk><trkseg>"#);
        for i in 0..200 {
            let lat = 35.80 + i as f64 * 0.0018;
            let lon = 129.20 + (i as f64 * 0.3).sin() * 0.01;
            let _ = write!(
                gpx,
                r#"<trkpt lat="{lat}" lon="{lon}"><ele>{}</ele></trkpt>"#,
                20.0 + (i % 40) as f64
            );
        }
        gpx + "</trkseg></trk></gpx>"
    }

    /// Below-horizon pixels no keyframe covers when turning through the
    /// whole orbit, using blank keyframes so no tiles are needed.
    fn uncovered_pixels(gpx: &str, pitch: f64) -> usize {
        const SIZE: u32 = 320;
        let Ok(scene) = SceneRenderer::new(gpx, SIZE, SIZE, pitch, 20.0, 360.0, None) else {
            panic!("scene should build");
        };
        let side = 2 * scene.keyframe_margin() as u32;
        let views: HashMap<i64, GroundView> = scene
            .keyframe_keys()
            .into_iter()
            .map(|key| {
                let mut pixmap = Pixmap::new(SIZE + side, SIZE + side).unwrap();
                pixmap.fill(Color::WHITE);
                let camera = scene.keyframe_camera(key);
                (key, GroundView { camera, pixmap })
            })
            .collect();
        let mut holes = 0;
        let turns = (0..36).flat_map(|k| [2.0, 5.0, 8.0].map(|d| (k as f64 * 10.0 + d) / 360.0));
        for turn in turns {
            let camera = scene.camera_at(turn);
            let steps = scene.bearing_at(turn) / KEYFRAME_DEG;
            let sources: Vec<&GroundView> = [steps.floor() as i64, steps.ceil() as i64]
                .iter()
                .map(|k| &views[&k.rem_euclid(KEYFRAMES)])
                .collect();
            let mut target = Pixmap::new(SIZE, SIZE).unwrap();
            reproject_ground(&mut target, &camera, &sources, Color::BLACK);
            let horizon = camera.horizon_y().max(0.0) as usize + 2;
            holes += target.pixels()[horizon * SIZE as usize..]
                .iter()
                .filter(|p| p.red() == 0)
                .count();
        }
        holes
    }

    #[test]
    fn keyframes_cover_every_frame_between_them() {
        let namsan = include_str!("../../../samples/namsan.gpx");
        for pitch in [30.0, 55.0, 70.0] {
            assert_eq!(
                uncovered_pixels(&long_route(), pitch),
                0,
                "long, pitch {pitch}"
            );
            assert_eq!(uncovered_pixels(namsan, pitch), 0, "namsan, pitch {pitch}");
        }
    }

    #[test]
    fn overscan_keeps_the_unpadded_tile_zoom() {
        let Ok(mut scene) = SceneRenderer::new(&long_route(), 640, 640, 55.0, 20.0, 360.0, None)
        else {
            panic!("scene should build");
        };
        scene.ground_tiles(14);
        for key in scene.keyframe_keys() {
            let plain = scene
                .camera_facing(key as f64 * KEYFRAME_DEG)
                .ground_tiles(14)[0]
                .id
                .zoom;
            assert!(scene.keyframe_tiles(key).iter().all(|t| t.id.zoom == plain));
        }
    }

    fn scene(gpx: &str) -> SceneRenderer {
        let Ok(scene) = SceneRenderer::new(gpx, 160, 120, 55.0, 20.0, 360.0, None) else {
            panic!("scene should build");
        };
        scene
    }

    #[test]
    fn keyframes_from_a_twin_match_rendering_them_here() {
        let gpx = include_str!("../../../samples/namsan.gpx");
        let local = scene(gpx);
        let worker = scene(gpx);
        let mut deferred = scene(gpx);
        deferred.set_defer_keyframes(true);
        for turn in [0.0, 0.013, 0.5, 0.987] {
            for key in deferred.missing_keyframes(turn).into_iter().take(2) {
                let generation = deferred.generation();
                let bytes = worker.render_keyframe(key).unwrap_or_default();
                assert!(matches!(
                    deferred.add_keyframe(key, generation, bytes),
                    Ok(true)
                ));
            }
            assert_eq!(
                deferred.render_frame(0.4, turn),
                local.render_frame(0.4, turn)
            );
        }
    }

    #[test]
    fn missing_keyframes_start_nearest_the_bearing() {
        let scene = scene(include_str!("../../../samples/namsan.gpx"));
        let keys = scene.missing_keyframes(0.0);
        assert_eq!(keys.len(), KEYFRAMES as usize);
        assert_eq!(keys[0], 2, "bearing 20° is keyframe 2");
        assert!(keys[1..3].contains(&1) && keys[1..3].contains(&3));
        let bytes = scene.render_keyframe(2).unwrap_or_default();
        assert!(matches!(
            scene.add_keyframe(2, scene.generation(), bytes),
            Ok(true)
        ));
        assert!(!scene.missing_keyframes(0.0).contains(&2));
    }

    #[test]
    fn deferred_frames_never_render_keyframes() {
        let mut scene = scene(include_str!("../../../samples/namsan.gpx"));
        scene.set_defer_keyframes(true);
        scene.render_frame(0.5, 0.25);
        assert!(scene.keyframes.borrow().is_empty());
        let bytes = scene.render_keyframe(2).unwrap_or_default();
        assert!(matches!(
            scene.add_keyframe(2, scene.generation(), bytes),
            Ok(true)
        ));
        scene.render_frame(0.5, 0.25);
        assert_eq!(
            scene.keyframes.borrow().len(),
            1,
            "falls back to the cached one"
        );
    }

    #[test]
    fn deferring_drops_keyframes_rendered_before() {
        let mut scene = scene(include_str!("../../../samples/namsan.gpx"));
        scene.render_frame(0.5, 0.0);
        assert!(!scene.missing_keyframes(0.0).contains(&2));
        let generation = scene.generation();
        scene.set_defer_keyframes(true);
        assert!(scene.missing_keyframes(0.0).contains(&2));
        assert_ne!(scene.generation(), generation);
    }

    #[test]
    fn labels_without_label_tiles_change_nothing() {
        let gpx = include_str!("../../../samples/namsan.gpx");
        let plain = scene(gpx);
        let mut labelled = scene(gpx);
        let font = include_bytes!("../../core/assets/font.ttf").to_vec();
        assert!(labelled.set_labels(font, Some(14.0), None).is_ok());
        for turn in [0.0, 0.3] {
            assert_eq!(
                labelled.render_frame(0.5, turn),
                plain.render_frame(0.5, turn)
            );
        }
    }

    /// A Terrarium tile rising 512–767 m from west to east (above the
    /// sample track).
    fn slope_tile() -> Vec<u8> {
        (0..256 * 256)
            .flat_map(|i| [130, (i % 256) as u8, 0, 255])
            .collect()
    }

    #[test]
    fn terrain_draws_relief_and_hides_on_request() {
        let gpx = include_str!("../../../samples/namsan.gpx");
        let mut flat = scene(gpx);
        assert!(flat.enable_terrain());
        let mut raised = scene(gpx);
        assert!(raised.enable_terrain());
        assert!(!raised.terrain_map_tiles(14).is_empty());
        let tiles = raised.terrain_tiles();
        assert!(!tiles.is_empty());
        for tile in &tiles {
            assert!(raised.add_terrain_tile(tile, &slope_tile()));
        }
        let texture = raised.render_terrain_texture(14).unwrap_or_default();
        assert!(raised.set_terrain_texture(texture).is_ok());
        assert_ne!(raised.render_frame(0.5, 0.2), flat.render_frame(0.5, 0.2));
        raised.set_terrain_visible(false);
        assert_eq!(raised.render_frame(0.5, 0.2), flat.render_frame(0.5, 0.2));
    }

    #[test]
    fn terrain_must_be_enabled_before_listing_tiles() {
        let mut scene = scene(include_str!("../../../samples/namsan.gpx"));
        let camera = scene.camera;
        scene.ground_tiles(12);
        assert!(!scene.enable_terrain());
        assert_eq!(scene.camera, camera);
        assert_eq!(scene.max_zoom, 12);
    }

    #[test]
    fn terrain_keyframes_still_match_a_twin() {
        let gpx = include_str!("../../../samples/namsan.gpx");
        let mut local = scene(gpx);
        let mut worker = scene(gpx);
        let mut deferred = scene(gpx);
        for s in [&mut local, &mut worker, &mut deferred] {
            assert!(s.enable_terrain());
            for tile in s.terrain_tiles() {
                s.add_terrain_tile(&tile, &slope_tile());
            }
        }
        let texture = worker.render_terrain_texture(14).unwrap_or_default();
        assert!(local.set_terrain_texture(texture.clone()).is_ok());
        assert!(deferred.set_terrain_texture(texture).is_ok());
        deferred.set_defer_keyframes(true);
        for key in deferred.missing_keyframes(0.3).into_iter().take(2) {
            let bytes = worker.render_keyframe(key).unwrap_or_default();
            let generation = deferred.generation();
            assert!(matches!(
                deferred.add_keyframe(key, generation, bytes),
                Ok(true)
            ));
        }
        assert_eq!(
            deferred.render_frame(0.5, 0.3),
            local.render_frame(0.5, 0.3)
        );
    }

    #[test]
    fn stale_keyframes_are_ignored() {
        let mut scene = scene(include_str!("../../../samples/namsan.gpx"));
        let generation = scene.generation();
        let bytes = scene.render_keyframe(2).unwrap_or_default();
        assert!(scene.set_map_style("light").is_ok());
        assert_ne!(scene.generation(), generation);
        assert!(matches!(
            scene.add_keyframe(2, generation, bytes),
            Ok(false)
        ));
        assert!(scene.keyframes.borrow().is_empty());
    }

    #[test]
    fn keyframes_only_need_prefetched_tiles() {
        for (gpx, orbit) in [(long_route(), 360.0), (long_route(), 0.0)] {
            let Ok(mut scene) = SceneRenderer::new(&gpx, 640, 640, 55.0, 20.0, orbit, None) else {
                panic!("scene should build");
            };
            let prefetched: HashSet<(u32, u32, u32)> = scene
                .ground_tiles(14)
                .iter()
                .map(|t| (t.zoom, t.x, t.y))
                .collect();
            for turn in (0..=720).map(|i| i as f64 / 720.0) {
                let steps = scene.bearing_at(turn) / KEYFRAME_DEG;
                for key in [steps.floor() as i64, steps.ceil() as i64] {
                    let key = key.rem_euclid(KEYFRAMES);
                    for p in scene.keyframe_tiles(key) {
                        let id = (p.id.zoom, p.id.x, p.id.y);
                        assert!(prefetched.contains(&id), "key {key} needs {id:?}");
                    }
                }
            }
        }
    }
}
