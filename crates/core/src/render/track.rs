use tiny_skia::{Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::ramp::elevation_color;
use crate::anim::Timeline;
use crate::geo::mercator::Projection;
use crate::gpx::model::TrackPoint;

/// Visual parameters for a static track render.
#[derive(Debug, Clone)]
pub struct TrackStyle {
    pub width: u32,
    pub height: u32,
    pub padding: f64,
    pub background: Color,
    pub track_color: Color,
    pub track_width: f32,
}

impl TrackStyle {
    /// The same style for a canvas with `ratio` device pixels per CSS pixel.
    pub fn scaled(mut self, ratio: f32) -> Self {
        self.padding *= ratio as f64;
        self.track_width *= ratio;
        self
    }
}

impl Default for TrackStyle {
    fn default() -> Self {
        Self {
            width: 640,
            height: 640,
            padding: 32.0,
            background: Color::from_rgba8(0x1e, 0x1e, 0x1e, 0xff),
            track_color: Color::from_rgba8(0xff, 0x5a, 0x36, 0xff),
            track_width: 3.0,
        }
    }
}

/// Render a single track as a polyline on a solid background (Milestone 1).
///
/// Returns the filled pixel buffer; with fewer than two points only the
/// background is drawn.
pub fn render_track(points: &[TrackPoint], style: &TrackStyle) -> Pixmap {
    let mut pixmap =
        Pixmap::new(style.width, style.height).expect("viewport dimensions must be non-zero");
    pixmap.fill(style.background);

    if let Some(proj) = Projection::fit(points, style.width, style.height, style.padding) {
        stroke_track(
            &mut pixmap,
            points,
            &proj,
            style.track_color,
            style.track_width,
        );
    }
    pixmap
}

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

    #[test]
    fn renders_expected_dimensions() {
        let style = TrackStyle::default();
        let pixmap = render_track(&sample(), &style);
        assert_eq!(pixmap.width(), style.width);
        assert_eq!(pixmap.height(), style.height);
    }

    #[test]
    fn track_paints_over_background() {
        let style = TrackStyle::default();
        let bg = render_track(&[], &style);
        let with_track = render_track(&sample(), &style);
        assert_ne!(
            bg.data(),
            with_track.data(),
            "track pixels should differ from a blank background"
        );
    }

    #[test]
    fn empty_track_is_pure_background() {
        let style = TrackStyle::default();
        let pixmap = render_track(&[], &style);
        let expected = style.background.premultiply().to_color_u8();
        let px = pixmap.pixel(0, 0).unwrap();
        assert_eq!(px.red(), expected.red());
        assert_eq!(px.green(), expected.green());
        assert_eq!(px.blue(), expected.blue());
    }
}
