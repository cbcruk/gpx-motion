use tiny_skia::{Color, Pixmap};

use super::sample::Image;

use crate::geo::camera::{mat_mul, Camera, Mat3, NEAR};

/// Shifts screen coordinates half a pixel so integers land on pixel centers.
const TO_PIXEL_CENTERS: Mat3 = [[1.0, 0.0, -0.5], [0.0, 1.0, -0.5], [0.0, 0.0, 1.0]];

/// A ground-plane image (map only, without fog) and the camera it was
/// rendered with.
pub struct GroundView {
    /// Camera whose viewport matches `pixmap` (possibly overscanned).
    pub camera: Camera,
    /// The rendered ground, premultiplied.
    pub pixmap: Pixmap,
}

/// Fill `target` with the ground as `camera` sees it, resampled from views
/// rendered by other cameras of the same zoom.
///
/// The ground is a plane, so each target pixel maps to one world point, which
/// is looked up in the first source that saw it (bilinear). Pixels above the
/// horizon or seen by no source get `background`. Much cheaper than redrawing
/// the map, at the cost of slight blur; keep sources close in bearing.
pub fn reproject_ground(
    target: &mut Pixmap,
    camera: &Camera,
    sources: &[&GroundView],
    background: Color,
) {
    let fill = background.premultiply().to_color_u8();
    let (width, height) = (target.width() as usize, target.height() as usize);
    let to_ground = camera.screen_to_ground();
    let max_w = camera.near_ground_weight();
    // Target pixel → source pixel is one homography per source; along a row
    // its homogeneous result changes linearly, so step it by the x column.
    let sources: Vec<(Mat3, Image)> = sources
        .iter()
        .map(|s| {
            let h = mat_mul(&s.camera.ground_to_screen(), &to_ground);
            (mat_mul(&TO_PIXEL_CENTERS, &h), Image::new(&s.pixmap))
        })
        .collect();
    let pixels = target.pixels_mut();
    for y in 0..height {
        let row = &mut pixels[y * width..(y + 1) * width];
        let sy = y as f64 + 0.5;
        // The ground's homogeneous weight depends only on the row: it is
        // non-positive above the horizon and large nearer than the near plane.
        let w = to_ground[2][1] * sy + to_ground[2][2];
        if w <= f64::EPSILON || w > max_w {
            row.fill(fill);
            continue;
        }
        let min_depth = NEAR * w;
        for (x, px) in row.iter_mut().enumerate() {
            let sx = x as f64 + 0.5;
            *px = sources
                .iter()
                .find_map(|(h, image)| {
                    let depth = h[2][0] * sx + h[2][1] * sy + h[2][2];
                    if depth < min_depth {
                        return None;
                    }
                    let inv = 1.0 / depth;
                    image.sample(
                        (h[0][0] * sx + h[0][1] * sy + h[0][2]) * inv,
                        (h[1][0] * sx + h[1][1] * sy + h[1][2]) * inv,
                    )
                })
                .unwrap_or(fill);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::camera::Lens;
    use tiny_skia::{Paint, Rect, Transform};

    fn camera(bearing_deg: f64) -> Camera {
        Camera::new(
            37.5,
            127.0,
            15.0,
            200,
            150,
            Lens {
                pitch_deg: 50.0,
                bearing_deg,
                fov_deg: 40.0,
            },
        )
    }

    fn checker(width: u32, height: u32) -> Pixmap {
        let mut pixmap = Pixmap::new(width, height).unwrap();
        pixmap.fill(Color::WHITE);
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(0x20, 0x60, 0xa0, 0xff));
        for i in (0..width).step_by(20) {
            let rect = Rect::from_xywh(i as f32, 0.0, 10.0, height as f32).unwrap();
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
        pixmap
    }

    #[test]
    fn same_camera_reproduces_the_ground_below_the_horizon() {
        let cam = camera(0.0);
        let source = GroundView {
            camera: cam,
            pixmap: checker(200, 150),
        };
        let mut target = Pixmap::new(200, 150).unwrap();
        reproject_ground(&mut target, &cam, &[&source], Color::BLACK);
        let below = cam.horizon_y().max(0.0) as u32 + 2;
        for y in below..150 {
            for x in 0..200 {
                assert_eq!(target.pixel(x, y), source.pixmap.pixel(x, y), "({x}, {y})");
            }
        }
    }

    #[test]
    fn falls_back_to_later_sources_then_background() {
        let source = GroundView {
            camera: camera(0.0),
            pixmap: checker(200, 150),
        };
        let mut target = Pixmap::new(200, 150).unwrap();
        reproject_ground(&mut target, &camera(90.0), &[&source], Color::BLACK);
        let black = target
            .pixels()
            .iter()
            .filter(|p| p.red() == 0 && p.alpha() == 0xff)
            .count();
        assert!(black > 0, "a 90° turn leaves areas no source saw");
        assert!(black < 200 * 150, "the overlap is resampled");
    }
}
