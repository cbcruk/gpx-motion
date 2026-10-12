//! The scene's cameras: the one fitted to the route, the framing camera
//! turned along the orbit and brought in by the framing zoom, and the one
//! following the marker, each kept above the shown terrain.

use gpx_motion_core::{
    marker_vertex, Camera, FrameStyle, Lens, Terrain, TrackPoint, MAX_PITCH_DEG, TERRAIN_CLEARANCE,
};
use wasm_bindgen::prelude::*;

use super::terrain::TerrainState;
use super::{Route, SceneRenderer};

/// Zoom levels the framing camera can come in from the route's fit, and
/// back off from it.
const FRAMING_ZOOM: (f64, f64) = (-1.0, 4.0);
/// Finest zoom a camera following the marker comes in to.
pub(super) const MAX_FOLLOW_ZOOM: f64 = 18.0;
/// Height a following camera keeps above the ground under it, as a share
/// of its distance to the marker.
pub(super) const EYE_CLEARANCE: f64 = 0.2;
/// Most track points a refit for a new tilt looks at.
const MAX_FIT_POINTS: usize = 256;
/// Most times a following camera backs off to clear the ground under it.
const MAX_CLEARANCE_STEPS: usize = 8;

/// What the camera frames the route in: the viewport, the room kept around
/// the route, the lens, and the orbit it must stay in view along.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Framing {
    pub width: u32,
    pub height: u32,
    pub padding: f64,
    /// Screen rows kept clear along the bottom (the profile panel).
    pub bottom_inset: f64,
    pub lens: Lens,
    pub orbit_deg: f64,
}

/// The camera fitted to the route and how the page moves it: along the
/// orbit, closer by the framing zoom, or following the marker.
#[derive(Debug, Clone)]
pub(super) struct CameraRig {
    /// Fitted to the whole route at the start bearing; its world zoom is
    /// the scene's.
    fitted: Camera,
    framing: Framing,
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

impl CameraRig {
    /// The camera framing `route` as `framing` asks, far enough back to keep
    /// it in view along the orbit; `None` when it can't fit.
    pub fn fit(route: &Route, framing: Framing) -> Option<Self> {
        let fitted = Camera::fit(
            &route.points,
            &route.heights,
            framing.width,
            framing.height,
            framing.padding,
            framing.bottom_inset,
            framing.lens,
        )?
        .widen_for_orbit(
            &route.points,
            &route.heights,
            framing.padding,
            framing.bottom_inset,
            framing.orbit_deg,
        );
        let mut rig = Self {
            fitted,
            framing,
            follow: None,
            framing_zoom: 0.0,
            start_target: (0.0, 0.0),
        };
        rig.start_target = rig.camera_at(route, 0.0).target();
        Some(rig)
    }

    /// The camera fitted to the whole route at the start bearing.
    pub fn fitted(&self) -> &Camera {
        &self.fitted
    }

    /// Where the framing camera first aimed.
    pub fn start_target(&self) -> (f64, f64) {
        self.start_target
    }

    /// The orbit's sweep in degrees.
    pub fn orbit_deg(&self) -> f64 {
        self.framing.orbit_deg
    }

    /// Screen rows kept clear along the bottom.
    pub fn bottom_inset(&self) -> f64 {
        self.framing.bottom_inset
    }

    /// The zoom the camera follows the marker at, or `None` when framing.
    pub fn follow(&self) -> Option<f64> {
        self.follow
    }

    /// Follow the marker at `zoom`, clamped between the fitted zoom and
    /// [`MAX_FOLLOW_ZOOM`], or frame the route with `None`.
    pub fn set_follow(&mut self, zoom: Option<f64>) {
        let fitted = self.fitted.zoom;
        self.follow = zoom
            .filter(|z| z.is_finite())
            .map(|z| z.clamp(fitted, MAX_FOLLOW_ZOOM.max(fitted)));
    }

    /// The tilt from straight down, in degrees.
    pub fn pitch_deg(&self) -> f64 {
        self.framing.lens.pitch_deg
    }

    /// Tilt to `pitch` degrees (clamped to `0..=`[`MAX_PITCH_DEG`]) and refit
    /// the framing distance for it at the same world zoom; a non-finite
    /// pitch changes nothing.
    pub fn set_pitch(&mut self, route: &Route, pitch: f64) {
        if !pitch.is_finite() {
            return;
        }
        let framing = &mut self.framing;
        framing.lens.pitch_deg = pitch.clamp(0.0, MAX_PITCH_DEG);
        // Long tracks fit by a sample of their points (and their extremes),
        // so tilting stays quick.
        let (points, heights) = fit_sample(&route.points, &route.heights);
        self.fitted = self
            .fitted
            .with_pitch(framing.lens.pitch_deg)
            .refit_for_orbit(
                framing.lens.bearing_deg,
                &points,
                &heights,
                framing.padding,
                framing.bottom_inset,
                framing.orbit_deg,
            );
    }

    /// Zoom levels the framing camera is in from fitting the route.
    pub fn framing_zoom(&self) -> f64 {
        self.framing_zoom
    }

    /// Bring the framing camera `levels` closer (clamped to
    /// [`FRAMING_ZOOM`]); a non-finite value changes nothing.
    pub fn set_framing_zoom(&mut self, levels: f64) {
        if levels.is_finite() {
            self.framing_zoom = levels.clamp(FRAMING_ZOOM.0, FRAMING_ZOOM.1);
        }
    }

    /// The bearing `turn` (clamped to `0..=1`) of the way along the orbit.
    pub fn bearing_at(&self, turn: f64) -> f64 {
        self.framing.lens.bearing_deg + self.framing.orbit_deg * turn.clamp(0.0, 1.0)
    }

    /// Camera turned `turn` of the way along the orbit, re-aimed at the route
    /// and brought in by the framing zoom.
    pub fn camera_at(&self, route: &Route, turn: f64) -> Camera {
        // Aimed at the fitted distance, then brought in toward the screen's
        // middle: re-aiming up close lets near points cross the near plane.
        let mut camera = self.fitted.with_bearing(self.bearing_at(turn)).aim(
            &route.points,
            &route.heights,
            self.framing.padding,
            self.framing.bottom_inset,
        );
        camera.distance *= 2f64.powf(-self.framing_zoom);
        camera.detail_zoom += self.framing_zoom;
        camera
    }

    /// Camera turned `turn` of the way along the orbit at progress `u`:
    /// following the marker when set to (see [`CameraRig::set_follow`]),
    /// else framing the route.
    pub fn view_at(
        &self,
        route: &Route,
        style: &FrameStyle,
        terrain: &TerrainState,
        turn: f64,
        u: f64,
    ) -> Camera {
        let Some(zoom) = self.follow else {
            return self.camera_at(route, turn);
        };
        let (lat, lon) = route.timeline.position_at(u);
        let shown = terrain.shown();
        let marker = marker_vertex(
            &route.timeline,
            &self.fitted,
            route.relief,
            terrain.built(),
            u,
            style,
        );
        // The marker stands at its lift, or on the shown terrain.
        let top = marker
            .lift
            .max(if shown.is_some() { marker.ground } else { 0.0 });
        let mut camera = self.fitted.with_bearing(self.bearing_at(turn));
        // The marker at the depth `zoom` puts the ground at: the target
        // (where the line of sight through it meets the ground) that much
        // farther.
        let near = self.follow_depth(zoom);
        let cos_p = camera.pitch_deg().to_radians().cos().max(f64::EPSILON);
        camera.distance = near + top / cos_p;
        camera.detail_zoom = zoom;
        camera = camera.with_raised_target(lat, lon, top);
        let Some(terrain) = shown else {
            return camera;
        };
        // Back off only as far as the ground under the eye needs.
        clear_of(terrain, camera, near * EYE_CLEARANCE)
    }

    /// The GL camera at `turn` along the orbit and progress `u`, kept above
    /// the shown terrain.
    pub fn gl_camera_at(
        &self,
        route: &Route,
        style: &FrameStyle,
        terrain: &TerrainState,
        turn: f64,
        u: f64,
    ) -> Camera {
        if self.follow.is_some() {
            return self.view_at(route, style, terrain, turn, u);
        }
        let camera = self.camera_at(route, turn);
        let Some(terrain) = terrain.shown() else {
            return camera;
        };
        // Above the highest peak at the fit; brought closer, that height
        // shrinks with the zoom (or it couldn't come in over hills) while the
        // ground under the eye takes over, both easing in from the fit so the
        // view doesn't jump.
        let closer = self.framing_zoom.max(0.0);
        let peak = terrain.max_height() * TERRAIN_CLEARANCE * 2f64.powf(-closer);
        let camera = camera.with_min_altitude(peak);
        if closer == 0.0 {
            return camera;
        }
        let clearance = camera.distance * EYE_CLEARANCE * closer.min(1.0);
        clear_of(terrain, camera, clearance)
    }

    /// How far before a following camera, in world pixels, the ground at
    /// follow zoom `zoom` lies.
    fn follow_depth(&self, zoom: f64) -> f64 {
        self.fitted.focal() * 2f64.powf(self.fitted.zoom - zoom)
    }
}

#[wasm_bindgen]
impl SceneRenderer {
    /// Follow the marker at world zoom `zoom` (clamped between the route's
    /// framing zoom and 18), or frame the whole route again with `None`.
    /// The orbit's bearing still applies.
    #[wasm_bindgen(js_name = setFollow)]
    pub fn set_follow(&mut self, zoom: Option<f64>) {
        self.view.set_follow(zoom);
    }

    /// The zoom the camera follows the marker at (the one asked for,
    /// clamped), or `undefined` when framing the route.
    #[wasm_bindgen(getter, js_name = followZoom)]
    pub fn follow_zoom(&self) -> Option<f64> {
        self.view.follow()
    }

    /// Tilt the camera `pitch` degrees from straight down (clamped to
    /// `0..=75`), refitting the framing distance for it at the same world
    /// zoom, so terrain and tiles already listed still fit.
    #[wasm_bindgen(js_name = setPitch)]
    pub fn set_pitch(&mut self, pitch: f64) {
        self.view.set_pitch(&self.route, pitch);
    }

    /// The camera's tilt from straight down, in degrees.
    #[wasm_bindgen(getter, js_name = pitchDeg)]
    pub fn pitch_deg(&self) -> f64 {
        self.view.pitch_deg()
    }

    /// Bring the framing camera `levels` zoom levels closer to the route's
    /// middle (negative backs off), clamped to `-1..=4`; `0` fits the route.
    #[wasm_bindgen(js_name = setFramingZoom)]
    pub fn set_framing_zoom(&mut self, levels: f64) {
        self.view.set_framing_zoom(levels);
    }

    /// Zoom levels the framing camera is in from fitting the route.
    #[wasm_bindgen(getter, js_name = framingZoom)]
    pub fn framing_zoom(&self) -> f64 {
        self.view.framing_zoom()
    }
}

impl SceneRenderer {
    /// The GL camera at `turn` along the orbit and progress `u`; see
    /// [`CameraRig::gl_camera_at`].
    pub(super) fn gl_camera_at(&self, turn: f64, u: f64) -> Camera {
        self.view
            .gl_camera_at(&self.route, &self.style, &self.terrain, turn, u)
    }

    /// The camera at `turn` along the orbit and progress `u`; see
    /// [`CameraRig::view_at`].
    #[cfg(test)]
    pub(super) fn view_at(&self, turn: f64, u: f64) -> Camera {
        self.view
            .view_at(&self.route, &self.style, &self.terrain, turn, u)
    }
}

/// `camera` backed off, toward where it looks from, until it is
/// `clearance` world pixels above the ground under it.
fn clear_of(terrain: &Terrain, camera: Camera, clearance: f64) -> Camera {
    camera.clear_of_terrain(
        |x, y| terrain.height_at_world(x, y) + clearance,
        MAX_CLEARANCE_STEPS,
    )
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

#[cfg(test)]
mod tests {
    use super::super::test_support::{namsan, with_terrain};
    use super::*;

    /// The Namsan scene's route, style, and flat camera rig.
    fn rig() -> (Route, FrameStyle, CameraRig) {
        let scene = namsan(false);
        (scene.route, scene.style, scene.view)
    }

    #[test]
    fn the_start_target_is_the_first_framing_aim() {
        let (route, _, rig) = rig();
        assert_eq!(rig.start_target(), rig.camera_at(&route, 0.0).target());
        assert_eq!(rig.framing_zoom(), 0.0);
        assert_eq!(rig.follow(), None);
    }

    #[test]
    fn the_orbit_turns_the_bearing_within_its_sweep() {
        let (_, _, rig) = rig();
        assert_eq!(rig.bearing_at(0.0), 20.0);
        assert_eq!(rig.bearing_at(0.5), 200.0);
        assert_eq!(rig.bearing_at(-1.0), 20.0);
        assert_eq!(rig.bearing_at(2.0), 380.0);
    }

    #[test]
    fn the_framing_zoom_brings_the_camera_in_by_powers_of_two() {
        let (route, _, mut rig) = rig();
        let fitted = rig.camera_at(&route, 0.3);
        rig.set_framing_zoom(2.0);
        let close = rig.camera_at(&route, 0.3);
        assert!((close.distance - fitted.distance / 4.0).abs() < 1e-9 * fitted.distance);
        assert_eq!(close.detail_zoom, fitted.detail_zoom + 2.0);
        assert_eq!(close.target(), fitted.target());
        rig.set_framing_zoom(10.0);
        assert_eq!(rig.framing_zoom(), FRAMING_ZOOM.1);
        rig.set_framing_zoom(f64::NAN);
        assert_eq!(rig.framing_zoom(), FRAMING_ZOOM.1);
        rig.set_framing_zoom(-5.0);
        assert_eq!(rig.framing_zoom(), FRAMING_ZOOM.0);
    }

    #[test]
    fn follow_zooms_stay_between_the_fit_and_the_finest() {
        let (_, _, mut rig) = rig();
        let fitted = rig.fitted().zoom;
        rig.set_follow(Some(30.0));
        assert_eq!(rig.follow(), Some(MAX_FOLLOW_ZOOM));
        rig.set_follow(Some(0.0));
        assert_eq!(rig.follow(), Some(fitted));
        rig.set_follow(Some(f64::INFINITY));
        assert_eq!(rig.follow(), None);
    }

    #[test]
    fn a_new_pitch_refits_at_the_same_world_zoom() {
        let (route, _, mut rig) = rig();
        let before = *rig.fitted();
        let start = rig.start_target();
        rig.set_pitch(&route, 20.0);
        assert_eq!(rig.pitch_deg(), 20.0);
        assert_eq!(rig.fitted().zoom, before.zoom);
        assert_ne!(rig.fitted().distance, before.distance);
        assert_eq!(rig.start_target(), start, "the origin stays");
        rig.set_pitch(&route, 90.0);
        assert_eq!(rig.pitch_deg(), MAX_PITCH_DEG);
        rig.set_pitch(&route, f64::NAN);
        assert_eq!(rig.pitch_deg(), MAX_PITCH_DEG);
    }

    #[test]
    fn a_framing_view_is_the_orbit_camera_without_terrain() {
        let (route, style, mut rig) = rig();
        let flat = TerrainState::flat();
        for turn in [0.0, 0.4] {
            let camera = rig.camera_at(&route, turn);
            assert_eq!(rig.view_at(&route, &style, &flat, turn, 0.7), camera);
            assert_eq!(rig.gl_camera_at(&route, &style, &flat, turn, 0.7), camera);
        }
        rig.set_follow(Some(rig.fitted().zoom + 2.0));
        let a = rig.view_at(&route, &style, &flat, 0.0, 0.2);
        let b = rig.view_at(&route, &style, &flat, 0.0, 0.8);
        assert_ne!(a.target(), b.target(), "following moves with the marker");
        assert_eq!(a.detail_zoom, rig.fitted().zoom + 2.0);
    }

    #[test]
    fn clearing_terrain_backs_the_camera_off_above_it() {
        let scene = with_terrain();
        let Some(terrain) = scene.terrain.built() else {
            panic!("terrain is built");
        };
        let mut camera = scene.view.camera_at(&scene.route, 0.0);
        camera.distance /= 64.0;
        let clearance = 50.0;
        let cleared = clear_of(terrain, camera, clearance);
        let (ex, ey, eh) = cleared.eye();
        assert!(eh >= terrain.height_at_world(ex, ey) + clearance - 1e-6);
        assert!(cleared.distance >= camera.distance);
        assert_eq!(cleared.target(), camera.target());
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
}
