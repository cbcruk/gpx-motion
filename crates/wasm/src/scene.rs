use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gpx_motion_core::geo::mercator::{lat_to_pixel, lon_to_pixel};
use gpx_motion_core::{
    parse, Camera, Dem, FrameStyle, LabelMemory, LabelSources, Lens, MapStyle, ProfileStyle,
    Relief, Terrain, TerrainArea, TileId, TilePlacement, Timeline, TrackPoint, VectorTile,
    LABEL_LAYERS,
};
use tiny_skia::{Color, Rect};
use wasm_bindgen::prelude::*;

use crate::{LabelFont, Tile};

mod gl;
mod gl_labels;

pub use gl_labels::GlLabels;

/// Largest terrain area side in world pixels.
const MAX_TERRAIN_SIDE: f64 = 2048.0;

/// Degrees between the bearings sampled along the orbit when listing label
/// tiles.
const LABEL_STEP_DEG: f64 = 5.0;

/// Zoom levels the framing camera can come in from the route's fit, and
/// back off from it.
const FRAMING_ZOOM: (f64, f64) = (-1.0, 4.0);
/// Steepest tilt from straight down, in degrees (as [`Lens::pitch_deg`]).
const MAX_PITCH_DEG: f64 = 75.0;

/// Finest zoom a camera following the marker comes in to.
const MAX_FOLLOW_ZOOM: f64 = 18.0;
/// Height a following camera keeps above the ground under it, as a share
/// of its distance to the marker.
const EYE_CLEARANCE: f64 = 0.2;
/// Most track points a refit for a new tilt looks at.
const MAX_FIT_POINTS: usize = 256;

/// Most times a following camera backs off to clear the ground under it.
const MAX_CLEARANCE_STEPS: usize = 8;
/// Most label tiles listed along the route for a following camera; long
/// routes get coarser tiles to stay within it.
const MAX_FOLLOW_LABEL_TILES: usize = 128;

/// A parsed GPX track as a 3D scene for WebGL: the route raised by its
/// elevation over the ground, framed for an orbiting camera, with an
/// elevation profile panel.
///
/// The camera can turn up to `orbit` degrees from its start bearing, chosen
/// per frame independently of the track progress. The scene hands its
/// contents to the page as data (`glTrack`, `glCamera`, `glGround`, …) and
/// fetching stays on the JS side: the scene lists the tiles it needs
/// ([`SceneRenderer::terrain_tiles`], [`SceneRenderer::label_tiles`]) and
/// takes back their bytes.
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
    map_style: MapStyle,
    labels: Option<LabelFont>,
    label_tiles: HashMap<TileId, VectorTile>,
    /// Label points and lines of every label tile.
    gl_label_sources: RefCell<Option<LabelSources>>,
    /// Label tiles along the route, for a camera following the marker.
    follow_label_tiles: HashMap<TileId, VectorTile>,
    /// Label points and lines of every tile in `follow_label_tiles`.
    gl_follow_label_sources: RefCell<Option<LabelSources>>,
    /// Labels of the last frame, which fade over frames.
    gl_label_memory: RefCell<LabelMemory>,
    /// Screen rectangles labels keep off (e.g. HTML over the canvas).
    label_keepout: Vec<Rect>,
    terrain_area: Option<TerrainArea>,
    dem: Option<Dem>,
    terrain: Option<Terrain>,
    terrain_visible: bool,
    /// Set once tiles were listed: the camera can't change after that.
    tiles_listed: bool,
    /// Zoom at which the camera follows the marker, instead of framing the
    /// whole route.
    follow: Option<f64>,
    /// Zoom levels the framing camera comes in (or, below 0, backs off) from
    /// the distance that fits the route.
    framing_zoom: f64,
    /// Where the framing camera first aimed: the GL origin without terrain,
    /// fixed so positions already handed out stay valid.
    start_target: (f64, f64),
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
            map_style: MapStyle::dark().with_pixel_ratio(ratio),
            labels: None,
            label_tiles: HashMap::new(),
            gl_label_sources: RefCell::new(None),
            follow_label_tiles: HashMap::new(),
            gl_follow_label_sources: RefCell::new(None),
            gl_label_memory: RefCell::new(LabelMemory::new(gl_labels::FADE_FRAMES)),
            label_keepout: Vec::new(),
            terrain_area: None,
            dem: None,
            terrain: None,
            terrain_visible: true,
            tiles_listed: false,
            follow: None,
            framing_zoom: 0.0,
            start_target: (0.0, 0.0),
        }
        .with_start_target())
    }

    /// Prepare for ground relief from DEM tiles: cap the track's height
    /// exaggeration as over terrain and reframe the camera for it. Call
    /// before listing tiles. Returns `false`, changing nothing, when the
    /// route is too large for a terrain area or tiles were already listed
    /// for the current camera.
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
        let Some(area) = TerrainArea::around(&self.points, camera.zoom, MAX_TERRAIN_SIDE)
            .filter(|a| a.fits(MAX_TERRAIN_SIDE))
        else {
            return false;
        };
        self.relief = relief;
        self.heights = heights;
        self.camera = camera;
        self.start_target = self.camera_at(0.0).target();
        self.terrain_area = Some(area);
        self.dem = Some(Dem::new(area.dem_zoom));
        self.terrain = None;
        self.gl_label_sources.replace(None);
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

    /// Show or hide the terrain (shown by default once built).
    #[wasm_bindgen(js_name = setTerrainVisible)]
    pub fn set_terrain_visible(&mut self, visible: bool) {
        self.terrain_visible = visible;
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

    /// Follow the marker at world zoom `zoom` (clamped between the route's
    /// framing zoom and 18), or frame the whole route again with `None`.
    /// The orbit's bearing still applies.
    #[wasm_bindgen(js_name = setFollow)]
    pub fn set_follow(&mut self, zoom: Option<f64>) {
        self.follow = zoom
            .filter(|z| z.is_finite())
            .map(|z| z.clamp(self.camera.zoom, MAX_FOLLOW_ZOOM.max(self.camera.zoom)));
    }

    /// Tilt the camera `pitch` degrees from straight down (clamped to
    /// `0..=75`), refitting the framing distance for it at the same world
    /// zoom, so terrain and tiles already listed still fit.
    #[wasm_bindgen(js_name = setPitch)]
    pub fn set_pitch(&mut self, pitch: f64) {
        if !pitch.is_finite() {
            return;
        }
        self.lens.pitch_deg = pitch.clamp(0.0, MAX_PITCH_DEG);
        let mut camera = self
            .camera
            .with_pitch(self.lens.pitch_deg)
            .with_bearing(self.bearing_deg);
        camera.distance = 0.0;
        // Long tracks fit by a sample of their points (and their extremes),
        // so tilting stays quick.
        let (points, heights) = fit_sample(&self.points, &self.heights);
        self.camera = camera.widen_for_orbit(
            &points,
            &heights,
            self.padding,
            self.bottom_inset,
            self.orbit_deg,
        );
    }

    /// The camera's tilt from straight down, in degrees.
    #[wasm_bindgen(getter, js_name = pitchDeg)]
    pub fn pitch_deg(&self) -> f64 {
        self.lens.pitch_deg
    }

    /// Bring the framing camera `levels` zoom levels closer to the route's
    /// middle (negative backs off), clamped to `-1..=4`; `0` fits the route.
    #[wasm_bindgen(js_name = setFramingZoom)]
    pub fn set_framing_zoom(&mut self, levels: f64) {
        if levels.is_finite() {
            self.framing_zoom = levels.clamp(FRAMING_ZOOM.0, FRAMING_ZOOM.1);
        }
    }

    /// Zoom levels the framing camera is in from fitting the route.
    #[wasm_bindgen(getter, js_name = framingZoom)]
    pub fn framing_zoom(&self) -> f64 {
        self.framing_zoom
    }

    /// Whether the camera follows the marker (so it moves with the progress).
    #[wasm_bindgen(getter)]
    pub fn following(&self) -> bool {
        self.follow.is_some()
    }

    /// The zoom the camera follows the marker at (the one asked for,
    /// clamped), or `undefined` when framing the route.
    #[wasm_bindgen(getter, js_name = followZoom)]
    pub fn follow_zoom(&self) -> Option<f64> {
        self.follow
    }

    /// Vector tiles along the route (at most `maxZoom`, coarser for long
    /// routes) whose names the labels show while following the marker, in
    /// place of [`SceneRenderer::label_tiles`]. Give them to
    /// [`SceneRenderer::add_follow_label_tile`].
    #[wasm_bindgen(js_name = followLabelTiles)]
    pub fn follow_label_tiles(&self, max_zoom: u32) -> Vec<Tile> {
        let mut zoom = max_zoom;
        loop {
            let tiles = route_tiles(&self.points, zoom);
            if tiles.len() <= MAX_FOLLOW_LABEL_TILES || zoom == 0 {
                return tiles.into_iter().map(Tile::from).collect();
            }
            zoom -= 1;
        }
    }

    /// Keep the label layers of an MVT tile fetched for `tile` from
    /// [`SceneRenderer::follow_label_tiles`].
    #[wasm_bindgen(js_name = addFollowLabelTile)]
    pub fn add_follow_label_tile(&mut self, tile: &Tile, bytes: &[u8]) -> Result<(), JsError> {
        let decoded = VectorTile::decode_layers(bytes, &LABEL_LAYERS)
            .map_err(|e| JsError::new(&e.to_string()))?;
        self.follow_label_tiles
            .insert(TilePlacement::from(tile).id, decoded);
        self.gl_follow_label_sources.replace(None);
        Ok(())
    }

    /// The direction of travel at progress `u` in compass degrees (`0`
    /// north, `90` east), from the track `windowM` meters around the marker
    /// (smoothing out GPS zigzags); `undefined` where it barely moves.
    #[wasm_bindgen(js_name = headingAt)]
    pub fn heading_at(&self, u: f64, window_m: f64) -> Option<f64> {
        self.timeline.heading_at(u, window_m)
    }

    /// The track's length in meters.
    #[wasm_bindgen(getter, js_name = trackLengthM)]
    pub fn track_length_m(&self) -> f64 {
        self.timeline.total_distance_m()
    }

    /// The track's length in world pixels at `zoom`, e.g. to pace a camera
    /// following the marker.
    #[wasm_bindgen(js_name = trackLengthPx)]
    pub fn track_length_px(&self, zoom: f64) -> f64 {
        let (lat, _) = self.timeline.position_at(0.5);
        let m_per_px = 40_075_016.686 * lat.to_radians().cos() / (256.0 * 2f64.powf(zoom));
        self.timeline.total_distance_m() / m_per_px
    }

    /// Vector tiles (at most `maxZoom`, the source's) whose names the labels
    /// show: those of the terrain area after
    /// [`SceneRenderer::enable_terrain`], else those the camera sees along
    /// the orbit. Give them to [`SceneRenderer::add_label_tile`].
    #[wasm_bindgen(js_name = labelTiles)]
    pub fn label_tiles(&mut self, max_zoom: u32) -> Vec<Tile> {
        self.tiles_listed = true;
        if let Some(area) = self.terrain_area {
            return area
                .projection()
                .visible_tiles_at(max_zoom, area.width, area.height)
                .into_iter()
                .map(Tile::from)
                .collect();
        }
        let steps = (self.orbit_deg.abs() / LABEL_STEP_DEG).ceil().max(1.0) as usize;
        let mut seen = HashSet::new();
        let orbit = (0..=steps).flat_map(|k| {
            let turn = k as f64 / steps as f64;
            self.camera_at(turn).ground_tiles(max_zoom)
        });
        orbit
            .filter(|p| seen.insert(p.id))
            .map(Tile::from)
            .collect()
    }

    /// Keep the named places, peaks, waters, roads, and rivers of an MVT
    /// tile fetched for `tile`, for labels ([`LABEL_LAYERS`]); the rest of
    /// the tile is skipped.
    #[wasm_bindgen(js_name = addLabelTile)]
    pub fn add_label_tile(&mut self, tile: &Tile, bytes: &[u8]) -> Result<(), JsError> {
        let decoded = VectorTile::decode_layers(bytes, &LABEL_LAYERS)
            .map_err(|e| JsError::new(&e.to_string()))?;
        self.label_tiles
            .insert(TilePlacement::from(tile).id, decoded);
        self.gl_label_sources.replace(None);
        Ok(())
    }

    /// Show map labels in this font (TrueType/OpenType bytes with glyphs for
    /// the map's script, e.g. Korean), from tiles given to
    /// [`SceneRenderer::add_label_tile`]; see `glLabels`. `size` is in CSS
    /// pixels (default 12); `language` prefers `name:<language>` names over
    /// the local `name`.
    #[wasm_bindgen(js_name = setLabels)]
    pub fn set_labels(
        &mut self,
        font: Vec<u8>,
        size: Option<f32>,
        language: Option<String>,
    ) -> Result<(), JsError> {
        self.labels = Some(LabelFont::new(font, size, language)?);
        self.gl_label_sources.replace(None);
        self.gl_follow_label_sources.replace(None);
        Ok(())
    }

    /// Keep map labels off these rectangles (e.g. an attribution laid over
    /// the canvas): `x`, `y`, `width`, `height` each, in scene pixels.
    /// Replaces the previous ones; empty clears them.
    #[wasm_bindgen(js_name = setLabelKeepout)]
    pub fn set_label_keepout(&mut self, rects: &[f32]) {
        self.label_keepout = crate::keepout_rects(rects);
    }

    /// Stop showing map labels.
    #[wasm_bindgen(js_name = clearLabels)]
    pub fn clear_labels(&mut self) {
        self.labels = None;
    }

    /// Switch the basemap style (a preset name, `light` or `dark`, or a JSON
    /// style document), which sets the background, fog, and label colors.
    #[wasm_bindgen(js_name = setMapStyle)]
    pub fn set_map_style(&mut self, style: &str) -> Result<(), JsError> {
        self.map_style = MapStyle::resolve(style)
            .map_err(|e| JsError::new(&e.to_string()))?
            .with_pixel_ratio(self.map_style.pixel_ratio);
        self.gl_label_sources.replace(None);
        self.gl_follow_label_sources.replace(None);
        Ok(())
    }

    /// Override the track color used where elevation is missing (RGB, `0..=255`).
    #[wasm_bindgen(js_name = setTailColor)]
    pub fn set_tail_color(&mut self, r: u8, g: u8, b: u8) {
        self.style.tail_color = Color::from_rgba8(r, g, b, 0xff);
    }
}

impl SceneRenderer {
    /// The terrain the scene stands on, when built and not hidden.
    fn shown_terrain(&self) -> Option<&Terrain> {
        self.terrain.as_ref().filter(|_| self.terrain_visible)
    }

    fn bearing_at(&self, turn: f64) -> f64 {
        self.bearing_deg + self.orbit_deg * turn.clamp(0.0, 1.0)
    }

    /// Camera turned `turn` of the way along the orbit at progress `u`:
    /// following the marker when set to (see [`SceneRenderer::set_follow`]),
    /// else framing the route.
    fn view_at(&self, turn: f64, u: f64) -> Camera {
        let Some(zoom) = self.follow else {
            return self.camera_at(turn);
        };
        let (lat, lon) = self.timeline.position_at(u);
        let shown = self.shown_terrain();
        let marker = gpx_motion_core::marker_vertex(
            &self.timeline,
            &self.camera,
            self.relief,
            self.terrain.as_ref(),
            u,
            &self.style,
        );
        // The marker stands at its lift, or on the shown terrain.
        let top = marker
            .lift
            .max(if shown.is_some() { marker.ground } else { 0.0 });
        let mut camera = self.camera.with_bearing(self.bearing_at(turn));
        // The marker at the depth `zoom` puts the ground at: the target
        // (where the line of sight through it meets the ground) that much
        // farther.
        let near = self.camera.focal() * 2f64.powf(self.camera.zoom - zoom);
        let cos_p = camera.pitch_deg().to_radians().cos().max(f64::EPSILON);
        camera.distance = near + top / cos_p;
        camera.detail_zoom = zoom;
        camera = camera.with_raised_target(lat, lon, top);
        let Some(terrain) = shown else {
            return camera;
        };
        // Back off only as far as the ground under the eye needs.
        Self::clear_of(terrain, camera, near * EYE_CLEARANCE)
    }

    /// Camera turned `turn` of the way along the orbit, re-aimed at the route
    /// and brought in by the framing zoom.
    fn camera_at(&self, turn: f64) -> Camera {
        // Aimed at the fitted distance, then brought in toward the screen's
        // middle: re-aiming up close lets near points cross the near plane.
        let mut camera = self.camera.with_bearing(self.bearing_at(turn)).aim(
            &self.points,
            &self.heights,
            self.padding,
            self.bottom_inset,
        );
        camera.distance *= 2f64.powf(-self.framing_zoom);
        camera.detail_zoom += self.framing_zoom;
        camera
    }

    /// The scene with its first framing target kept (see `start_target`).
    fn with_start_target(mut self) -> Self {
        self.start_target = self.camera_at(0.0).target();
        self
    }

    /// `camera` backed off, toward where it looks from, until it is
    /// `clearance` world pixels above the ground under it; each step moves the
    /// eye over other ground, so it repeats until that settles.
    fn clear_of(terrain: &Terrain, mut camera: Camera, clearance: f64) -> Camera {
        for _ in 0..MAX_CLEARANCE_STEPS {
            let (x, y, _) = camera.eye();
            let before = camera.distance;
            camera = camera.with_min_altitude(terrain.height_at_world(x, y) + clearance);
            if camera.distance - before <= before * 1e-9 {
                break;
            }
        }
        camera
    }
}

/// `points` (with their `heights`) as they are when at most
/// [`MAX_FIT_POINTS`], else evenly sampled down to about that many plus the
/// ones furthest north, south, east, west, and highest, which the framing
/// fits.
fn fit_sample(points: &[TrackPoint], heights: &[f64]) -> (Vec<TrackPoint>, Vec<f64>) {
    if points.len() <= MAX_FIT_POINTS {
        return (points.to_vec(), heights.to_vec());
    }
    let extreme = |key: &dyn Fn(usize) -> f64, max: bool| {
        (0..points.len())
            .max_by(|&a, &b| {
                let order = key(a).total_cmp(&key(b));
                if max {
                    order
                } else {
                    order.reverse()
                }
            })
            .unwrap_or(0)
    };
    let lat = |i: usize| points[i].lat;
    let lon = |i: usize| points[i].lon;
    let height = |i: usize| heights.get(i).copied().unwrap_or(0.0);
    let stride = points.len().div_ceil(MAX_FIT_POINTS);
    let mut kept: Vec<usize> = (0..points.len()).step_by(stride).collect();
    kept.extend([
        extreme(&lat, true),
        extreme(&lat, false),
        extreme(&lon, true),
        extreme(&lon, false),
        extreme(&height, true),
    ]);
    kept.sort_unstable();
    kept.dedup();
    let sampled = kept.iter().map(|&i| points[i].clone()).collect();
    let sampled_heights = if heights.is_empty() {
        Vec::new()
    } else {
        kept.iter().map(|&i| height(i)).collect()
    };
    (sampled, sampled_heights)
}

/// Tiles at `zoom` under the route's points and around them, for labels
/// along it.
fn route_tiles(points: &[TrackPoint], zoom: u32) -> Vec<TilePlacement> {
    let n = 1i64 << zoom;
    let mut seen = HashSet::new();
    for p in points {
        let x = (lon_to_pixel(p.lon, zoom as f64) / 256.0).floor() as i64;
        let y = (lat_to_pixel(p.lat, zoom as f64) / 256.0).floor() as i64;
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
        .map(|(x, y)| TilePlacement {
            id: TileId { zoom, x, y },
            dx: 0.0,
            dy: 0.0,
            size: 256.0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    /// A straight ~40 km route north, too long for a terrain area at its zoom.
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

    fn scene() -> SceneRenderer {
        let gpx = include_str!("../../../samples/namsan.gpx");
        let Ok(scene) = SceneRenderer::new(gpx, 480, 360, 55.0, 20.0, 360.0, None) else {
            panic!("scene should build");
        };
        scene
    }

    #[test]
    fn terrain_must_be_enabled_before_listing_tiles() {
        let mut scene = scene();
        let camera = scene.camera;
        assert!(!scene.label_tiles(12).is_empty());
        assert!(!scene.enable_terrain());
        assert_eq!(scene.camera, camera);
    }

    #[test]
    fn label_tiles_cover_the_terrain_area_or_the_orbit() {
        let mut terrain = scene();
        assert!(terrain.enable_terrain());
        let area = terrain.label_tiles(14);
        assert!(!area.is_empty() && area.iter().all(|t| t.zoom <= 14));
        let Ok(mut long) = SceneRenderer::new(&long_route(), 640, 640, 55.0, 20.0, 360.0, None)
        else {
            panic!("scene should build");
        };
        let orbit = long.label_tiles(14);
        let ids: HashSet<(u32, u32, u32)> = orbit.iter().map(|t| (t.zoom, t.x, t.y)).collect();
        assert_eq!(ids.len(), orbit.len(), "no duplicates");
        // Every tile the camera shows between the sampled bearings is listed.
        for turn in (0..=720).map(|i| i as f64 / 720.0) {
            for p in long.camera_at(turn).ground_tiles(14) {
                assert!(ids.contains(&(p.id.zoom, p.id.x, p.id.y)), "turn {turn}");
            }
        }
    }

    #[test]
    fn switching_the_map_style_keeps_the_terrain() {
        let mut scene = scene();
        assert!(scene.enable_terrain());
        assert!(scene.set_terrain_heights().is_ok());
        assert!(scene.set_map_style("light").is_ok());
        assert!(scene.terrain.is_some());
    }

    #[test]
    fn follow_label_tiles_stay_within_their_cap() {
        let Ok(long) = SceneRenderer::new(&long_route(), 640, 640, 55.0, 20.0, 360.0, None) else {
            panic!("scene should build");
        };
        assert_eq!(long.follow_label_tiles(14)[0].zoom, 14);
        // Too many at 18 for a 40 km route: coarser ones instead.
        let tiles = long.follow_label_tiles(18);
        assert!(!tiles.is_empty() && tiles.len() <= MAX_FOLLOW_LABEL_TILES);
        let zoom = tiles[0].zoom;
        assert!(zoom < 18, "{zoom}");
        assert!(tiles.iter().all(|t| t.zoom == zoom));
    }

    #[test]
    fn long_tracks_fit_by_a_sample_with_their_extremes() {
        let points: Vec<TrackPoint> = (0..2000)
            .map(|i| {
                TrackPoint::new(
                    37.5 + (i as f64 * 0.37).sin() * 0.01,
                    127.0 + i as f64 * 1e-5,
                )
            })
            .collect();
        let heights: Vec<f64> = (0..2000)
            .map(|i| if i == 1001 { 500.0 } else { 0.0 })
            .collect();
        let (sample, sampled_heights) = fit_sample(&points, &heights);
        assert!(sample.len() <= MAX_FIT_POINTS + 5);
        assert_eq!(sample.len(), sampled_heights.len());
        assert!(sampled_heights.contains(&500.0), "the highest point stays");
        let north = points.iter().map(|p| p.lat).fold(f64::MIN, f64::max);
        assert!(sample.iter().any(|p| p.lat == north));
        let short = &points[..100];
        assert_eq!(fit_sample(short, &heights[..100]).0, short.to_vec());
    }

    #[test]
    fn headings_follow_the_route() {
        let Ok(long) = SceneRenderer::new(&long_route(), 640, 640, 55.0, 20.0, 360.0, None) else {
            panic!("scene should build");
        };
        // The long route runs north.
        let Some(heading) = long.heading_at(0.5, 150.0) else {
            panic!("it moves");
        };
        assert!(heading.min(360.0 - heading) < 5.0, "{heading}");
        assert!(long.track_length_m() > 30_000.0);
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
