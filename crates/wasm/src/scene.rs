//! The WebGL 3D scene: a route raised over the ground for an orbiting or
//! following camera, handed to the page as GPU-ready data.

use std::fmt;

use gpx_motion_core::mercator::meters_per_pixel;
use gpx_motion_core::{
    parse, FrameStyle, Lens, MapStyle, ProfileStyle, Relief, Timeline, TrackPoint,
};
use tiny_skia::Color;
use wasm_bindgen::prelude::*;

use crate::js_error;

mod camera;
mod gl;
mod gl_labels;
mod labels;
mod terrain;

use camera::{CameraRig, Framing};
use labels::LabelState;
use terrain::{TerrainError, TerrainState};

pub use gl_labels::GlLabels;

/// A parsed GPX track as a 3D scene for WebGL: the route raised by its
/// elevation over the ground, framed for an orbiting camera, with an
/// elevation profile panel.
///
/// The camera can turn up to `orbit` degrees from its start bearing, chosen
/// per frame independently of the track progress. The scene hands its
/// contents to the page as data and fetching stays on the JS side: the
/// scene lists the tiles it needs ([`SceneRenderer::terrain_tiles`],
/// [`SceneRenderer::label_tiles`]) and takes back their bytes.
///
/// Whether the ground has terrain is decided when the scene is built (see
/// [`SceneRenderer::new`]), before anything is listed or handed out, so the
/// camera and the scene origin never change under data already given to
/// the page.
///
/// JS names follow the [crate's convention](crate#naming): `gl*` for what
/// the 3D view takes each frame (and its drawing constants),
/// `set*`/`add*`/`clear*` for changes, and plain names for state and tile
/// listings.
#[wasm_bindgen]
pub struct SceneRenderer {
    route: Route,
    style: FrameStyle,
    profile: ProfileStyle,
    map_style: MapStyle,
    view: CameraRig,
    labels: LabelState,
    terrain: TerrainState,
}

/// The track the scene shows, with each point's height above the ground.
pub(super) struct Route {
    timeline: Timeline,
    points: Vec<TrackPoint>,
    /// Height of each point above the track's lowest, in meters, by `relief`.
    heights: Vec<f64>,
    relief: Relief,
}

impl Route {
    fn new(points: Vec<TrackPoint>, timeline: Timeline, terrain: bool) -> Self {
        let relief = Relief::auto(&timeline);
        // Over terrain the track's exaggeration is capped.
        let relief = if terrain {
            relief.over_terrain()
        } else {
            relief
        };
        let heights = relief.heights_m(&timeline);
        Self {
            timeline,
            points,
            heights,
            relief,
        }
    }
}

/// Why a scene could not be framed.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SceneError {
    /// The track doesn't fit the viewport.
    Unfit,
    /// Terrain was asked for and can't be had.
    Terrain(TerrainError),
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unfit => f.write_str("could not fit track to viewport"),
            Self::Terrain(e) => e.fmt(f),
        }
    }
}

impl From<TerrainError> for SceneError {
    fn from(e: TerrainError) -> Self {
        Self::Terrain(e)
    }
}

#[wasm_bindgen]
impl SceneRenderer {
    /// Parse GPX text and frame the route for a `width`×`height` viewport seen
    /// at `pitch` degrees from straight down, facing `bearing` degrees, far
    /// enough back to stay in view anywhere along an `orbit`-degree turn.
    /// `pixelRatio` (default `1`) scales strokes, text, and map line widths
    /// for a canvas sized in device pixels.
    ///
    /// With `terrain`, the ground gets relief from DEM tiles (see
    /// [`SceneRenderer::terrain_tiles`]): the track's height exaggeration is
    /// capped as over terrain and the camera framed for it. Throws, with a
    /// message starting `no terrain:` and naming the reason, when the route
    /// can't have terrain (too large an area at its zoom); build the scene
    /// again without it to show flat ground.
    ///
    /// # Errors
    ///
    /// Throws when `gpx` doesn't parse or holds fewer than two distinct points,
    /// and (with a `no terrain:` message) when terrain was asked for but the
    /// route can't have it.
    #[wasm_bindgen(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gpx: &str,
        width: u32,
        height: u32,
        pitch: f64,
        bearing: f64,
        orbit: f64,
        pixel_ratio: Option<f32>,
        terrain: Option<bool>,
    ) -> Result<SceneRenderer, JsError> {
        let ratio = pixel_ratio
            .filter(|r| r.is_finite() && *r > 0.0)
            .unwrap_or(1.0);
        let points = parse(gpx).map_err(js_error)?;
        let timeline = Timeline::build(&points)
            .ok_or_else(|| JsError::new("track needs two distinct points"))?;
        let lens = Lens {
            pitch_deg: pitch,
            bearing_deg: bearing,
            ..Lens::default()
        };
        let size = (width, height);
        Self::build(
            points,
            timeline,
            size,
            lens,
            orbit,
            ratio,
            terrain == Some(true),
        )
        .map_err(js_error)
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
        self.route.timeline.duration_ms
    }

    /// The direction of travel at progress `u` in compass degrees (`0`
    /// north, `90` east), from the track `windowM` meters around the marker
    /// (smoothing out GPS zigzags); `undefined` where it barely moves.
    #[wasm_bindgen(js_name = headingAt)]
    pub fn heading_at(&self, u: f64, window_m: f64) -> Option<f64> {
        self.route.timeline.heading_at(u, window_m)
    }

    /// The track's length in meters.
    #[wasm_bindgen(getter, js_name = trackLengthM)]
    pub fn track_length_m(&self) -> f64 {
        self.route.timeline.total_distance_m()
    }

    /// The track's length in world pixels at `zoom`, e.g. to pace a camera
    /// following the marker.
    #[wasm_bindgen(js_name = trackLengthPx)]
    pub fn track_length_px(&self, zoom: f64) -> f64 {
        let (lat, _) = self.route.timeline.position_at(0.5);
        self.route.timeline.total_distance_m() / meters_per_pixel(lat, zoom)
    }

    /// Switch the basemap style (a preset name, `light` or `dark`, or a JSON
    /// style document), which sets the background, fog, and label colors.
    ///
    /// # Errors
    ///
    /// Throws when `style` is neither a preset nor a valid style document.
    #[wasm_bindgen(js_name = setMapStyle)]
    pub fn set_map_style(&mut self, style: &str) -> Result<(), JsError> {
        self.map_style = MapStyle::resolve(style)
            .map_err(js_error)?
            .with_pixel_ratio(self.map_style.pixel_ratio);
        self.labels.invalidate();
        Ok(())
    }

    /// Override the track color used where elevation is missing (RGB, `0..=255`).
    #[wasm_bindgen(js_name = setTailColor)]
    pub fn set_tail_color(&mut self, r: u8, g: u8, b: u8) {
        self.style.tail_color = Color::from_rgba8(r, g, b, 0xff);
    }
}

impl SceneRenderer {
    /// The scene of a parsed track, framed for a `size` viewport through
    /// `lens` and an `orbit`-degree turn, over terrain when `terrain`.
    fn build(
        points: Vec<TrackPoint>,
        timeline: Timeline,
        (width, height): (u32, u32),
        lens: Lens,
        orbit: f64,
        ratio: f32,
        terrain: bool,
    ) -> Result<Self, SceneError> {
        let style = FrameStyle {
            width,
            height,
            color_by_elevation: true,
            ..Default::default()
        }
        .scaled(ratio);
        let profile = ProfileStyle::default().scaled(ratio);
        let route = Route::new(points, timeline, terrain);
        let framing = Framing {
            width,
            height,
            padding: style.padding,
            bottom_inset: f64::from(profile.reserved_height()),
            lens,
            orbit_deg: orbit,
        };
        let Some(view) = CameraRig::fit(&route, framing) else {
            return Err(if terrain {
                TerrainError::Unfit.into()
            } else {
                SceneError::Unfit
            });
        };
        let terrain = if terrain {
            TerrainState::around(&route.points, view.fitted().zoom)?
        } else {
            TerrainState::flat()
        };
        Ok(Self {
            route,
            style,
            profile,
            map_style: MapStyle::dark().with_pixel_ratio(ratio),
            view,
            labels: LabelState::new(),
            terrain,
        })
    }
}

#[cfg(test)]
mod test_support {
    use std::fmt::Write;

    use super::*;

    /// A straight ~40 km route north, too long for a terrain area at the zoom
    /// that frames it in a 2048 px viewport.
    pub(super) fn long_route() -> String {
        let mut gpx = String::from(r#"<gpx version="1.1"><trk><trkseg>"#);
        for i in 0..200 {
            let lat = 35.80 + f64::from(i) * 0.0018;
            let lon = 129.20 + (f64::from(i) * 0.3).sin() * 0.01;
            let _ = write!(
                gpx,
                r#"<trkpt lat="{lat}" lon="{lon}"><ele>{}</ele></trkpt>"#,
                20.0 + f64::from(i % 40)
            );
        }
        gpx + "</trkseg></trk></gpx>"
    }

    /// The scene of `gpx` in a `width`×`height` viewport at pitch 55,
    /// bearing 20, orbiting a full turn, over terrain when `terrain`.
    pub(super) fn build(
        gpx: &str,
        (width, height): (u32, u32),
        terrain: bool,
    ) -> Result<SceneRenderer, SceneError> {
        let points = parse(gpx).expect("valid GPX");
        let timeline = Timeline::build(&points).expect("a track");
        let lens = Lens {
            pitch_deg: 55.0,
            bearing_deg: 20.0,
            ..Lens::default()
        };
        SceneRenderer::build(points, timeline, (width, height), lens, 360.0, 1.0, terrain)
    }

    /// The Namsan sample as a 480×360 scene, over terrain when `terrain`.
    pub(super) fn namsan(terrain: bool) -> SceneRenderer {
        let gpx = include_str!("../../../samples/namsan.gpx");
        let Ok(scene) = build(gpx, (480, 360), terrain) else {
            panic!("scene should build");
        };
        scene
    }

    /// A Terrarium tile rising 512–767 m from west to east.
    pub(super) fn slope_tile() -> Vec<u8> {
        (0..256 * 256)
            .flat_map(|i| [130, (i % 256) as u8, 0, 255])
            .collect()
    }

    /// [`namsan`] over terrain built from [`slope_tile`]s.
    pub(super) fn with_terrain() -> SceneRenderer {
        let mut scene = namsan(true);
        for tile in scene.terrain_tiles() {
            scene.add_terrain_tile(&tile, &slope_tile());
        }
        assert!(scene.set_terrain_heights().is_ok());
        scene
    }

    /// [`long_route`] as a flat 640×640 scene.
    pub(super) fn long() -> SceneRenderer {
        let Ok(scene) = build(&long_route(), (640, 640), false) else {
            panic!("scene should build");
        };
        scene
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use gpx_motion_core::{Camera, TerrainArea};

    use super::test_support::*;
    use super::*;

    /// The camera `enableTerrain` used to fit: on the relief capped for
    /// terrain, widened for the orbit, after the flat scene was built.
    fn capped_fit(
        gpx: &str,
        (width, height): (u32, u32),
        lens: Lens,
    ) -> (Camera, Vec<TrackPoint>, Vec<f64>) {
        let points = parse(gpx).expect("valid GPX");
        let timeline = Timeline::build(&points).expect("a track");
        let heights = Relief::auto(&timeline).over_terrain().heights_m(&timeline);
        let padding = FrameStyle::default().scaled(1.0).padding;
        let inset = f64::from(ProfileStyle::default().scaled(1.0).reserved_height());
        let camera = Camera::fit(&points, &heights, width, height, padding, inset, lens)
            .expect("fits")
            .widen_for_orbit(&points, &heights, padding, inset, 360.0);
        (camera, points, heights)
    }

    #[test]
    fn terrain_at_construction_matches_enabling_it_afterwards() {
        let namsan = include_str!("../../../samples/namsan.gpx");
        let long = long_route();
        let lens = Lens {
            pitch_deg: 55.0,
            bearing_deg: 20.0,
            ..Lens::default()
        };
        let padding = FrameStyle::default().scaled(1.0).padding;
        let inset = f64::from(ProfileStyle::default().scaled(1.0).reserved_height());
        for (gpx, (w, h)) in [(namsan, (480, 360)), (long.as_str(), (640, 640))] {
            let Ok(scene) = SceneRenderer::new(gpx, w, h, 55.0, 20.0, 360.0, None, Some(true))
            else {
                panic!("scene should build");
            };
            let (camera, points, heights) = capped_fit(gpx, (w, h), lens);
            assert_eq!(*scene.view.fitted(), camera);
            let start = camera
                .with_bearing(20.0)
                .aim(&points, &heights, padding, inset)
                .target();
            assert_eq!(scene.view.start_target(), start);
            let area = TerrainArea::around(&points, camera.zoom, 2048.0);
            assert_eq!(scene.terrain.area(), area);
        }
    }

    #[test]
    fn terrain_is_refused_with_its_reason() {
        let Err(err) = build(&long_route(), (2048, 2048), true) else {
            panic!("too large for terrain");
        };
        let SceneError::Terrain(TerrainError::TooLarge { width, height }) = err else {
            panic!("{err:?}");
        };
        assert!(width.max(height) > 2048, "{width}×{height}");
        assert!(err.to_string().contains("too large"), "{err}");
        // The same route builds flat.
        let Ok(flat) = build(&long_route(), (2048, 2048), false) else {
            panic!("scene should build");
        };
        assert!(flat.terrain_tiles().is_empty());
    }

    #[test]
    fn terrain_caps_the_relief_and_reframes() {
        // The long route barely climbs, so its relief is exaggerated flat.
        let flat = long();
        let Ok(terrain) = build(&long_route(), (640, 640), true) else {
            panic!("fits at 640 px");
        };
        assert_eq!(flat.route.relief.over_terrain(), terrain.route.relief);
        assert_ne!(flat.route.relief, terrain.route.relief, "capped");
        assert_ne!(flat.view.fitted(), terrain.view.fitted(), "reframed for it");
        assert!(flat.terrain_tiles().is_empty());
        assert!(!terrain.terrain_tiles().is_empty());
        // Namsan's relief is under the cap: terrain leaves its framing alone.
        assert_eq!(namsan(false).view.fitted(), namsan(true).view.fitted());
    }

    #[test]
    fn switching_the_map_style_keeps_the_terrain() {
        let mut scene = namsan(true);
        assert!(scene.set_terrain_heights().is_ok());
        assert!(scene.set_map_style("light").is_ok());
        assert!(scene.terrain.built().is_some());
    }

    #[test]
    fn headings_follow_the_route() {
        let long = long();
        // The long route runs north.
        let Some(heading) = long.heading_at(0.5, 150.0) else {
            panic!("it moves");
        };
        assert!(heading.min(360.0 - heading) < 5.0, "{heading}");
        assert!(long.track_length_m() > 30_000.0);
        // Run backward it heads south.
        let points = parse(&long_route()).expect("valid GPX");
        let mut gpx = String::from(r#"<gpx version="1.1"><trk><trkseg>"#);
        for p in points.iter().rev() {
            let _ = write!(gpx, r#"<trkpt lat="{}" lon="{}"></trkpt>"#, p.lat, p.lon);
        }
        gpx += "</trkseg></trk></gpx>";
        let Ok(back) = build(&gpx, (640, 640), false) else {
            panic!("scene should build");
        };
        let Some(heading) = back.heading_at(0.5, 150.0) else {
            panic!("it moves");
        };
        assert!((heading - 180.0).abs() < 5.0, "{heading}");
    }
}
