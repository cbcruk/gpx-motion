use super::mercator::{lat_to_pixel, lon_to_pixel, TILE_SIZE};
use crate::gpx::model::TrackPoint;
use crate::tiles::{TileId, TilePlacement};

/// A 3×3 matrix acting on homogeneous 2D points, row-major.
pub type Mat3 = [[f64; 3]; 3];

/// `m · p` for a homogeneous point `p`.
fn mul(m: &Mat3, p: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2])
}

/// Matrix product `a · b`.
pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    [0, 1, 2].map(|r| [0, 1, 2].map(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

/// Closest view depth (world px) that is still drawn; geometry nearer than
/// this is clipped rather than projected.
pub const NEAR: f64 = 1.0;

/// Ground beyond this many camera distances is not drawn (fog hides it).
const FAR_FACTOR: f64 = 4.0;
/// Upper bound on tiles per 3D view; farther views drop to coarser zooms.
const MAX_GROUND_TILES: usize = 64;

/// Equatorial meters per world pixel at zoom 0.
const METERS_PER_PX_Z0: f64 = 156_543.033_928;

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
    /// Tilt from straight down (`0`) toward the horizon, clamped to `0..=75`.
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

/// A point in camera space: screen-aligned `x`/`y` and forward `depth`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewPoint {
    pub x: f64,
    pub y: f64,
    pub depth: f64,
}

impl ViewPoint {
    /// Linear interpolation toward `other`.
    pub fn lerp(self, other: ViewPoint, t: f64) -> ViewPoint {
        ViewPoint {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
            depth: self.depth + (other.depth - self.depth) * t,
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
            pitch: lens.pitch_deg.clamp(0.0, 75.0).to_radians(),
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

    /// The same camera with a different bearing (degrees).
    pub fn with_bearing(mut self, bearing_deg: f64) -> Self {
        self.bearing = bearing_deg.to_radians();
        self
    }

    /// The same view on a canvas `margin_px` larger on every side: same focal
    /// length and center, so it shows everything this camera does, shifted by
    /// `margin_px`, plus a border around it.
    pub fn overscanned(mut self, margin_px: f64) -> Self {
        self.width += 2.0 * margin_px;
        self.height += 2.0 * margin_px;
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

    /// World pixels per meter of height at a latitude.
    pub fn meters_to_px(&self, meters: f64, lat: f64) -> f64 {
        let m_per_px = METERS_PER_PX_Z0 * lat.to_radians().cos() / 2f64.powf(self.zoom);
        meters / m_per_px
    }

    /// The ground point the camera looks at, in world pixels `(x, y)`.
    pub fn target(&self) -> (f64, f64) {
        (self.target_x, self.target_y)
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

    /// Camera-space position of a world point.
    pub fn view(&self, x: f64, y: f64, h: f64) -> ViewPoint {
        let (dx, dy) = (x - self.target_x, y - self.target_y);
        let (sin_b, cos_b) = self.bearing.sin_cos();
        let rx = cos_b * dx + sin_b * dy;
        let ry = -sin_b * dx + cos_b * dy;
        let (sin_p, cos_p) = self.pitch.sin_cos();
        ViewPoint {
            x: rx,
            y: ry * cos_p - h * sin_p,
            depth: self.distance - ry * sin_p - h * cos_p,
        }
    }

    /// Screen position of a camera-space point (call only with `depth >= NEAR`).
    pub fn to_screen(&self, v: ViewPoint) -> (f64, f64) {
        (
            self.width / 2.0 + self.focal * v.x / v.depth,
            self.height / 2.0 + self.focal * v.y / v.depth,
        )
    }

    /// Screen position of a world point, or `None` behind the near plane.
    pub fn project_world(&self, x: f64, y: f64, h: f64) -> Option<(f64, f64)> {
        let v = self.view(x, y, h);
        (v.depth >= NEAR).then(|| self.to_screen(v))
    }

    /// Screen position of a coordinate raised `h_px` world pixels.
    pub fn project(&self, lat: f64, lon: f64, h_px: f64) -> Option<(f64, f64)> {
        self.project_world(
            lon_to_pixel(lon, self.zoom),
            lat_to_pixel(lat, self.zoom),
            h_px,
        )
    }

    /// World point on the ground plane seen at screen position `(sx, sy)`, or
    /// `None` at or above the horizon (or nearer than the near plane).
    ///
    /// The inverse of [`Camera::project_world`] with `h = 0`.
    pub fn ground_at(&self, sx: f64, sy: f64) -> Option<(f64, f64)> {
        let [x, y, w] = mul(&self.screen_to_ground(), [sx, sy, 1.0]);
        (w > f64::EPSILON && w <= self.near_ground_weight()).then(|| (x / w, y / w))
    }

    /// Largest homogeneous weight `w` from [`Camera::screen_to_ground`] that
    /// is still at or beyond the near plane.
    pub fn near_ground_weight(&self) -> f64 {
        self.distance * self.pitch.cos() / NEAR
    }

    /// Homogeneous map from screen pixels to world pixels on the ground
    /// plane. The third component `w` is positive below the horizon, and the
    /// view depth there is `distance · cos(pitch) / w`.
    pub fn screen_to_ground(&self) -> Mat3 {
        let (sin_p, cos_p) = self.pitch.sin_cos();
        let (sin_b, cos_b) = self.bearing.sin_cos();
        let (cx, cy, f, d) = (
            self.width / 2.0,
            self.height / 2.0,
            self.focal,
            self.distance,
        );
        // (u, v) = ((sx - cx) / f, (sy - cy) / f); the ground point in view
        // axes is (u·d·cos p, v·d) / (cos p + v·sin p).
        let view = [
            [d * cos_p / f, 0.0, -cx * d * cos_p / f],
            [0.0, d / f, -cy * d / f],
            [0.0, sin_p / f, cos_p - cy * sin_p / f],
        ];
        let to_world = [
            [cos_b, -sin_b, self.target_x],
            [sin_b, cos_b, self.target_y],
            [0.0, 0.0, 1.0],
        ];
        mat_mul(&to_world, &view)
    }

    /// Homogeneous map from world pixels on the ground plane to screen
    /// pixels. The third component is the view depth.
    pub fn ground_to_screen(&self) -> Mat3 {
        let (sin_p, cos_p) = self.pitch.sin_cos();
        let (sin_b, cos_b) = self.bearing.sin_cos();
        let (cx, cy, f, d) = (
            self.width / 2.0,
            self.height / 2.0,
            self.focal,
            self.distance,
        );
        let to_view = [
            [
                cos_b,
                sin_b,
                -(cos_b * self.target_x + sin_b * self.target_y),
            ],
            [-sin_b, cos_b, sin_b * self.target_x - cos_b * self.target_y],
            [0.0, 0.0, 1.0],
        ];
        // depth = d − ry·sin p; screen = (cx + f·rx/depth, cy + f·ry·cos p/depth).
        let project = [
            [f, -cx * sin_p, cx * d],
            [0.0, f * cos_p - cy * sin_p, cy * d],
            [0.0, -sin_p, d],
        ];
        mat_mul(&project, &to_view)
    }

    /// Screen y of the horizon (may lie above the viewport).
    pub fn horizon_y(&self) -> f64 {
        let (sin_p, cos_p) = self.pitch.sin_cos();
        if sin_p <= f64::EPSILON {
            return f64::NEG_INFINITY;
        }
        self.height / 2.0 - self.focal * cos_p / sin_p
    }

    /// The whole tile zoom this camera draws with; see
    /// [`super::mercator::tile_zoom_for`].
    pub fn tile_zoom(&self) -> u32 {
        super::mercator::tile_zoom_for(self.detail_zoom)
    }

    /// Tiles for the ground this camera shows, with the far cutoff and tile
    /// budget the 3D renderers share; see [`Camera::visible_tiles`].
    pub fn ground_tiles(&self, max_zoom: u32) -> Vec<TilePlacement> {
        self.visible_tiles(max_zoom, FAR_FACTOR, MAX_GROUND_TILES)
    }

    /// Tiles for the ground this camera shows at exactly `tile_zoom` (capped
    /// to [`Camera::tile_zoom`]), with no tile budget. Lets an overscanned view keep
    /// the detail its unpadded camera picked with [`Camera::ground_tiles`].
    pub fn ground_tiles_at(&self, tile_zoom: u32) -> Vec<TilePlacement> {
        self.visible_tiles(tile_zoom, FAR_FACTOR, usize::MAX)
    }

    /// Tiles covering the visible ground, as world-pixel placements (`dx`/`dy`
    /// are the tile's world origin at the camera zoom, not screen offsets).
    ///
    /// Ground beyond `far_factor × distance` is ignored. The tile zoom starts
    /// at `max_zoom` (capped to [`Camera::tile_zoom`]) and drops until at most
    /// `max_tiles` cover the footprint.
    pub fn visible_tiles(
        &self,
        max_zoom: u32,
        far_factor: f64,
        max_tiles: usize,
    ) -> Vec<TilePlacement> {
        let (min_x, min_y, max_x, max_y) = self.footprint(far_factor);
        let mut tile_zoom = max_zoom.min(self.tile_zoom());
        loop {
            let size = TILE_SIZE * 2f64.powf(self.zoom - tile_zoom as f64);
            let (x0, x1) = ((min_x / size).floor() as i64, (max_x / size).floor() as i64);
            let (y0, y1) = ((min_y / size).floor() as i64, (max_y / size).floor() as i64);
            let count = ((x1 - x0 + 1) * (y1 - y0 + 1)).max(0) as usize;
            if count <= max_tiles || tile_zoom == 0 {
                let n = 2i64.pow(tile_zoom);
                let mut tiles = Vec::with_capacity(count);
                for ty in y0.max(0)..=y1.min(n - 1) {
                    for tx in x0.max(0)..=x1.min(n - 1) {
                        tiles.push(TilePlacement {
                            id: TileId {
                                zoom: tile_zoom,
                                x: tx as u32,
                                y: ty as u32,
                            },
                            dx: tx as f64 * size,
                            dy: ty as f64 * size,
                            size,
                        });
                    }
                }
                return tiles;
            }
            tile_zoom -= 1;
        }
    }

    /// World-pixel bounding box of the ground seen through the viewport edges.
    fn footprint(&self, far_factor: f64) -> (f64, f64, f64, f64) {
        let far = self.distance * far_factor;
        let (sin_p, cos_p) = self.pitch.sin_cos();
        let (sin_b, cos_b) = self.bearing.sin_cos();
        let mut bounds = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );

        const STEPS: usize = 8;
        for k in 0..=STEPS {
            let t = k as f64 / STEPS as f64;
            for (sx, sy) in [
                (t * self.width, 0.0),
                (t * self.width, self.height),
                (0.0, t * self.height),
                (self.width, t * self.height),
            ] {
                let vx = (sx - self.width / 2.0) / self.focal;
                let vy = (sy - self.height / 2.0) / self.focal;
                // Ray in the bearing-rotated frame: right, screen-down, forward.
                let dir_x = vx;
                let dir_y = vy * cos_p - sin_p;
                let dir_h = -vy * sin_p - cos_p;
                let (cam_y, cam_h) = (self.distance * sin_p, self.distance * cos_p);
                let t_hit = if dir_h < -1e-9 {
                    cam_h / -dir_h
                } else {
                    f64::INFINITY
                };
                let (mut gx, mut gy) = (dir_x * t_hit, cam_y + dir_y * t_hit);
                let reach = gx.hypot(gy);
                if !reach.is_finite() || reach > far {
                    let len = dir_x.hypot(dir_y).max(1e-9);
                    gx = dir_x / len * far;
                    gy = dir_y / len * far;
                }
                let wx = self.target_x + cos_b * gx - sin_b * gy;
                let wy = self.target_y + sin_b * gx + cos_b * gy;
                bounds.0 = bounds.0.min(wx);
                bounds.1 = bounds.1.min(wy);
                bounds.2 = bounds.2.max(wx);
                bounds.3 = bounds.3.max(wy);
            }
        }
        bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::Projection;

    fn lens(pitch_deg: f64, bearing_deg: f64) -> Lens {
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
    fn a_look_at_camera_from_the_eye_reproduces_the_projection() {
        let cam = Camera::new(37.55, 126.98, 15.0, 640, 480, lens(60.0, 35.0));
        let cam = Camera {
            distance: cam.distance * 1.7,
            ..cam
        };
        assert!((cam.fov_deg() - 40.0).abs() < 1e-9);
        let (ex, ey, eh) = cam.eye();
        let (tx, ty) = cam.target();
        let norm = |v: [f64; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            v.map(|c| c / l)
        };
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let forward = norm([tx - ex, ty - ey, -eh]);
        let right = norm(cross([0.0, 0.0, 1.0], forward));
        let up = cross(forward, right);
        let focal = 240.0 / (cam.fov_deg().to_radians() / 2.0).tan();
        for (x, y, h) in [(tx + 40.0, ty - 90.0, 0.0), (tx - 25.0, ty + 10.0, 30.0)] {
            let p = [x - ex, y - ey, h - eh];
            let depth = dot(p, forward);
            let expected = (
                320.0 + focal * dot(p, right) / depth,
                240.0 - focal * dot(p, up) / depth,
            );
            let (sx, sy) = cam.project_world(x, y, h).unwrap();
            assert!((sx - expected.0).abs() < 1e-6 && (sy - expected.1).abs() < 1e-6);
        }
    }

    #[test]
    fn the_eye_is_where_view_depth_is_zero() {
        let cam = Camera::new(37.55, 126.98, 15.0, 640, 480, lens(60.0, 35.0));
        let (x, y, h) = cam.eye();
        let v = cam.view(x, y, h);
        assert!(
            v.x.abs() < 1e-6 && v.y.abs() < 1e-6 && v.depth.abs() < 1e-6,
            "{v:?}"
        );
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
    fn overscan_shifts_by_the_margin() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(55.0, 30.0));
        let wide = cam.overscanned(50.0);
        let (wx, wy) = cam.ground_at(100.0, 400.0).unwrap();
        let (x, y) = wide.project_world(wx, wy, 0.0).unwrap();
        assert!((x - 150.0).abs() < 1e-6 && (y - 450.0).abs() < 1e-6);
    }

    #[test]
    fn ground_at_inverts_projection() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(55.0, 30.0));
        for (sx, sy) in [(320.0, 240.0), (10.0, 470.0), (600.0, 200.0)] {
            let (wx, wy) = cam.ground_at(sx, sy).unwrap();
            let (px, py) = cam.project_world(wx, wy, 0.0).unwrap();
            assert!(
                (px - sx).abs() < 1e-6 && (py - sy).abs() < 1e-6,
                "{sx},{sy}"
            );
        }
        assert!(cam.ground_at(320.0, cam.horizon_y() - 1.0).is_none());
    }

    #[test]
    fn top_down_matches_flat_projection() {
        let (lat, lon) = (37.5665, 126.9780);
        let cam = Camera::new(lat, lon, 15.0, 640, 480, lens(0.0, 0.0));
        let flat = Projection::centered(lat, lon, 15.0, 640, 480);
        let (plat, plon) = (37.5700, 126.9850);
        let (cx, cy) = cam.project(plat, plon, 0.0).unwrap();
        let (fx, fy) = flat.project(plat, plon);
        assert!((cx - fx).abs() < 1e-6 && (cy - fy).abs() < 1e-6);
    }

    #[test]
    fn raised_points_move_up_the_screen_when_pitched() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(50.0, 0.0));
        let (_, ground) = cam.project(37.5, 127.0, 0.0).unwrap();
        let (_, raised) = cam.project(37.5, 127.0, 50.0).unwrap();
        assert!(raised < ground);
    }

    #[test]
    fn bearing_points_that_direction_up() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(0.0, 90.0));
        let (ex, ey) = cam.project(37.5, 127.01, 0.0).unwrap();
        assert!(
            (ex - 320.0).abs() < 1e-6 && ey < 240.0,
            "east should point up"
        );
        let (nx, _) = cam.project(37.51, 127.0, 0.0).unwrap();
        assert!(nx < 320.0, "north should point left");
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

    #[test]
    fn points_behind_the_camera_are_not_projected() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(70.0, 0.0));
        let behind_y = lat_to_pixel(37.5, 15.0) + cam.distance * 2.0;
        assert!(cam
            .project_world(lon_to_pixel(127.0, 15.0), behind_y, 0.0)
            .is_none());
    }

    #[test]
    fn visible_tiles_respect_budget_and_cover_target() {
        let cam = Camera::new(37.5, 127.0, 16.0, 1280, 720, lens(60.0, 20.0));
        let tiles = cam.visible_tiles(14, 4.0, 64);
        assert!(!tiles.is_empty() && tiles.len() <= 64);
        let (tx, ty) = (lon_to_pixel(127.0, 16.0), lat_to_pixel(37.5, 16.0));
        assert!(tiles
            .iter()
            .any(|t| tx >= t.dx && tx < t.dx + t.size && ty >= t.dy && ty < t.dy + t.size));
    }
}
