use std::cell::RefCell;
use std::collections::HashMap;

use gpx_motion_core::{
    draw_fog, draw_profile, draw_track_3d, draw_vector_map_3d, parse, reproject_ground, Camera,
    FrameStyle, GroundView, Lens, MapStyle, ProfileStyle, Relief, SceneView, TileId, TilePlacement,
    Timeline, TrackPoint, VectorTile,
};
use tiny_skia::{Color, Pixmap};
use wasm_bindgen::prelude::*;

use crate::{to_straight_rgba, Tile};

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

/// A parsed GPX track rendered as a tilted 3D scene: the route raised by its
/// elevation over a perspective basemap, with an elevation profile panel.
///
/// The camera can turn up to `orbit` degrees from its start bearing, chosen
/// per frame independently of the track progress. Tile fetching stays on the
/// JS side: [`SceneRenderer::ground_tiles`] lists what the whole orbit will
/// see, and [`SceneRenderer::add_vector_tile`] hands the bytes back.
#[wasm_bindgen]
pub struct SceneRenderer {
    timeline: Timeline,
    style: FrameStyle,
    profile: ProfileStyle,
    relief: Relief,
    camera: Camera,
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
    keyframes: RefCell<HashMap<i64, GroundView>>,
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
            keyframes: RefCell::new(HashMap::new()),
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

    /// Every vector tile the camera sees over the whole orbit, without
    /// duplicates. Pass the source's `maxZoom`; later frames use the same cap.
    #[wasm_bindgen(js_name = groundTiles)]
    pub fn ground_tiles(&mut self, max_zoom: u32) -> Vec<Tile> {
        self.max_zoom = max_zoom;
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

    /// Switch the basemap style (`light` or `dark`).
    #[wasm_bindgen(js_name = setMapStyle)]
    pub fn set_map_style(&mut self, style: &str) -> Result<(), JsError> {
        self.map_style = MapStyle::preset(style)
            .ok_or_else(|| JsError::new(&format!("unknown map style `{style}`")))?
            .with_pixel_ratio(self.map_style.pixel_ratio);
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
        let view = SceneView {
            camera: &camera,
            relief: self.relief,
            terrain: None,
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
        let keys: Vec<i64> = keys.into_iter().map(|k| k.rem_euclid(KEYFRAMES)).collect();
        for &key in &keys {
            if !self.keyframes.borrow().contains_key(&key) {
                let view = self.keyframe(key);
                self.keyframes.borrow_mut().insert(key, view);
            }
        }
        let keyframes = self.keyframes.borrow();
        let sources: Vec<&GroundView> = keys.iter().map(|k| &keyframes[k]).collect();
        let mut pixmap =
            Pixmap::new(self.style.width, self.style.height).expect("viewport must be non-zero");
        reproject_ground(&mut pixmap, camera, &sources, self.map_style.background);
        draw_fog(&mut pixmap, camera, self.map_style.background);
        self.ground.replace(Some((bearing, pixmap.clone())));
        pixmap
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

    fn clear_ground(&self) {
        self.ground.replace(None);
        self.keyframes.borrow_mut().clear();
    }
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
