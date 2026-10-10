use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, PathBuilder, Pixmap,
    Point, Rect, Shader, SpreadMode, Stroke, Transform,
};

use super::frame::FrameStyle;
use super::ramp::stops;
use super::text::{draw_text, line_height};
use crate::anim::Timeline;

/// Layout of the elevation profile panel along the bottom edge.
#[derive(Debug, Clone, Copy)]
pub struct ProfileStyle {
    /// Panel height in pixels, including the label row.
    pub height: f32,
    /// Gap between the panel and the canvas edges.
    pub margin: f32,
    pub label_size: f32,
}

impl Default for ProfileStyle {
    fn default() -> Self {
        Self {
            height: 110.0,
            margin: 12.0,
            label_size: 15.0,
        }
    }
}

impl ProfileStyle {
    /// Vertical space the panel occupies from the bottom edge, so bottom
    /// overlays can sit above it.
    pub fn reserved_height(&self) -> f32 {
        self.height + self.margin
    }
}

/// Draw a distance–elevation chart with the progress up to `u` highlighted and
/// a marker at the current position.
///
/// The highlight uses the elevation ramp when `frame.color_by_elevation` is
/// set, otherwise `frame.tail_color`. Does nothing without elevation data.
pub fn draw_profile(
    pixmap: &mut Pixmap,
    timeline: &Timeline,
    u: f64,
    style: &ProfileStyle,
    frame: &FrameStyle,
) {
    let Some(elevation) = timeline.elevation() else {
        return;
    };
    let total = elevation.total_distance_m();
    if total <= 0.0 {
        return;
    }

    let canvas_w = pixmap.width() as f32;
    let canvas_h = pixmap.height() as f32;
    let Some(panel) = Rect::from_ltrb(
        style.margin,
        canvas_h - style.margin - style.height,
        canvas_w - style.margin,
        canvas_h - style.margin,
    ) else {
        return;
    };
    fill_rect(pixmap, panel, Color::from_rgba8(0, 0, 0, 0x73));

    let pad = 8.0;
    let label_h = line_height(style.label_size);
    let Some(plot) = Rect::from_ltrb(
        panel.left() + pad,
        panel.top() + pad + label_h,
        panel.right() - pad,
        panel.bottom() - pad,
    ) else {
        return;
    };

    let to_xy = |dist: f64, ele: f64| -> (f32, f32) {
        let x = plot.left() + (dist / total) as f32 * plot.width();
        let y = plot.bottom() - elevation.normalize(ele) as f32 * plot.height();
        (x, y)
    };
    let (i, frac) = timeline.locate(u);
    let current = (elevation.distance_at(i, frac), elevation.at(i, frac));
    let (mx, my) = to_xy(current.0, current.1);

    let full = thin_columns(elevation.profile().map(|(d, e)| to_xy(d, e)));
    let mut done: Vec<(f32, f32)> = full.iter().copied().take_while(|p| p.0 < mx).collect();
    done.push((mx, my));

    if let Some(area) = area_path(&full, plot.bottom()) {
        let mut paint = antialiased();
        paint.set_color(Color::from_rgba8(0xff, 0xff, 0xff, 0x26));
        pixmap.fill_path(
            &area,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some(area) = area_path(&done, plot.bottom()) {
        let mut paint = antialiased();
        paint.shader = progress_shader(frame, plot);
        pixmap.fill_path(
            &area,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some(line) = line_path(&full) {
        let mut paint = antialiased();
        paint.set_color(Color::from_rgba8(0xff, 0xff, 0xff, 0xb0));
        let stroke = Stroke {
            width: 1.5,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&line, &paint, &stroke, Transform::identity(), None);
    }

    if let Some(drop) = line_path(&[(mx, my), (mx, plot.bottom())]) {
        let mut paint = antialiased();
        paint.set_color(Color::from_rgba8(0xff, 0xff, 0xff, 0x80));
        let stroke = Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(&drop, &paint, &stroke, Transform::identity(), None);
    }
    if let Some(dot) = PathBuilder::from_circle(mx, my, 4.0) {
        let mut paint = antialiased();
        paint.set_color(frame.marker_color);
        pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
    }

    let label = format!(
        "{:.0} m  ·  {:.2} / {:.2} km",
        current.1,
        current.0 / 1000.0,
        total / 1000.0
    );
    draw_text(
        pixmap,
        &label,
        panel.left() + pad,
        panel.top() + pad * 0.5,
        style.label_size,
        Color::WHITE,
    );
}

/// Keep points at least half a pixel apart horizontally (plus the last), so
/// long recordings don't stroke thousands of sub-pixel segments.
fn thin_columns(points: impl Iterator<Item = (f32, f32)>) -> Vec<(f32, f32)> {
    let mut kept: Vec<(f32, f32)> = Vec::new();
    let mut tail = None;
    for p in points {
        match kept.last() {
            Some(last) if p.0 - last.0 < 0.5 => tail = Some(p),
            _ => {
                kept.push(p);
                tail = None;
            }
        }
    }
    kept.extend(tail);
    kept
}

fn antialiased() -> Paint<'static> {
    Paint {
        anti_alias: true,
        ..Default::default()
    }
}

fn fill_rect(pixmap: &mut Pixmap, rect: Rect, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

fn line_path(points: &[(f32, f32)]) -> Option<tiny_skia::Path> {
    let (first, rest) = points.split_first()?;
    let mut builder = PathBuilder::new();
    builder.move_to(first.0, first.1);
    for &(x, y) in rest {
        builder.line_to(x, y);
    }
    builder.finish()
}

/// Closed area between a polyline and the horizontal `baseline`.
fn area_path(points: &[(f32, f32)], baseline: f32) -> Option<tiny_skia::Path> {
    let (first, last) = (points.first()?, points.last()?);
    let mut builder = PathBuilder::new();
    builder.move_to(first.0, baseline);
    for &(x, y) in points {
        builder.line_to(x, y);
    }
    builder.line_to(last.0, baseline);
    builder.close();
    builder.finish()
}

/// Vertical elevation-ramp gradient matching the plot's y-axis, or the solid
/// tail color.
fn progress_shader(frame: &FrameStyle, plot: Rect) -> Shader<'static> {
    let solid = Shader::SolidColor(with_alpha(frame.tail_color, 0.75));
    if !frame.color_by_elevation {
        return solid;
    }
    let gradient_stops: Vec<GradientStop> = stops()
        .map(|(at, color)| GradientStop::new(at, with_alpha(color, 0.75)))
        .collect();
    LinearGradient::new(
        Point::from_xy(0.0, plot.bottom()),
        Point::from_xy(0.0, plot.top()),
        gradient_stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(solid)
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color::from_rgba(color.red(), color.green(), color.blue(), alpha).unwrap_or(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpx::model::TrackPoint;

    fn hill() -> Timeline {
        let pts: Vec<TrackPoint> = [(0.0, 10.0), (0.01, 200.0), (0.02, 50.0)]
            .iter()
            .map(|&(lon, ele)| {
                let mut p = TrackPoint::new(37.0, 127.0 + lon);
                p.ele = Some(ele);
                p
            })
            .collect();
        Timeline::build(&pts).unwrap()
    }

    fn blank() -> Pixmap {
        let mut p = Pixmap::new(400, 300).unwrap();
        p.fill(Color::BLACK);
        p
    }

    #[test]
    fn draws_inside_bottom_panel_only() {
        let mut pixmap = blank();
        let style = ProfileStyle::default();
        draw_profile(&mut pixmap, &hill(), 0.5, &style, &FrameStyle::default());
        let panel_top = (300.0 - style.reserved_height()) as u32;
        assert_eq!(pixmap.pixel(200, panel_top - 5).unwrap().red(), 0);
        assert_ne!(pixmap.data(), blank().data());
    }

    #[test]
    fn progress_changes_the_chart() {
        let style = ProfileStyle::default();
        let mut early = blank();
        let mut late = blank();
        draw_profile(&mut early, &hill(), 0.1, &style, &FrameStyle::default());
        draw_profile(&mut late, &hill(), 0.9, &style, &FrameStyle::default());
        assert_ne!(early.data(), late.data());
    }

    #[test]
    fn no_elevation_draws_nothing() {
        let pts = vec![TrackPoint::new(0.0, 0.0), TrackPoint::new(0.0, 1.0)];
        let mut pixmap = blank();
        let style = ProfileStyle::default();
        draw_profile(
            &mut pixmap,
            &Timeline::build(&pts).unwrap(),
            0.5,
            &style,
            &FrameStyle::default(),
        );
        assert_eq!(pixmap.data(), blank().data());
    }
}
