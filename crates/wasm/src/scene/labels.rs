//! The scene's map labels: the font, the label tiles the page fetches (around
//! the orbit, and along the route for a following camera), the label
//! sources gathered from them, and the fade memory of the labels shown.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gpx_motion_core::mercator::{lat_to_pixel, lon_to_pixel};
use gpx_motion_core::{
    LabelMemory, LabelSources, LabelStyle, TileId, TilePlacement, TrackPoint, VectorTile,
    LABEL_LAYERS,
};
use tiny_skia::Rect;
use wasm_bindgen::prelude::*;

use super::SceneRenderer;
use crate::{js_error, LabelFont, Tile};

/// Views a label takes to fade in or out (about 0.4 s at 60 frames a second).
const FADE_FRAMES: u32 = 24;
/// Degrees between the bearings sampled along the orbit when listing label
/// tiles.
const LABEL_STEP_DEG: f64 = 5.0;
/// Most label tiles listed along the route for a following camera; long
/// routes get coarser tiles to stay within it.
const MAX_FOLLOW_LABEL_TILES: usize = 128;

/// Label tiles of one kind and the label sources gathered from them, which
/// are gathered on first use and again after the tiles change.
#[derive(Default)]
struct LabelTiles {
    tiles: HashMap<TileId, VectorTile>,
    sources: RefCell<Option<LabelSources>>,
}

impl LabelTiles {
    /// Keep `tile`, so the sources are gathered again.
    fn insert(&mut self, id: TileId, tile: VectorTile) {
        self.tiles.insert(id, tile);
        self.forget_sources();
    }

    fn forget_sources(&mut self) {
        *self.sources.get_mut() = None;
    }
}

/// Everything the scene keeps for its map labels.
///
/// The label sources are gathered from the tiles lazily, while placing
/// labels for a frame, and dropped whenever what they were gathered from
/// changes: adding a tile drops those of its kind, and
/// [`LabelState::invalidate`] (a new font or map style) drops all of them
/// along with the fade memory.
pub(super) struct LabelState {
    font: Option<LabelFont>,
    /// Screen rectangles labels keep off (e.g. HTML over the canvas).
    keepout: Vec<Rect>,
    /// Tiles the framing camera sees along the orbit, or the terrain area's.
    orbit: LabelTiles,
    /// Tiles along the route, for a camera following the marker.
    along: LabelTiles,
    /// Labels of the last frame, which fade over frames.
    memory: RefCell<LabelMemory>,
}

impl LabelState {
    /// No font, tiles, or labels shown yet.
    pub fn new() -> Self {
        Self {
            font: None,
            keepout: Vec::new(),
            orbit: LabelTiles::default(),
            along: LabelTiles::default(),
            memory: RefCell::new(LabelMemory::new(FADE_FRAMES)),
        }
    }

    /// The font labels are drawn in, `None` while labels are off.
    pub fn font(&self) -> Option<&LabelFont> {
        self.font.as_ref()
    }

    /// Draw labels in `font`, or stop drawing them with `None`; either way
    /// the next labels are placed afresh.
    pub fn set_font(&mut self, font: Option<LabelFont>) {
        self.font = font;
        self.invalidate();
    }

    /// Screen rectangles labels keep off.
    pub fn keepout(&self) -> &[Rect] {
        &self.keepout
    }

    /// Keep labels off `rects` from now on, replacing the previous ones.
    pub fn set_keepout(&mut self, rects: Vec<Rect>) {
        self.keepout = rects;
    }

    /// Keep label tile `id` seen along the orbit (or in the terrain area).
    pub fn add_tile(&mut self, id: TileId, tile: VectorTile) {
        self.orbit.insert(id, tile);
    }

    /// Keep label tile `id` along the route, for a following camera.
    pub fn add_follow_tile(&mut self, id: TileId, tile: VectorTile) {
        self.along.insert(id, tile);
    }

    /// Drop the label sources gathered from every tile and the fade memory,
    /// so the next labels are gathered and placed afresh (all at once, with
    /// no fade): for when the font or the map style changes what the tiles
    /// give.
    pub fn invalidate(&mut self) {
        self.orbit.forget_sources();
        self.along.forget_sources();
        *self.memory.get_mut() = LabelMemory::new(FADE_FRAMES);
    }

    /// `place` with the label sources for this frame, gathered for `style`
    /// at world zoom `zoom` if not yet, and the fade memory.
    ///
    /// The sources are those of the tiles along the route while `following`
    /// once any are in, else those of the orbit.
    pub fn with_sources<R>(
        &self,
        following: bool,
        zoom: f64,
        style: &LabelStyle,
        place: impl FnOnce(&LabelSources, &mut LabelMemory) -> R,
    ) -> R {
        let kind = if following && !self.along.tiles.is_empty() {
            &self.along
        } else {
            &self.orbit
        };
        let mut cache = kind.sources.borrow_mut();
        let sources = cache.get_or_insert_with(|| {
            let tiles: Vec<(TilePlacement, &VectorTile)> = kind
                .tiles
                .iter()
                .map(|(id, tile)| (world_placement(*id, zoom), tile))
                .collect();
            LabelSources::collect(&tiles, style)
        });
        place(sources, &mut self.memory.borrow_mut())
    }
}

#[wasm_bindgen]
impl SceneRenderer {
    /// Vector tiles (at most `maxZoom`, the source's) whose names the labels
    /// show: those of the terrain area for a scene built with terrain, else
    /// those the camera sees along the orbit. Give them to
    /// [`SceneRenderer::add_label_tile`].
    #[wasm_bindgen(js_name = labelTiles)]
    pub fn label_tiles(&self, max_zoom: u32) -> Vec<Tile> {
        if let Some(area) = self.terrain.area() {
            return area
                .projection()
                .visible_tiles_at(max_zoom, area.width, area.height)
                .into_iter()
                .map(Tile::from)
                .collect();
        }
        let steps = (self.view.orbit_deg().abs() / LABEL_STEP_DEG)
            .ceil()
            .max(1.0) as usize;
        let mut seen = HashSet::new();
        let orbit = (0..=steps).flat_map(|k| {
            let turn = k as f64 / steps as f64;
            self.view
                .camera_at(&self.route, turn)
                .ground_tiles(max_zoom)
        });
        orbit
            .filter(|p| seen.insert(p.id))
            .map(Tile::from)
            .collect()
    }

    /// Keep the named places, peaks, waters, roads, and rivers of an MVT
    /// tile fetched for `tile`, for labels ([`LABEL_LAYERS`]); the rest of
    /// the tile is skipped.
    ///
    /// # Errors
    ///
    /// Throws when `bytes` isn't a valid MVT tile.
    #[wasm_bindgen(js_name = addLabelTile)]
    pub fn add_label_tile(&mut self, tile: &Tile, bytes: &[u8]) -> Result<(), JsError> {
        let decoded = VectorTile::decode_layers(bytes, &LABEL_LAYERS).map_err(js_error)?;
        self.labels.add_tile(TilePlacement::from(tile).id, decoded);
        Ok(())
    }

    /// Vector tiles along the route (at most `maxZoom`, coarser for long
    /// routes) whose names the labels show while following the marker, in
    /// place of [`SceneRenderer::label_tiles`]. Give them to
    /// [`SceneRenderer::add_follow_label_tile`].
    #[wasm_bindgen(js_name = followLabelTiles)]
    pub fn follow_label_tiles(&self, max_zoom: u32) -> Vec<Tile> {
        let mut zoom = max_zoom;
        loop {
            let tiles = route_tiles(&self.route.points, zoom);
            if tiles.len() <= MAX_FOLLOW_LABEL_TILES || zoom == 0 {
                return tiles.into_iter().map(Tile::from).collect();
            }
            zoom -= 1;
        }
    }

    /// Keep the label layers of an MVT tile fetched for `tile` from
    /// [`SceneRenderer::follow_label_tiles`].
    ///
    /// # Errors
    ///
    /// Throws when `bytes` isn't a valid MVT tile.
    #[wasm_bindgen(js_name = addFollowLabelTile)]
    pub fn add_follow_label_tile(&mut self, tile: &Tile, bytes: &[u8]) -> Result<(), JsError> {
        let decoded = VectorTile::decode_layers(bytes, &LABEL_LAYERS).map_err(js_error)?;
        self.labels
            .add_follow_tile(TilePlacement::from(tile).id, decoded);
        Ok(())
    }

    /// Show map labels in this font (TrueType/OpenType bytes with glyphs for
    /// the map's script, e.g. Korean), from tiles given to
    /// [`SceneRenderer::add_label_tile`]; see `glLabels`. `size` is in CSS
    /// pixels (default 12); `language` prefers `name:<language>` names over
    /// the local `name`.
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
        let font = LabelFont::new(font, size, language)?;
        self.labels.set_font(Some(font));
        Ok(())
    }

    /// Keep map labels off these rectangles (e.g. an attribution laid over
    /// the canvas): `x`, `y`, `width`, `height` each, in scene pixels.
    /// Replaces the previous ones; empty clears them.
    #[wasm_bindgen(js_name = setLabelKeepout)]
    pub fn set_label_keepout(&mut self, rects: &[f32]) {
        self.labels.set_keepout(crate::keepout_rects(rects));
    }

    /// Stop showing map labels.
    #[wasm_bindgen(js_name = clearLabels)]
    pub fn clear_labels(&mut self) {
        self.labels.set_font(None);
    }
}

/// Where tile `id` lies in world pixels at `zoom`, as 3D label placement
/// expects.
fn world_placement(id: TileId, zoom: f64) -> TilePlacement {
    let size = 256.0 * 2f64.powf(zoom - f64::from(id.zoom));
    TilePlacement {
        id,
        dx: f64::from(id.x) * size,
        dy: f64::from(id.y) * size,
        size,
    }
}

/// Tiles at `zoom` under the route's points and around them, for labels
/// along it.
fn route_tiles(points: &[TrackPoint], zoom: u32) -> Vec<TilePlacement> {
    let n = 1i64 << zoom;
    let mut seen = HashSet::new();
    for p in points {
        let x = (lon_to_pixel(p.lon, f64::from(zoom)) / 256.0).floor() as i64;
        let y = (lat_to_pixel(p.lat, f64::from(zoom)) / 256.0).floor() as i64;
        for (dx, dy) in (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| (dx, dy))) {
            let (tx, ty) = (x + dx, y + dy);
            if (0..n).contains(&tx) && (0..n).contains(&ty) {
                seen.insert((tx as u32, ty as u32));
            }
        }
    }
    let mut tiles: Vec<_> = seen.into_iter().collect();
    tiles.sort_unstable();
    tiles
        .into_iter()
        .map(|(x, y)| TilePlacement::whole(TileId { zoom, x, y }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{long, namsan};
    use super::*;
    use tiny_skia::Color;

    const FONT: &[u8] = include_bytes!("../../../core/assets/font.ttf");

    fn style() -> LabelStyle {
        let Ok(font) = LabelFont::new(FONT.to_vec(), None, None) else {
            panic!("font should load");
        };
        font.style(Color::BLACK, 1.0)
    }

    fn id(x: u32) -> TileId {
        TileId { zoom: 12, x, y: 0 }
    }

    /// Whether `tiles` has label sources gathered.
    fn gathered(tiles: &LabelTiles) -> bool {
        tiles.sources.borrow().is_some()
    }

    /// Gather the sources the camera uses, `following` or not.
    fn gather(state: &LabelState, following: bool) {
        state.with_sources(following, 12.0, &style(), |_, _| ());
    }

    /// Whether the fade memory is as new.
    fn fresh_memory(state: &LabelState) -> bool {
        format!("{:?}", state.memory.borrow()) == format!("{:?}", LabelMemory::new(FADE_FRAMES))
    }

    #[test]
    fn invalidating_drops_every_source_and_the_fade_memory() {
        let mut state = LabelState::new();
        state.add_follow_tile(id(0), VectorTile::default());
        gather(&state, false);
        gather(&state, true);
        assert!(gathered(&state.orbit) && gathered(&state.along));
        *state.memory.borrow_mut() = LabelMemory::new(1);
        assert!(!fresh_memory(&state));
        state.invalidate();
        assert!(!gathered(&state.orbit) && !gathered(&state.along));
        assert!(fresh_memory(&state));
    }

    #[test]
    fn fonts_and_their_removal_invalidate() {
        let mut state = LabelState::new();
        gather(&state, false);
        *state.memory.borrow_mut() = LabelMemory::new(1);
        state.set_font(LabelFont::new(FONT.to_vec(), None, None).ok());
        assert!(state.font().is_some());
        assert!(!gathered(&state.orbit) && fresh_memory(&state));
        gather(&state, false);
        *state.memory.borrow_mut() = LabelMemory::new(1);
        state.set_font(None);
        assert!(state.font().is_none());
        assert!(!gathered(&state.orbit) && fresh_memory(&state));
    }

    #[test]
    fn a_new_tile_drops_only_its_kinds_sources_and_keeps_the_fades() {
        let mut state = LabelState::new();
        state.add_follow_tile(id(0), VectorTile::default());
        gather(&state, false);
        gather(&state, true);
        *state.memory.borrow_mut() = LabelMemory::new(1);
        state.add_tile(id(1), VectorTile::default());
        assert!(!gathered(&state.orbit) && gathered(&state.along));
        state.add_follow_tile(id(2), VectorTile::default());
        assert!(!gathered(&state.along));
        assert!(!fresh_memory(&state), "fades carry on as tiles come in");
    }

    #[test]
    fn following_uses_the_route_tiles_once_any_are_in() {
        let mut state = LabelState::new();
        gather(&state, true);
        assert!(gathered(&state.orbit) && !gathered(&state.along));
        state.add_follow_tile(id(0), VectorTile::default());
        gather(&state, true);
        assert!(gathered(&state.along));
    }

    #[test]
    fn switching_the_map_style_places_labels_afresh() {
        let mut scene = namsan(false);
        gather(&scene.labels, false);
        *scene.labels.memory.borrow_mut() = LabelMemory::new(1);
        assert!(scene.set_map_style("light").is_ok());
        assert!(!gathered(&scene.labels.orbit) && fresh_memory(&scene.labels));
    }

    #[test]
    fn label_tiles_cover_the_terrain_area_or_the_orbit() {
        let terrain = namsan(true);
        let area = terrain.label_tiles(14);
        assert!(!area.is_empty() && area.iter().all(|t| t.zoom <= 14));
        let long = long();
        let orbit = long.label_tiles(14);
        let ids: HashSet<(u32, u32, u32)> = orbit.iter().map(|t| (t.zoom, t.x, t.y)).collect();
        assert_eq!(ids.len(), orbit.len(), "no duplicates");
        // Every tile the camera shows between the sampled bearings is listed.
        for turn in (0..=720).map(|i| f64::from(i) / 720.0) {
            for p in long.view.camera_at(&long.route, turn).ground_tiles(14) {
                assert!(ids.contains(&(p.id.zoom, p.id.x, p.id.y)), "turn {turn}");
            }
        }
    }

    #[test]
    fn follow_label_tiles_stay_within_their_cap() {
        let long = long();
        assert_eq!(long.follow_label_tiles(14)[0].zoom, 14);
        // Too many at 18 for a 40 km route: coarser ones instead.
        let tiles = long.follow_label_tiles(18);
        assert!(!tiles.is_empty() && tiles.len() <= MAX_FOLLOW_LABEL_TILES);
        let zoom = tiles[0].zoom;
        assert!(zoom < 18, "{zoom}");
        assert!(tiles.iter().all(|t| t.zoom == zoom));
    }

    #[test]
    fn world_placement_matches_the_mercator_grid() {
        let id = TileId {
            zoom: 12,
            x: 3500,
            y: 1600,
        };
        let p = world_placement(id, 13.5);
        assert!((p.size - 256.0 * 2f64.powf(1.5)).abs() < 1e-9);
        assert!((p.dx - 3500.0 * p.size).abs() < 1e-6);
        assert!((p.dy - 1600.0 * p.size).abs() < 1e-6);
    }

    #[test]
    fn route_tiles_cover_the_route_and_its_neighbours() {
        let points = [
            TrackPoint::new(37.55, 126.98),
            TrackPoint::new(37.56, 127.0),
        ];
        let tiles = route_tiles(&points, 14);
        for p in &points {
            let x = (lon_to_pixel(p.lon, 14.0) / 256.0) as u32;
            let y = (lat_to_pixel(p.lat, 14.0) / 256.0) as u32;
            for (dx, dy) in [(0, 0), (1, 1), (0, 1)] {
                assert!(tiles.iter().any(|t| (t.id.x, t.id.y) == (x + dx, y + dy)));
                assert!(tiles.iter().any(|t| (t.id.x, t.id.y) == (x - dx, y - dy)));
            }
        }
        assert!(tiles.iter().all(|t| t.id.zoom == 14));
    }
}
