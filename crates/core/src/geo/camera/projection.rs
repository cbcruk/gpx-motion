//! Projecting world points through a [`Camera`] onto the screen, and back
//! onto the ground plane.

use super::Camera;
use crate::geo::mercator::{lat_to_pixel, lon_to_pixel};

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

impl Camera {
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

    /// [`Camera::view`] and [`Camera::to_screen`] with the turn and tilt
    /// worked out once, for projecting many points.
    pub fn projector(&self) -> Projector {
        let (sin_b, cos_b) = self.bearing.sin_cos();
        let (sin_p, cos_p) = self.pitch.sin_cos();
        Projector {
            camera: *self,
            sin_b,
            cos_b,
            sin_p,
            cos_p,
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

    /// Screen y of the horizon (may lie above the viewport).
    pub fn horizon_y(&self) -> f64 {
        let (sin_p, cos_p) = self.pitch.sin_cos();
        if sin_p <= f64::EPSILON {
            return f64::NEG_INFINITY;
        }
        self.height / 2.0 - self.focal * cos_p / sin_p
    }
}

/// A [`Camera`] ready to project many points (see [`Camera::projector`]).
#[derive(Debug, Clone, Copy)]
pub struct Projector {
    camera: Camera,
    sin_b: f64,
    cos_b: f64,
    sin_p: f64,
    cos_p: f64,
}

impl Projector {
    /// The camera this projects through.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// [`Camera::view`] of a world point.
    pub fn view(&self, x: f64, y: f64, h: f64) -> ViewPoint {
        let c = &self.camera;
        let (dx, dy) = (x - c.target_x, y - c.target_y);
        let rx = self.cos_b * dx + self.sin_b * dy;
        let ry = -self.sin_b * dx + self.cos_b * dy;
        ViewPoint {
            x: rx,
            y: ry * self.cos_p - h * self.sin_p,
            depth: c.distance - ry * self.sin_p - h * self.cos_p,
        }
    }

    /// [`Camera::to_screen`].
    pub fn to_screen(&self, v: ViewPoint) -> (f64, f64) {
        self.camera.to_screen(v)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::lens;
    use super::*;
    use crate::geo::Projection;

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
    fn the_projector_matches_the_camera() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(55.0, 33.0));
        let p = cam.projector();
        let (tx, ty) = cam.target();
        for (x, y, h) in [(0.0, 0.0, 0.0), (120.0, -40.0, 15.0), (-300.0, 900.0, 2.5)] {
            let (a, b) = (cam.view(tx + x, ty + y, h), p.view(tx + x, ty + y, h));
            assert_eq!((a.x, a.y, a.depth), (b.x, b.y, b.depth));
            assert_eq!(cam.to_screen(a), p.to_screen(b));
        }
    }

    #[test]
    fn points_behind_the_camera_are_not_projected() {
        let cam = Camera::new(37.5, 127.0, 15.0, 640, 480, lens(70.0, 0.0));
        let behind_y = lat_to_pixel(37.5, 15.0) + cam.distance * 2.0;
        assert!(cam
            .project_world(lon_to_pixel(127.0, 15.0), behind_y, 0.0)
            .is_none());
    }
}
