//! Whole-track polylines for the 2D view, in one color or by elevation.

use tiny_skia::{Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::ramp::elevation_color;
use crate::anim::Timeline;
use crate::geo::mercator::Projection;
use crate::gpx::model::TrackPoint;

/// Stroke a track polyline onto an existing canvas with the given projection.
///
/// Used both for the static PNG and the pre-draw (full-route) underlay. Does
/// nothing for fewer than two points.
pub fn stroke_track(
    pixmap: &mut Pixmap,
    points: &[TrackPoint],
    proj: &Projection,
    color: Color,
    width: f32,
) {
    let mut builder = PathBuilder::new();
    for (i, p) in points.iter().enumerate() {
        let (x, y) = proj.project(p.lat, p.lon);
        if i == 0 {
            builder.move_to(x as f32, y as f32);
        } else {
            builder.line_to(x as f32, y as f32);
        }
    }

    let Some(path) = builder.finish() else {
        return;
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(color);
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Stroke the whole track with each segment colored by its mean elevation.
///
/// Falls back to a single `fallback` color when the track has no elevation.
pub fn stroke_track_by_elevation(
    pixmap: &mut Pixmap,
    timeline: &Timeline,
    proj: &Projection,
    fallback: Color,
    width: f32,
) {
    let Some(elevation) = timeline.elevation() else {
        let points: Vec<TrackPoint> = (0..timeline.len())
            .map(|i| {
                let (lat, lon) = timeline.vertex(i);
                TrackPoint::new(lat, lon)
            })
            .collect();
        stroke_track(pixmap, &points, proj, fallback, width);
        return;
    };
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    for i in 1..timeline.len() {
        let (alat, alon) = timeline.vertex(i - 1);
        let (blat, blon) = timeline.vertex(i);
        let (ax, ay) = proj.project(alat, alon);
        let (bx, by) = proj.project(blat, blon);
        let mut builder = PathBuilder::new();
        builder.move_to(ax as f32, ay as f32);
        builder.line_to(bx as f32, by as f32);
        let Some(path) = builder.finish() else {
            continue;
        };
        let mean = (elevation.vertex(i - 1) + elevation.vertex(i)) / 2.0;
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(elevation_color(elevation.normalize(mean)));
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<TrackPoint> {
        vec![
            TrackPoint::new(37.5665, 126.9780),
            TrackPoint::new(37.5700, 126.9850),
            TrackPoint::new(37.5610, 126.9700),
        ]
    }

    fn stroked(points: &[TrackPoint]) -> Pixmap {
        let mut pixmap = Pixmap::new(640, 640).unwrap();
        pixmap.fill(Color::BLACK);
        let proj = Projection::fit(&sample(), 640, 640, 32.0).unwrap();
        stroke_track(&mut pixmap, points, &proj, Color::WHITE, 3.0);
        pixmap
    }

    #[test]
    fn track_paints_over_background() {
        assert_ne!(
            stroked(&[]).data(),
            stroked(&sample()).data(),
            "track pixels should differ from a blank background"
        );
    }

    #[test]
    fn a_single_point_draws_nothing() {
        let expected = Color::BLACK.premultiply().to_color_u8();
        let pixmap = stroked(&sample()[..1]);
        assert!(pixmap.pixels().iter().all(|&px| px == expected));
    }
}
