use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

use crate::anim::Timeline;
use crate::geo::mercator::Projection;

/// Visual parameters for an animated frame.
#[derive(Debug, Clone)]
pub struct FrameStyle {
    pub width: u32,
    pub height: u32,
    pub padding: f64,
    pub background: Color,
    pub tail_color: Color,
    pub tail_width: f32,
    pub marker_color: Color,
    pub marker_radius: f32,
}

impl Default for FrameStyle {
    fn default() -> Self {
        Self {
            width: 640,
            height: 640,
            padding: 32.0,
            background: Color::from_rgba8(0x1e, 0x1e, 0x1e, 0xff),
            tail_color: Color::from_rgba8(0xff, 0x5a, 0x36, 0xff),
            tail_width: 3.0,
            marker_color: Color::from_rgba8(0xff, 0xff, 0xff, 0xff),
            marker_radius: 6.0,
        }
    }
}

/// Render one animation frame: background, the fading tail ending at progress
/// `u`, and the current-position marker.
///
/// `tail_u` is the tail length expressed as a fraction of total progress.
pub fn render_frame(
    timeline: &Timeline,
    proj: &Projection,
    u: f64,
    tail_u: f64,
    style: &FrameStyle,
) -> Pixmap {
    let mut pixmap =
        Pixmap::new(style.width, style.height).expect("viewport dimensions must be non-zero");
    pixmap.fill(style.background);
    draw_overlay(&mut pixmap, timeline, proj, u, tail_u, style);
    pixmap
}

/// Draw the fading tail and current-position marker onto an existing canvas
/// (e.g. a pre-composited map background), without touching its pixels first.
pub fn draw_overlay(
    pixmap: &mut Pixmap,
    timeline: &Timeline,
    proj: &Projection,
    u: f64,
    tail_u: f64,
    style: &FrameStyle,
) {
    draw_tail(pixmap, timeline, proj, u, tail_u, style);
    draw_marker(pixmap, timeline, proj, u, style);
}

fn draw_tail(
    pixmap: &mut Pixmap,
    timeline: &Timeline,
    proj: &Projection,
    u: f64,
    tail_u: f64,
    style: &FrameStyle,
) {
    let u0 = (u - tail_u).max(0.0);
    let samples = timeline.window(u0, u);
    if samples.len() < 2 {
        return;
    }

    let span = (u - u0).max(f64::EPSILON);
    let stroke = Stroke {
        width: style.tail_width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };

    // Stroke each segment with alpha rising toward the marker so the tail fades.
    for pair in samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (ax, ay) = proj.project(a.lat, a.lon);
        let (bx, by) = proj.project(b.lat, b.lon);

        let mut builder = PathBuilder::new();
        builder.move_to(ax as f32, ay as f32);
        builder.line_to(bx as f32, by as f32);
        let Some(path) = builder.finish() else {
            continue;
        };

        let recency = ((b.u - u0) / span).clamp(0.0, 1.0) as f32;
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(scale_alpha(style.tail_color, recency));
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn draw_marker(
    pixmap: &mut Pixmap,
    timeline: &Timeline,
    proj: &Projection,
    u: f64,
    style: &FrameStyle,
) {
    let (lat, lon) = timeline.position_at(u);
    let (x, y) = proj.project(lat, lon);
    let Some(circle) = PathBuilder::from_circle(x as f32, y as f32, style.marker_radius) else {
        return;
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(style.marker_color);
    pixmap.fill_path(
        &circle,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn scale_alpha(color: Color, factor: f32) -> Color {
    Color::from_rgba(
        color.red(),
        color.green(),
        color.blue(),
        color.alpha() * factor,
    )
    .unwrap_or(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpx::model::TrackPoint;

    fn timeline_and_proj(style: &FrameStyle) -> (Timeline, Projection) {
        let pts = vec![
            TrackPoint::new(37.5665, 126.9780),
            TrackPoint::new(37.5700, 126.9850),
            TrackPoint::new(37.5610, 126.9700),
        ];
        let tl = Timeline::build(&pts).unwrap();
        let proj = Projection::fit(&pts, style.width, style.height, style.padding).unwrap();
        (tl, proj)
    }

    #[test]
    fn frame_has_expected_dimensions() {
        let style = FrameStyle::default();
        let (tl, proj) = timeline_and_proj(&style);
        let frame = render_frame(&tl, &proj, 0.5, 0.3, &style);
        assert_eq!(frame.width(), style.width);
        assert_eq!(frame.height(), style.height);
    }

    #[test]
    fn first_frame_differs_from_blank_background() {
        let style = FrameStyle::default();
        let (tl, proj) = timeline_and_proj(&style);
        let mut blank = Pixmap::new(style.width, style.height).unwrap();
        blank.fill(style.background);
        let frame = render_frame(&tl, &proj, 0.0, 0.3, &style);
        // Even at u=0 the marker is drawn, so the frame is non-empty.
        assert_ne!(blank.data(), frame.data());
    }

    #[test]
    fn later_frame_differs_from_first() {
        let style = FrameStyle::default();
        let (tl, proj) = timeline_and_proj(&style);
        let first = render_frame(&tl, &proj, 0.0, 0.3, &style);
        let later = render_frame(&tl, &proj, 0.8, 0.3, &style);
        assert_ne!(first.data(), later.data());
    }
}
