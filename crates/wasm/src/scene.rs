use std::cell::RefCell;
use std::collections::HashMap;

use gpx_motion_core::{
    draw_fog, draw_profile, draw_track_3d, draw_vector_map_3d, parse, Camera, FrameStyle, Lens,
    MapStyle, ProfileStyle, Relief, TileId, TilePlacement, Timeline, TrackPoint, VectorTile,
};
use tiny_skia::{Color, Pixmap};
use wasm_bindgen::prelude::*;

use crate::{to_straight_rgba, Tile};

/// Degrees between the orbit bearings sampled when collecting ground tiles.
const ORBIT_SAMPLE_DEG: f64 = 5.0;

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
    max_zoom: u32,
    map_style: MapStyle,
    tiles: HashMap<TileId, VectorTile>,
    ground: RefCell<Option<(f64, Pixmap)>>,
}

#[wasm_bindgen]
impl SceneRenderer {
    /// Parse GPX text and frame the route for a `width`×`height` viewport seen
    /// at `pitch` degrees from straight down, facing `bearing` degrees, far
    /// enough back to stay in view anywhere along an `orbit`-degree turn.
    #[wasm_bindgen(constructor)]
    pub fn new(
        gpx: &str,
        width: u32,
        height: u32,
        pitch: f64,
        bearing: f64,
        orbit: f64,
    ) -> Result<SceneRenderer, JsError> {
        let points = parse(gpx).map_err(|e| JsError::new(&e.to_string()))?;
        let timeline = Timeline::build(&points)
            .ok_or_else(|| JsError::new("track needs two distinct points"))?;
        let style = FrameStyle {
            width,
            height,
            color_by_elevation: true,
            ..Default::default()
        };
        let profile = ProfileStyle::default();
        let relief = Relief::auto(&timeline);
        let lens = Lens {
            pitch_deg: pitch,
            bearing_deg: bearing,
            ..Lens::default()
        };
        let heights: Vec<f64> = points.iter().map(|p| relief.height_m(p.ele)).collect();
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
            max_zoom: camera.zoom,
            map_style: MapStyle::dark(),
            tiles: HashMap::new(),
            ground: RefCell::new(None),
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
        let steps = (self.orbit_deg.abs() / ORBIT_SAMPLE_DEG).ceil() as usize;
        let mut seen = HashMap::new();
        for k in 0..=steps {
            let t = if steps == 0 {
                0.0
            } else {
                k as f64 / steps as f64
            };
            for placement in self.camera_at(t).ground_tiles(max_zoom) {
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
        self.ground.replace(None);
        Ok(())
    }

    /// Switch the basemap style (`light` or `dark`).
    #[wasm_bindgen(js_name = setMapStyle)]
    pub fn set_map_style(&mut self, style: &str) -> Result<(), JsError> {
        self.map_style = MapStyle::preset(style)
            .ok_or_else(|| JsError::new(&format!("unknown map style `{style}`")))?;
        self.ground.replace(None);
        Ok(())
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
    /// The basemap is redrawn only when `turn` changes; that costs far more
    /// than the track and profile, so animate `u` and keep `turn` steady.
    #[wasm_bindgen(js_name = renderFrame)]
    pub fn render_frame(&self, u: f64, turn: f64) -> Vec<u8> {
        let bearing = self.bearing_at(turn);
        let camera = self.camera_at(turn);
        let mut pixmap = self.ground(&camera, bearing);
        draw_track_3d(
            &mut pixmap,
            &self.timeline,
            &camera,
            &self.relief,
            u,
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
        self.camera.with_bearing(self.bearing_at(turn)).aim(
            &self.points,
            &self.heights,
            self.padding,
            self.bottom_inset,
        )
    }

    fn ground(&self, camera: &Camera, bearing: f64) -> Pixmap {
        if let Some((cached, pixmap)) = &*self.ground.borrow() {
            if *cached == bearing {
                return pixmap.clone();
            }
        }
        let mut pixmap =
            Pixmap::new(self.style.width, self.style.height).expect("viewport must be non-zero");
        pixmap.fill(self.map_style.background);
        let tiles: Vec<(TilePlacement, &VectorTile)> = camera
            .ground_tiles(self.max_zoom)
            .into_iter()
            .filter_map(|p| self.tiles.get(&p.id).map(|tile| (p, tile)))
            .collect();
        draw_vector_map_3d(&mut pixmap, &tiles, &self.map_style, camera, 1.0);
        draw_fog(&mut pixmap, camera, self.map_style.background);
        self.ground.replace(Some((bearing, pixmap.clone())));
        pixmap
    }
}
