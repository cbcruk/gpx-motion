//! A pinhole camera orbiting a ground target: construction, fitting the
//! route into view, and turning, tilting and backing off.

mod projection;
mod tiles;

pub use projection::{mat_mul, Mat3, Projector, ViewPoint, NEAR};

use super::mercator::{lat_to_pixel, lon_to_pixel, meters_per_pixel};
use crate::gpx::model::TrackPoint;

/// Steepest camera tilt from straight down, in degrees; pitches are clamped
/// to `0..=MAX_PITCH_DEG`.
pub const MAX_PITCH_DEG: f64 = 75.0;

/// Ground beyond this many camera distances is not drawn (fog hides it).
const FAR_FACTOR: f64 = 4.0;

/// A pinhole camera orbiting a ground target in Web Mercator world pixels.
///
/// World coordinates are global pixels at `zoom` (x east, y south) plus a
/// height `h` in the same pixel units. With `pitch = 0` and the default
/// distance the camera looks straight down and reproduces the flat 2D
/// [`Projection`](super::Projection) one-to-one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub zoom: f64,
    target_x: f64,
    target_y: f64,
    bearing: f64,
    pitch: f64,
    /// Distance from the camera to the target, in world pixels.
    pub distance: f64,
    /// Zoom whose map detail (tile zoom, line widths) the view draws with:
    /// `zoom` itself, except mid-way through [`Camera::zoom_between`], where
    /// the world stays at one zoom while the view zooms.
    pub detail_zoom: f64,
    focal: f64,
    width: f64,
    height: f64,
}

/// Camera orientation and field of view, in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lens {
    /// Tilt from straight down (`0`) toward the horizon, clamped to
    /// `0..=`[`MAX_PITCH_DEG`].
    pub pitch_deg: f64,
    /// Compass direction that points up the screen.
    pub bearing_deg: f64,
    /// Vertical field of view, clamped to `10..=120`.
    pub fov_deg: f64,
}

impl Default for Lens {
    fn default() -> Self {
        Self {
            pitch_deg: 55.0,
            bearing_deg: 0.0,
            fov_deg: 40.0,
        }
    }
}

/// The usable screen rectangle inside padding and a bottom inset.
struct ScreenBounds {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

impl ScreenBounds {
    fn new(width: u32, height: u32, padding: f64, bottom_inset: f64) -> Self {
        Self {
            left: padding,
            right: width as f64 - padding,
            top: padding,
            bottom: height as f64 - padding - bottom_inset,
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
}

impl Camera {
    /// A camera aimed at a coordinate.
    ///
    /// The distance starts at the focal length, i.e. the 2D scale of `zoom`.
    pub fn new(lat: f64, lon: f64, zoom: f64, width: u32, height: u32, lens: Lens) -> Self {
        let fov = lens.fov_deg.clamp(10.0, 120.0).to_radians();
        let focal = (height as f64 / 2.0) / (fov / 2.0).tan();
        Self {
            zoom,
            target_x: lon_to_pixel(lon, zoom),
            target_y: lat_to_pixel(lat, zoom),
            bearing: lens.bearing_deg.to_radians(),
            pitch: lens.pitch_deg.clamp(0.0, MAX_PITCH_DEG).to_radians(),
            distance: focal,
            detail_zoom: zoom,
            focal,
            width: width as f64,
            height: height as f64,
        }
    }

    /// Aim at the center of `points` and pick the smallest distance that keeps
    /// every point (ground and raised by `heights_m`) inside the viewport,
    /// leaving `padding` pixels on each side and `bottom_inset` at the bottom.
    ///
    /// `heights_m` holds one height per point in meters (empty for flat).
    /// Returns `None` when `points` is empty.
    pub fn fit(
        points: &[TrackPoint],
        heights_m: &[f64],
        width: u32,
        height: u32,
        padding: f64,
        bottom_inset: f64,
        lens: Lens,
    ) -> Option<Self> {
        let first = points.first()?;
        let (mut min_lat, mut max_lat, mut min_lon, mut max_lon) =
            (first.lat, first.lat, first.lon, first.lon);
        for p in points {
            min_lat = min_lat.min(p.lat);
            max_lat = max_lat.max(p.lat);
            min_lon = min_lon.min(p.lon);
            max_lon = max_lon.max(p.lon);
        }
        let zoom = super::Projection::fit(points, width, height, padding)?.zoom;
        let center_lat = (min_lat + max_lat) / 2.0;
        let center_lon = (min_lon + max_lon) / 2.0;
        let mut camera = Self::new(center_lat, center_lon, zoom, width, height, lens);

        let world = camera.world_points(points, heights_m);
        let bounds = ScreenBounds::new(width, height, padding, bottom_inset);

        camera.settle(&world, &bounds, true);
        Some(camera)
    }

    /// Smallest distance at which this camera (keeping its target and
    /// bearing) shows every point, ground and raised by `heights_m`, inside the
    /// viewport with `padding` and `bottom_inset`.
    pub fn distance_to_fit(
        &self,
        points: &[TrackPoint],
        heights_m: &[f64],
        padding: f64,
        bottom_inset: f64,
    ) -> f64 {
        let world = self.world_points(points, heights_m);
        self.min_distance(&world, &self.bounds(padding, bottom_inset))
    }

    /// Back the camera off until the route fits at every bearing an orbit of
    /// `orbit_deg` (from the current bearing) passes through, each bearing
    /// re-aimed with [`Camera::aim`].
    ///
    /// Use the result as the orbit's fixed distance and call `aim` per frame,
    /// so the route stays centered without the zoom pumping.
    pub fn widen_for_orbit(
        mut self,
        points: &[TrackPoint],
        heights_m: &[f64],
        padding: f64,
        bottom_inset: f64,
        orbit_deg: f64,
    ) -> Self {
        let world = self.world_points(points, heights_m);
        let bounds = self.bounds(padding, bottom_inset);
        let start = self.bearing.to_degrees();
        let steps = (orbit_deg.abs() / 15.0).ceil() as usize;
        for k in 0..=steps {
            let t = if steps == 0 {
                0.0
            } else {
                k as f64 / steps as f64
            };
            let mut turned = self.with_bearing(start + orbit_deg * t);
            turned.settle(&world, &bounds, true);
            self.distance = self.distance.max(turned.distance);
        }
        self
    }

    /// The camera turned to face `bearing_deg` and refitted from scratch with
    /// [`Camera::widen_for_orbit`], so the route fits along an `orbit_deg` turn
    /// from there, however near or far the camera was before.
    pub fn refit_for_orbit(
        self,
        bearing_deg: f64,
        points: &[TrackPoint],
        heights_m: &[f64],
        padding: f64,
        bottom_inset: f64,
        orbit_deg: f64,
    ) -> Self {
        let mut start = self.with_bearing(bearing_deg);
        start.distance = 0.0;
        start.widen_for_orbit(points, heights_m, padding, bottom_inset, orbit_deg)
    }

    /// The same camera with its target shifted so the route sits centered on
    /// screen at the current bearing and distance.
    pub fn aim(
        mut self,
        points: &[TrackPoint],
        heights_m: &[f64],
        padding: f64,
        bottom_inset: f64,
    ) -> Self {
        let world = self.world_points(points, heights_m);
        let bounds = self.bounds(padding, bottom_inset);
        self.settle(&world, &bounds, false);
        self
    }

    fn bounds(&self, padding: f64, bottom_inset: f64) -> ScreenBounds {
        ScreenBounds::new(self.width as u32, self.height as u32, padding, bottom_inset)
    }

    /// Perspective makes the near side loom larger, so the bbox center is
    /// off-center on screen: shift the target toward the projected center
    /// until it settles, refitting the distance each step when `refit`.
    fn settle(&mut self, world: &[(f64, f64, f64)], bounds: &ScreenBounds, refit: bool) {
        if refit {
            self.distance = self.min_distance(world, bounds);
        }
        for _ in 0..4 {
            let Some((sx0, sy0, sx1, sy1)) = self.screen_bounds(world) else {
                break;
            };
            let off_x = (sx0 + sx1) / 2.0 - (bounds.left + bounds.right) / 2.0;
            let off_y = (sy0 + sy1) / 2.0 - (bounds.top + bounds.bottom) / 2.0;
            let scale = self.distance / self.focal;
            let rx = off_x * scale;
            let ry = off_y * scale / self.pitch.cos().max(0.2);
            let (sin_b, cos_b) = self.bearing.sin_cos();
            self.target_x += cos_b * rx - sin_b * ry;
            self.target_y += sin_b * rx + cos_b * ry;
            if refit {
                self.distance = self.min_distance(world, bounds);
            }
        }
    }

    fn world_points(&self, points: &[TrackPoint], heights_m: &[f64]) -> Vec<(f64, f64, f64)> {
        points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let h = heights_m
                    .get(i)
                    .map_or(0.0, |m| self.meters_to_px(*m, p.lat));
                (
                    lon_to_pixel(p.lon, self.zoom),
                    lat_to_pixel(p.lat, self.zoom),
                    h,
                )
            })
            .collect()
    }

    fn min_distance(&self, world: &[(f64, f64, f64)], bounds: &ScreenBounds) -> f64 {
        let mut cam = *self;
        let (mut lo, mut hi) = (self.focal * 0.05, self.focal * 50.0);
        for _ in 0..48 {
            cam.distance = (lo + hi) / 2.0;
            let fits = world.iter().all(|&(x, y, h)| {
                [0.0, h].iter().all(|&z| {
                    cam.project_world(x, y, z)
                        .is_some_and(|(sx, sy)| bounds.contains(sx, sy))
                })
            });
            if fits {
                hi = cam.distance;
            } else {
                lo = cam.distance;
            }
        }
        hi
    }

    /// Screen bounding box of world points, both on the ground and raised.
    fn screen_bounds(&self, world: &[(f64, f64, f64)]) -> Option<(f64, f64, f64, f64)> {
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for &(x, y, h) in world {
            for z in [0.0, h] {
                let (sx, sy) = self.project_world(x, y, z)?;
                bounds = Some(match bounds {
                    None => (sx, sy, sx, sy),
                    Some((a, b, c, d)) => (a.min(sx), b.min(sy), c.max(sx), d.max(sy)),
                });
            }
        }
        bounds
    }

    /// The same camera tilted `pitch_deg` from straight down (clamped to
    /// `0..=`[`MAX_PITCH_DEG`], as [`Lens::pitch_deg`]), keeping its target
    /// and distance.
    pub fn with_pitch(mut self, pitch_deg: f64) -> Self {
        self.pitch = pitch_deg.clamp(0.0, MAX_PITCH_DEG).to_radians();
        self
    }

    /// The same camera with a different bearing (degrees).
    pub fn with_bearing(mut self, bearing_deg: f64) -> Self {
        self.bearing = bearing_deg.to_radians();
        self
    }

    /// The same camera re-aimed at a coordinate.
    pub fn with_target(mut self, lat: f64, lon: f64) -> Self {
        self.target_x = lon_to_pixel(lon, self.zoom);
        self.target_y = lat_to_pixel(lat, self.zoom);
        self
    }

    /// The same camera aimed so a point `h_px` world pixels above a
    /// coordinate lands at the screen center: the target slides forward
    /// along the view to where the line of sight through that point meets
    /// the ground.
    pub fn with_raised_target(self, lat: f64, lon: f64, h_px: f64) -> Self {
        let mut camera = self.with_target(lat, lon);
        let shift = h_px * camera.pitch.tan();
        let (sin_b, cos_b) = camera.bearing.sin_cos();
        camera.target_x += shift * sin_b;
        camera.target_y -= shift * cos_b;
        camera
    }

    /// The view `s` of the way (`0..=1`) from `self` to `other`, in
    /// `other`'s world zoom (so terrain built at that zoom still fits).
    ///
    /// The distance moves geometrically and the target so that the ground
    /// zooms about the point both views keep at the screen center; bearing
    /// and pitch turn the short way, and the [`Camera::detail_zoom`] moves
    /// linearly, so the map detail matches each end. `0` (or NaN) gives
    /// `self` rescaled to `other`'s zoom, the same view; `1` gives `other`.
    /// The pivot stays fixed only when both share bearing and pitch.
    pub fn zoom_between(&self, other: &Camera, s: f64) -> Camera {
        let s = if s.is_nan() { 0.0 } else { s.clamp(0.0, 1.0) };
        if s == 1.0 {
            return *other;
        }
        let scale = 2f64.powf(other.zoom - self.zoom);
        let (ax, ay, ad) = (
            self.target_x * scale,
            self.target_y * scale,
            self.distance * scale,
        );
        let total = other.distance / ad;
        let ratio = total.powf(s);
        let w = super::pivot_weight(ratio, total, s);
        let turn = (other.bearing - self.bearing + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        Camera {
            target_x: ax + (other.target_x - ax) * w,
            target_y: ay + (other.target_y - ay) * w,
            distance: ad * ratio,
            bearing: other.bearing - turn * (1.0 - s),
            pitch: self.pitch + (other.pitch - self.pitch) * s,
            detail_zoom: self.detail_zoom + (other.detail_zoom - self.detail_zoom) * s,
            ..*other
        }
    }

    /// The same camera backed off until it is at least `h_px` world pixels
    /// above the ground plane (e.g. clear of the terrain).
    pub fn with_min_altitude(mut self, h_px: f64) -> Self {
        let cos_p = self.pitch.cos();
        if cos_p > f64::EPSILON {
            self.distance = self.distance.max(h_px / cos_p);
        }
        self
    }

    /// The same camera backed off, toward where it looks from, so its eye
    /// clears `min_altitude(x, y)` world pixels above the ground plane,
    /// where `(x, y)` is the eye's world position (e.g. the terrain height
    /// there plus a margin).
    ///
    /// Backing off moves the eye over other ground, so this repeats, at most
    /// `max_steps` times, stopping early once the distance no longer grows;
    /// over steep ground the eye may still sit slightly low after the last
    /// step.
    pub fn clear_of_terrain(
        mut self,
        min_altitude: impl Fn(f64, f64) -> f64,
        max_steps: usize,
    ) -> Self {
        for _ in 0..max_steps {
            let (x, y, _) = self.eye();
            let before = self.distance;
            self = self.with_min_altitude(min_altitude(x, y));
            if self.distance - before <= before * 1e-9 {
                break;
            }
        }
        self
    }

    /// World pixels per meter of height at a latitude.
    pub fn meters_to_px(&self, meters: f64, lat: f64) -> f64 {
        meters / meters_per_pixel(lat, self.zoom)
    }

    /// The ground point the camera looks at, in world pixels `(x, y)`.
    pub fn target(&self) -> (f64, f64) {
        (self.target_x, self.target_y)
    }

    /// Compass direction up the screen, in degrees.
    pub fn bearing_deg(&self) -> f64 {
        self.bearing.to_degrees()
    }

    /// Tilt from straight down, in degrees.
    pub fn pitch_deg(&self) -> f64 {
        self.pitch.to_degrees()
    }

    /// Focal length in screen pixels: how many pixels a world pixel at view
    /// depth `d` spans is `focal / d`.
    pub fn focal(&self) -> f64 {
        self.focal
    }

    /// Viewport size in pixels `(width, height)`.
    pub fn viewport(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    /// Farthest view depth whose ground the 3D renderers draw (fog hides
    /// what lies beyond).
    pub fn far_depth(&self) -> f64 {
        FAR_FACTOR * self.distance
    }

    /// Vertical field of view in degrees.
    pub fn fov_deg(&self) -> f64 {
        (2.0 * (self.height / 2.0 / self.focal).atan()).to_degrees()
    }

    /// The camera's own position in world pixels `(x, y, h)`.
    pub fn eye(&self) -> (f64, f64, f64) {
        let (sin_b, cos_b) = self.bearing.sin_cos();
        let (sin_p, cos_p) = self.pitch.sin_cos();
        let back = self.distance * sin_p;
        (
            self.target_x - sin_b * back,
            self.target_y + cos_b * back,
            self.distance * cos_p,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn lens(pitch_deg: f64, bearing_deg: f64) -> Lens {
        Lens {
            pitch_deg,
            bearing_deg,
            fov_deg: 40.0,
        }
    }

    #[test]
    fn zoom_between_keeps_its_ends_and_the_ground_pivot() {
        let wide = Camera::new(37.55, 126.98, 12.0, 640, 480, lens(55.0, 20.0));
        let wide = Camera {
            distance: wide.distance * 3.0,
            ..wide
        };
        let close = Camera::new(37.56, 127.0, 15.0, 640, 480, lens(55.0, 20.0));
        assert_eq!(wide.zoom_between(&close, 1.0), close);
        let start = wide.zoom_between(&close, 0.0);
        let (lat, lon) = (37.551, 126.99);
        let a = wide.project(lat, lon, 0.0).unwrap();
        let b = start.project(lat, lon, 0.0).unwrap();
        assert!((a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6);
        // Views related by a zoom share the ground point their targets scale
        // about; it must stay at one screen point throughout.
        let k = close.distance / start.distance;
        let p = (
            (close.target_x - start.target_x * k) / (1.0 - k),
            (close.target_y - start.target_y * k) / (1.0 - k),
        );
        let screen = start.project_world(p.0, p.1, 0.0);
        for s in [0.2, 0.5, 0.8] {
            let mid = wide.zoom_between(&close, s);
            assert_eq!(mid.zoom, close.zoom);
            let (x, y) = mid.project_world(p.0, p.1, 0.0).unwrap();
            let (ex, ey) = screen.unwrap();
            assert!((x - ex).abs() < 1e-6 && (y - ey).abs() < 1e-6, "s {s}");
            assert!(mid.distance < start.distance && mid.distance > close.distance);
            assert!(mid.detail_zoom > wide.zoom && mid.detail_zoom < close.zoom);
        }
        assert_eq!(start.detail_zoom, wide.zoom);
        assert_eq!(start.tile_zoom(), wide.tile_zoom());
        assert_eq!(wide.zoom_between(&close, f64::NAN), start);
    }

    #[test]
    fn zoom_between_turns_the_short_way_to_the_exact_end() {
        let a = Camera::new(37.55, 126.98, 12.0, 640, 480, lens(55.0, 350.0));
        let b = Camera::new(37.56, 127.0, 15.0, 640, 480, lens(40.0, 10.0));
        assert_eq!(a.zoom_between(&b, 1.0), b);
        let mid = a.zoom_between(&b, 0.5);
        assert!(mid.bearing.to_degrees().rem_euclid(360.0).abs() < 1e-9);
        assert!((mid.pitch.to_degrees() - 47.5).abs() < 1e-9);
    }

    #[test]
    fn orbit_widening_keeps_every_bearing_in_view() {
        let points = [
            TrackPoint::new(37.540, 126.970),
            TrackPoint::new(37.548, 126.995),
        ];
        let cam = Camera::fit(&points, &[], 640, 360, 20.0, 0.0, lens(55.0, 0.0)).unwrap();
        let wide = cam.widen_for_orbit(&points, &[], 20.0, 0.0, 90.0);
        assert!(wide.distance >= cam.distance);
        for bearing in [0.0, 30.0, 60.0, 90.0] {
            let aimed = wide.with_bearing(bearing).aim(&points, &[], 20.0, 0.0);
            let needed = aimed.distance_to_fit(&points, &[], 20.0, 0.0);
            assert!(wide.distance >= needed * 0.99, "bearing {bearing}");
        }
    }

    #[test]
    fn orbit_refit_ignores_the_previous_distance() {
        let points = [
            TrackPoint::new(37.540, 126.970),
            TrackPoint::new(37.548, 126.995),
        ];
        let cam = Camera::fit(&points, &[], 640, 360, 20.0, 0.0, lens(55.0, 0.0)).unwrap();
        let far = Camera {
            distance: cam.distance * 10.0,
            ..cam
        };
        let refit = |c: Camera| c.refit_for_orbit(30.0, &points, &[], 20.0, 0.0, 90.0);
        assert_eq!(refit(cam), refit(far));
        assert!((refit(cam).bearing_deg() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn clearing_terrain_lifts_the_eye_over_the_ground_under_it() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(55.0, 0.0));
        let ground = |x: f64, _: f64| 1000.0 + x * 1e-4;
        let cleared = cam.clear_of_terrain(ground, 8);
        let (x, y, h) = cleared.eye();
        assert!(cleared.distance > cam.distance);
        assert!(h >= ground(x, y) * (1.0 - 1e-6), "{h} vs {}", ground(x, y));
        assert_eq!(cam.clear_of_terrain(|_, _| 0.0, 8), cam);
    }

    #[test]
    fn raised_target_lands_at_screen_center() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(55.0, 120.0))
            .with_raised_target(37.51, 127.01, 300.0);
        let (x, y) = cam.project(37.51, 127.01, 300.0).unwrap();
        assert!(
            (x - 320.0).abs() < 1e-6 && (y - 240.0).abs() < 1e-6,
            "{x},{y}"
        );
    }

    #[test]
    fn fit_keeps_ground_and_raised_points_inside() {
        let pts = vec![
            TrackPoint::new(37.5600, 126.9800),
            TrackPoint::new(37.5520, 126.9890),
            TrackPoint::new(37.5560, 126.9750),
        ];
        let heights = [30.0, 240.0, 60.0];
        let cam = Camera::fit(&pts, &heights, 800, 600, 20.0, 100.0, lens(55.0, 30.0)).unwrap();
        for (p, h) in pts.iter().zip(heights) {
            for hp in [0.0, cam.meters_to_px(h, p.lat)] {
                let (x, y) = cam.project(p.lat, p.lon, hp).unwrap();
                assert!((19.9..=780.1).contains(&x), "x={x}");
                assert!((19.9..=480.1).contains(&y), "y={y}");
            }
        }
    }

    #[test]
    fn fit_centers_the_projected_track() {
        let pts = vec![
            TrackPoint::new(37.5600, 126.9800),
            TrackPoint::new(37.5520, 126.9890),
            TrackPoint::new(37.5560, 126.9750),
        ];
        let heights = [30.0, 240.0, 60.0];
        let cam = Camera::fit(&pts, &heights, 800, 600, 20.0, 0.0, lens(60.0, 30.0)).unwrap();
        let world: Vec<_> = pts
            .iter()
            .zip(heights)
            .map(|(p, h)| {
                (
                    lon_to_pixel(p.lon, cam.zoom),
                    lat_to_pixel(p.lat, cam.zoom),
                    cam.meters_to_px(h, p.lat),
                )
            })
            .collect();
        let (x0, y0, x1, y1) = cam.screen_bounds(&world).unwrap();
        assert!(
            ((x0 + x1) / 2.0 - 400.0).abs() < 8.0,
            "x center {}",
            (x0 + x1) / 2.0
        );
        assert!(
            ((y0 + y1) / 2.0 - 300.0).abs() < 8.0,
            "y center {}",
            (y0 + y1) / 2.0
        );
    }
}
