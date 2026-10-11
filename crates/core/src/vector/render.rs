use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint as SkPaint, PathBuilder, Pixmap, PixmapPaint, Stroke,
    StrokeDash, Transform,
};

use super::mvt::{GeomType, VectorTile};
use super::style::{Cap, Join, MapStyle, Paint};
use crate::geo::camera::{Camera, ViewPoint, NEAR};
use crate::geo::mercator::TILE_SIZE;
use crate::tiles::TilePlacement;

/// Rasterize vector tiles onto `target` with `style`, blended at `opacity`.
///
/// Rules are drawn in style order across every tile, so a road in one tile is
/// never painted under water from its neighbor. Tile buffers overlap at the
/// edges, which is invisible with opaque style colors. Overzoomed placements
/// (`size > 256`) scale the vector geometry, so it stays sharp at any zoom.
pub fn draw_vector_map(
    target: &mut Pixmap,
    tiles: &[(TilePlacement, &VectorTile)],
    style: &MapStyle,
    opacity: f32,
) {
    let flat = |p: &TilePlacement, x: f64, y: f64| ViewPoint {
        x: p.dx + x * p.size,
        y: p.dy + y * p.size,
        depth: NEAR,
    };
    let screen = |v: ViewPoint| (v.x, v.y);
    let zoom = |p: &TilePlacement| p.id.zoom as f64 + (p.size / TILE_SIZE).log2();
    draw_layers(
        target,
        tiles,
        style,
        opacity,
        flat,
        screen,
        Widths { zoom, depth: None },
    );
}

/// Rasterize vector tiles as a ground plane seen through a perspective camera.
///
/// `tiles` carry world-pixel placements from [`Camera::visible_tiles`].
/// Geometry is clipped at the camera's near plane before projection, so large
/// polygons reaching behind the camera still fill correctly. Line widths follow
/// the camera zoom and shrink with distance (and grow up close) relative to
/// the camera target.
pub fn draw_vector_map_3d(
    target: &mut Pixmap,
    tiles: &[(TilePlacement, &VectorTile)],
    style: &MapStyle,
    camera: &Camera,
    opacity: f32,
) {
    let ground =
        |p: &TilePlacement, x: f64, y: f64| camera.view(p.dx + x * p.size, p.dy + y * p.size, 0.0);
    let screen = |v: ViewPoint| camera.to_screen(v);
    let widths = Widths {
        zoom: |_: &TilePlacement| camera.detail_zoom,
        depth: Some(camera.distance),
    };
    draw_layers(target, tiles, style, opacity, ground, screen, widths);
}

/// Line width multipliers are rounded to steps of `1 / WIDTH_STEPS`, so lines
/// at similar depths share one path.
const WIDTH_STEPS: f32 = 8.0;
/// Range of the depth-based line width multiplier.
const WIDTH_SCALE: (f32, f32) = (0.25, 1.5);

/// How line widths scale in [`draw_layers`].
struct Widths<Z> {
    /// World zoom a tile's lines are drawn at, for [`MapStyle::line_scale`].
    zoom: Z,
    /// Depth at which lines keep their width (the camera distance); lines
    /// scale inversely with depth around it. `None` keeps widths flat.
    depth: Option<f64>,
}

/// Shared rule loop: `to_view` maps tile-normalized coordinates (`0..=1`) to
/// camera space, and `to_screen` maps unclipped camera space to pixels.
fn draw_layers<Z: Fn(&TilePlacement) -> f64>(
    target: &mut Pixmap,
    tiles: &[(TilePlacement, &VectorTile)],
    style: &MapStyle,
    opacity: f32,
    to_view: impl Fn(&TilePlacement, f64, f64) -> ViewPoint,
    to_screen: impl Fn(ViewPoint) -> (f64, f64),
    widths: Widths<Z>,
) {
    let buckets = (WIDTH_SCALE.1 * WIDTH_STEPS) as usize;
    let bucket_of = |run: &[ViewPoint]| {
        let depth = run.iter().map(|v| v.depth).sum::<f64>() / run.len().max(1) as f64;
        let scale = widths
            .depth
            .map_or(1.0, |d| (d / depth.max(NEAR)) as f32)
            .clamp(WIDTH_SCALE.0, WIDTH_SCALE.1);
        ((scale * WIDTH_STEPS).round() as usize).clamp(1, buckets)
    };

    let Some(mut layer) = Pixmap::new(target.width(), target.height()) else {
        return;
    };
    layer.fill(style.background);

    for rule in &style.rules {
        let fill = matches!(rule.paint, Paint::Fill(_));
        let wanted = |g: GeomType| match g {
            GeomType::Polygon => fill || rule.outlines,
            GeomType::LineString => !fill,
            _ => false,
        };
        for (placement, tile) in tiles {
            let Some(source) = tile.layer(&rule.layer) else {
                continue;
            };
            let zoom = (widths.zoom)(placement);
            let maplibre_zoom = style.maplibre_zoom(zoom);
            if !rule.draws_at(maplibre_zoom) {
                continue;
            }
            let colored = match &rule.colors {
                None => rule.paint,
                Some(curve) => recolored(rule.paint, curve.at(maplibre_zoom)),
            };
            let paint = match &rule.opacity {
                None => colored,
                Some(curve) => faded(colored, curve.at(maplibre_zoom) as f32),
            };
            if alpha(paint) <= 0.0 {
                continue;
            }
            // Neighbouring tiles overlap by a buffer; translucent paint would
            // blend twice there, so it keeps to its own tile. Polygon outlines
            // always do: their rings run along where the tile cut them.
            let translucent = alpha(paint) < 1.0;
            let tile_scale = style
                .curve_width(rule, zoom)
                .unwrap_or_else(|| style.line_scale(zoom));
            let margin = match rule.paint {
                Paint::Fill(_) => 0.0,
                Paint::Line { width, .. } => width * tile_scale * WIDTH_SCALE.1 / 2.0 + 1.0,
            };
            let viewport = Viewport {
                width: target.width() as f32,
                height: target.height() as f32,
                margin,
            };
            let extent = source.extent.max(1) as f64;
            let mut builders: Vec<PathBuilder> =
                (0..=buckets).map(|_| PathBuilder::new()).collect();
            for feature in source.features.iter().filter(|f| wanted(f.geom_type)) {
                if !rule.matches(source, feature) {
                    continue;
                }
                let polygon = feature.geom_type == GeomType::Polygon;
                for part in feature.parts() {
                    let mut points: Vec<(f64, f64)> = part
                        .iter()
                        .map(|&(x, y)| (x as f64 / extent, y as f64 / extent))
                        .collect();
                    let to_view = |points: &[(f64, f64)]| -> Vec<ViewPoint> {
                        points
                            .iter()
                            .map(|&(x, y)| to_view(placement, x, y))
                            .collect()
                    };
                    if fill {
                        if translucent {
                            points = clip_to_tile(&points);
                        }
                        let ring = clip_ring(&to_view(&points));
                        append(&mut builders[0], &ring, &to_screen, &viewport, true);
                        continue;
                    }
                    if polygon {
                        // A polygon's outline, closed back to its start.
                        points.extend(points.first().copied());
                    }
                    let lines = if polygon || translucent {
                        clip_line_to_tile(&points)
                    } else {
                        vec![points]
                    };
                    for line in lines {
                        for run in clip_line(&to_view(&line)) {
                            let builder = &mut builders[bucket_of(&run)];
                            append(builder, &run, &to_screen, &viewport, false);
                        }
                    }
                }
            }
            // One path per tile (and line width): tiny-skia's scan converter
            // slows down sharply as a single path's edge count grows.
            for (bucket, builder) in builders.into_iter().enumerate() {
                if let Some(path) = builder.finish() {
                    let scale = if bucket == 0 {
                        1.0
                    } else {
                        bucket as f32 / WIDTH_STEPS
                    };
                    let line = LineLook {
                        dash: rule.dash.as_deref(),
                        cap: rule.cap,
                        join: rule.join,
                    };
                    paint_path(&mut layer, &path, paint, scale * tile_scale, line);
                }
            }
        }
    }

    target.draw_pixmap(
        0,
        0,
        layer.as_ref(),
        &PixmapPaint {
            opacity: opacity.clamp(0.0, 1.0),
            ..Default::default()
        },
        Transform::identity(),
        None,
    );
}

/// `paint` in `color` instead.
fn recolored(paint: Paint, color: Color) -> Paint {
    match paint {
        Paint::Fill(_) => Paint::Fill(color),
        Paint::Line { width, .. } => Paint::Line { color, width },
    }
}

/// `paint` with its color's alpha multiplied by `opacity`.
fn faded(paint: Paint, opacity: f32) -> Paint {
    let fade = |c: Color| {
        Color::from_rgba(
            c.red(),
            c.green(),
            c.blue(),
            c.alpha() * opacity.clamp(0.0, 1.0),
        )
        .unwrap_or(c)
    };
    match paint {
        Paint::Fill(c) => Paint::Fill(fade(c)),
        Paint::Line { color, width } => Paint::Line {
            color: fade(color),
            width,
        },
    }
}

fn alpha(paint: Paint) -> f32 {
    match paint {
        Paint::Fill(c) | Paint::Line { color: c, .. } => c.alpha(),
    }
}

/// Keep a ring's part inside the tile square (`0..=1`; Sutherland–Hodgman).
fn clip_to_tile(ring: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut points = ring.to_vec();
    let edges: [(usize, f64, bool); 4] = [
        (0, 0.0, true),
        (0, 1.0, false),
        (1, 0.0, true),
        (1, 1.0, false),
    ];
    for (axis, bound, lower) in edges {
        let coord = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };
        let inside = |p: (f64, f64)| {
            if lower {
                coord(p) >= bound
            } else {
                coord(p) <= bound
            }
        };
        let mut out = Vec::with_capacity(points.len() + 4);
        for (i, &b) in points.iter().enumerate() {
            let a = points[(i + points.len() - 1) % points.len()];
            if inside(b) != inside(a) {
                let t = (bound - coord(a)) / (coord(b) - coord(a));
                out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            }
            if inside(b) {
                out.push(b);
            }
        }
        points = out;
        if points.is_empty() {
            break;
        }
    }
    points
}

/// The parts of a polyline inside the tile square (`0..=1`), each a run of
/// points (Liang–Barsky per segment).
fn clip_line_to_tile(points: &[(f64, f64)]) -> Vec<Vec<(f64, f64)>> {
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut open = false;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let mut inside = true;
        for (p, q) in [(-dx, a.0), (dx, 1.0 - a.0), (-dy, a.1), (dy, 1.0 - a.1)] {
            if p == 0.0 {
                inside &= q >= 0.0;
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
        if !inside || t0 > t1 {
            open = false;
            continue;
        }
        let at = |t: f64| (a.0 + dx * t, a.1 + dy * t);
        if !(open && t0 == 0.0) {
            runs.push(vec![at(t0)]);
        }
        if let Some(run) = runs.last_mut() {
            run.push(at(t1));
        }
        open = t1 == 1.0;
    }
    runs.retain(|r| r.len() > 1);
    runs
}

/// How a line rule's strokes look besides color and width.
#[derive(Clone, Copy)]
struct LineLook<'a> {
    /// Dash pattern in line widths.
    dash: Option<&'a [f32]>,
    /// Line ends; `None` rounds them.
    cap: Option<Cap>,
    /// Line corners; `None` rounds them.
    join: Option<Join>,
}

fn paint_path(
    layer: &mut Pixmap,
    path: &tiny_skia::Path,
    paint: Paint,
    width_scale: f32,
    line: LineLook,
) {
    let mut sk = SkPaint {
        anti_alias: true,
        ..Default::default()
    };
    match paint {
        Paint::Fill(color) => {
            sk.set_color(color);
            layer.fill_path(path, &sk, FillRule::Winding, Transform::identity(), None);
        }
        Paint::Line { color, width } => {
            sk.set_color(color);
            let width = width * width_scale;
            if width <= 0.0 {
                return;
            }
            // Dashes are in multiples of the line width, like MapLibre's. An
            // odd pattern repeats with its last dash running into the first.
            let dash = line.dash.and_then(|d| {
                let mut intervals: Vec<f32> = d.iter().map(|x| x * width).collect();
                let mut offset = 0.0;
                if !intervals.len().is_multiple_of(2) {
                    let last = intervals.pop().unwrap_or(0.0);
                    if intervals.is_empty() {
                        return None;
                    }
                    intervals[0] += last;
                    offset = last;
                }
                StrokeDash::new(intervals, offset)
            });
            let stroke = Stroke {
                width,
                line_cap: match line.cap {
                    None | Some(Cap::Round) => LineCap::Round,
                    Some(Cap::Butt) => LineCap::Butt,
                    Some(Cap::Square) => LineCap::Square,
                },
                line_join: match line.join {
                    None | Some(Join::Round) => LineJoin::Round,
                    Some(Join::Miter) => LineJoin::Miter,
                    Some(Join::Bevel) => LineJoin::Bevel,
                },
                // MapLibre's default `line-miter-limit`.
                miter_limit: 2.0,
                dash,
            };
            layer.stroke_path(path, &sk, &stroke, Transform::identity(), None);
        }
    }
}

/// Pixel area a rule's paths may touch; `margin` covers half a stroke.
struct Viewport {
    width: f32,
    height: f32,
    margin: f32,
}

impl Viewport {
    /// Whether a part with these screen points can be skipped: it lies wholly
    /// off-screen, or its bounding box is smaller than [`MIN_FEATURE_PX`].
    fn skips(&self, points: &[(f32, f32)]) -> bool {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let m = self.margin;
        let off_screen = x1 < -m || y1 < -m || x0 > self.width + m || y0 > self.height + m;
        let tiny = x1 - x0 < MIN_FEATURE_PX && y1 - y0 < MIN_FEATURE_PX;
        off_screen || tiny
    }
}

/// Parts whose screen bounding box fits in this many pixels each way are
/// dropped: in a tilted view, distant buildings shrink to specks that cost far
/// more to anti-alias than they add.
const MIN_FEATURE_PX: f32 = 1.5;

/// Screen vertices closer than this to the previous kept one are dropped;
/// far-away geometry otherwise packs many vertices into each pixel.
const MIN_SEGMENT_PX: f32 = 0.75;

/// Project `points` and add them to `builder`, skipping parts that lie wholly
/// outside the viewport and vertices that don't move a visible distance.
fn append(
    builder: &mut PathBuilder,
    points: &[ViewPoint],
    to_screen: &impl Fn(ViewPoint) -> (f64, f64),
    viewport: &Viewport,
    close: bool,
) {
    if points.len() < 2 {
        return;
    }
    let screen: Vec<(f32, f32)> = points
        .iter()
        .map(|&v| {
            let (x, y) = to_screen(v);
            (x as f32, y as f32)
        })
        .collect();
    if viewport.skips(&screen) {
        return;
    }
    let last_i = screen.len() - 1;
    let mut prev = screen[0];
    builder.move_to(prev.0, prev.1);
    for (i, &(x, y)) in screen.iter().enumerate().skip(1) {
        let near = (x - prev.0).abs() < MIN_SEGMENT_PX && (y - prev.1).abs() < MIN_SEGMENT_PX;
        if near && i != last_i {
            continue;
        }
        builder.line_to(x, y);
        prev = (x, y);
    }
    if close {
        builder.close();
    }
}

/// Point where segment `a`–`b` crosses the near plane.
fn near_crossing(a: ViewPoint, b: ViewPoint) -> ViewPoint {
    a.lerp(b, (NEAR - a.depth) / (b.depth - a.depth))
}

/// Clip a closed ring to `depth >= NEAR` (Sutherland–Hodgman, one plane).
fn clip_ring(ring: &[ViewPoint]) -> Vec<ViewPoint> {
    if ring.iter().all(|v| v.depth >= NEAR) {
        return ring.to_vec();
    }
    let mut out = Vec::with_capacity(ring.len() + 2);
    for (i, &cur) in ring.iter().enumerate() {
        let prev = ring[(i + ring.len() - 1) % ring.len()];
        let (cur_in, prev_in) = (cur.depth >= NEAR, prev.depth >= NEAR);
        if cur_in != prev_in {
            out.push(near_crossing(prev, cur));
        }
        if cur_in {
            out.push(cur);
        }
    }
    out
}

/// Split a polyline into the runs in front of the near plane.
fn clip_line(line: &[ViewPoint]) -> Vec<Vec<ViewPoint>> {
    let mut runs = Vec::new();
    let mut run: Vec<ViewPoint> = Vec::new();
    for (i, &cur) in line.iter().enumerate() {
        let cur_in = cur.depth >= NEAR;
        if i > 0 {
            let prev = line[i - 1];
            let prev_in = prev.depth >= NEAR;
            if cur_in != prev_in {
                let cross = near_crossing(prev, cur);
                if prev_in {
                    run.push(cross);
                    runs.push(std::mem::take(&mut run));
                } else {
                    run.push(cross);
                }
            }
        }
        if cur_in {
            run.push(cur);
        }
    }
    if run.len() >= 2 {
        runs.push(run);
    }
    runs.retain(|r| r.len() >= 2);
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::TileId;
    use crate::vector::style::Rule;
    use tiny_skia::Color;

    #[test]
    fn viewport_skips_off_screen_and_tiny_parts() {
        let viewport = Viewport {
            width: 100.0,
            height: 100.0,
            margin: 2.0,
        };
        assert!(viewport.skips(&[(-10.0, 5.0), (-5.0, 50.0)]));
        assert!(viewport.skips(&[(50.0, 50.0), (50.5, 51.0)]));
        assert!(!viewport.skips(&[(-10.0, 50.0), (10.0, 50.0)]));
        assert!(!viewport.skips(&[(-1.0, 50.0), (-1.0, 60.0)]));
    }

    fn placement(size: f64) -> TilePlacement {
        TilePlacement {
            id: TileId {
                zoom: 0,
                x: 0,
                y: 0,
            },
            dx: 0.0,
            dy: 0.0,
            size,
        }
    }

    fn style(fill: Color) -> MapStyle {
        MapStyle {
            pixel_ratio: 1.0,
            background: Color::BLACK,
            rules: vec![Rule::new("water", &[], Paint::Fill(fill))],
            maplibre_tiles: false,
        }
    }

    #[test]
    fn empty_tiles_paint_style_background() {
        let mut target = Pixmap::new(64, 64).unwrap();
        target.fill(Color::WHITE);
        draw_vector_map(&mut target, &[], &style(Color::WHITE), 1.0);
        let px = target.pixel(5, 5).unwrap();
        assert_eq!((px.red(), px.green(), px.blue()), (0, 0, 0));
    }

    #[test]
    fn zero_opacity_keeps_target() {
        let mut target = Pixmap::new(64, 64).unwrap();
        target.fill(Color::WHITE);
        draw_vector_map(&mut target, &[], &style(Color::WHITE), 0.0);
        assert_eq!(target.pixel(5, 5).unwrap().red(), 0xff);
    }

    fn vp(depth: f64) -> ViewPoint {
        ViewPoint {
            x: depth,
            y: 0.0,
            depth,
        }
    }

    #[test]
    fn ring_in_front_is_unchanged() {
        let ring = [vp(2.0), vp(3.0), vp(4.0)];
        assert_eq!(clip_ring(&ring), ring.to_vec());
    }

    #[test]
    fn ring_crossing_near_plane_is_cut_at_it() {
        let ring = [vp(-1.0), vp(3.0), vp(5.0)];
        let clipped = clip_ring(&ring);
        assert!(clipped.iter().all(|v| v.depth >= NEAR - 1e-9));
        assert_eq!(
            clipped
                .iter()
                .filter(|v| (v.depth - NEAR).abs() < 1e-9)
                .count(),
            2
        );
    }

    #[test]
    fn line_splits_where_it_dips_behind_camera() {
        let line = [vp(3.0), vp(2.0), vp(-2.0), vp(2.0), vp(4.0)];
        let runs = clip_line(&line);
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().flatten().all(|v| v.depth >= NEAR - 1e-9));
    }

    /// Render a MapLibre style's `layer` over `tile` (256 px at world zoom
    /// 14, MapLibre zoom 13) on black.
    fn render_maplibre(layer: serde_json::Value, tile: &VectorTile) -> Pixmap {
        let doc = serde_json::json!({
            "version": 8,
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#000"}},
                layer
            ]
        });
        let style = MapStyle::from_json(&doc.to_string()).unwrap();
        let mut target = Pixmap::new(256, 256).unwrap();
        let at_zoom_14 = TilePlacement {
            id: TileId {
                zoom: 14,
                x: 0,
                y: 0,
            },
            ..placement(256.0)
        };
        draw_vector_map(&mut target, &[(at_zoom_14, tile)], &style, 1.0);
        target
    }

    #[test]
    fn maplibre_opacity_curves_and_polygon_outlines_apply() {
        use super::super::mvt::Layer;
        use serde_json::json;
        let square = VectorTile {
            layers: vec![Layer::test_square("water", 1024, 3072)],
        };
        let outline = json!({"id": "edge", "type": "line", "source-layer": "water",
            "paint": {"line-color": "#fff", "line-width": 2,
                      "line-opacity": ["interpolate", ["linear"], ["zoom"], 12, 0, 14, 1]}});
        let drawn = render_maplibre(outline, &square);
        let lit: Vec<u8> = drawn
            .pixels()
            .iter()
            .map(|p| p.red())
            .filter(|&r| r > 0)
            .collect();
        assert!(
            lit.len() > 4 * 128 * 2 - 64,
            "the square's outline: {}",
            lit.len()
        );
        assert!(lit.iter().all(|&r| r < 200), "half opacity at zoom 13");
        assert_eq!(drawn.pixel(128, 128).unwrap().red(), 0, "not filled");
    }

    #[test]
    fn maplibre_colors_follow_zoom() {
        use super::super::mvt::Layer;
        use serde_json::json;
        let square = VectorTile {
            layers: vec![Layer::test_square("water", 1024, 3072)],
        };
        // Black at 12, white at 14: gray at the zoom these tests draw (13).
        let fill = json!({"id": "w", "type": "fill", "source-layer": "water",
            "paint": {"fill-color": ["interpolate", ["linear"], ["zoom"], 12, "#000", 14, "#fff"]}});
        let drawn = render_maplibre(fill, &square);
        let center = drawn.pixel(128, 128).unwrap().red();
        assert!((110..=145).contains(&center), "gray at zoom 13: {center}");
    }

    #[test]
    fn polygon_outlines_skip_where_the_tile_cut_them() {
        use super::super::mvt::Layer;
        use serde_json::json;
        // A polygon covering the whole tile and its buffer: its ring is all
        // tile cuts, so no outline belongs in this tile.
        let square = VectorTile {
            layers: vec![Layer::test_square("water", -512, 4608)],
        };
        let outline = json!({"id": "edge", "type": "line", "source-layer": "water",
            "paint": {"line-color": "#fff", "line-width": 4}});
        let drawn = render_maplibre(outline, &square);
        assert_eq!(drawn.pixels().iter().filter(|p| p.red() > 0).count(), 0);
        let x = clip_line_to_tile(&[(-0.5, 0.5), (0.5, 0.5), (0.5, 1.5)]);
        assert_eq!(x, vec![vec![(0.0, 0.5), (0.5, 0.5), (0.5, 1.0)]]);
    }

    #[test]
    fn translucent_fills_keep_to_their_tile() {
        use super::super::mvt::Layer;
        use serde_json::json;
        // A square reaching past the tile edge (into the buffer).
        let square = VectorTile {
            layers: vec![Layer::test_square("water", -512, 4608)],
        };
        // Draw the same tile twice, one tile apart: the overlap must not
        // blend twice along the shared edge.
        let doc = json!({"version": 8, "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000"}},
            {"id": "w", "type": "fill", "source-layer": "water",
             "paint": {"fill-color": "#fff", "fill-opacity": 0.5}}]});
        let style = MapStyle::from_json(&doc.to_string()).unwrap();
        let next = TilePlacement {
            id: TileId {
                zoom: 14,
                x: 1,
                y: 0,
            },
            dx: 128.0,
            ..placement(256.0)
        };
        let mut both = Pixmap::new(256, 256).unwrap();
        let first = TilePlacement {
            id: TileId {
                zoom: 14,
                x: 0,
                y: 0,
            },
            dx: -128.0,
            ..placement(256.0)
        };
        draw_vector_map(&mut both, &[(first, &square), (next, &square)], &style, 1.0);
        let reds: Vec<u8> = (60..200)
            .map(|x| both.pixel(x, 128).unwrap().red())
            .collect();
        let (lo, hi) = (reds.iter().min().unwrap(), reds.iter().max().unwrap());
        assert!(hi - lo <= 2, "uniform across the tile edge: {reds:?}");
    }

    #[test]
    fn odd_dash_patterns_run_into_the_next_period() {
        use serde_json::json;
        let three = road_pixels(road(
            json!({"line-color": "#fff", "line-width": 4, "line-dasharray": [1, 1, 1]}),
            json!({"layout": {"line-cap": "butt"}}),
        ));
        let solid = road_pixels(road(
            json!({"line-color": "#fff", "line-width": 4}),
            json!({}),
        ));
        // dash 2, gap 1 per 3 widths: two thirds drawn.
        let share = three as f64 / solid as f64;
        assert!((share - 2.0 / 3.0).abs() < 0.05, "{share}");
    }

    /// Pixels a MapLibre line style draws over a horizontal road across a
    /// 256 px tile at world zoom 14 (MapLibre zoom 13).
    fn road_pixels(layer: serde_json::Value) -> usize {
        use super::super::mvt::{Layer, Value};
        let doc = serde_json::json!({
            "version": 8,
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#000"}},
                layer
            ]
        });
        let style = MapStyle::from_json(&doc.to_string()).unwrap();
        let tile = VectorTile {
            layers: vec![Layer::test_line(
                "transportation",
                0,
                4096,
                2048,
                &[("class", Value::String("primary".into()))],
            )],
        };
        let mut target = Pixmap::new(256, 256).unwrap();
        let at_zoom_14 = TilePlacement {
            id: TileId {
                zoom: 14,
                x: 0,
                y: 0,
            },
            ..placement(256.0)
        };
        draw_vector_map(&mut target, &[(at_zoom_14, &tile)], &style, 1.0);
        target.pixels().iter().filter(|p| p.red() > 0).count()
    }

    fn road(paint: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        let mut layer = serde_json::json!({
            "id": "road", "type": "line", "source-layer": "transportation", "paint": paint
        });
        if let (Some(l), Some(e)) = (layer.as_object_mut(), extra.as_object()) {
            l.extend(e.clone());
        }
        layer
    }

    #[test]
    fn maplibre_widths_dashes_zooms_and_filters_apply() {
        use serde_json::json;
        let solid = road_pixels(road(
            json!({"line-color": "#fff", "line-width": 4}),
            json!({}),
        ));
        assert!((900..1100).contains(&solid), "4 px × 256 px: {solid}");
        let curve = json!(["interpolate", ["linear"], ["zoom"], 12, 2, 14, 10]);
        let wide = road_pixels(road(
            json!({"line-color": "#fff", "line-width": curve}),
            json!({}),
        ));
        assert_eq!(wide * 2, solid * 3, "6 px against 4 px at zoom 13");
        let dashed = road_pixels(road(
            json!({"line-color": "#fff", "line-width": 4, "line-dasharray": [1, 1]}),
            json!({}),
        ));
        assert!(
            dashed > solid / 3 && dashed < solid * 2 / 3,
            "half dashed: {dashed}"
        );
        let out_of_range = json!({"minzoom": 14});
        let hidden = road(json!({"line-color": "#fff", "line-width": 4}), out_of_range);
        assert_eq!(road_pixels(hidden), 0);
        let other = json!({"filter": ["==", ["get", "class"], "minor"]});
        let filtered = road(json!({"line-color": "#fff", "line-width": 4}), other);
        assert_eq!(road_pixels(filtered), 0);
    }

    #[test]
    fn missing_layers_are_skipped() {
        let mut target = Pixmap::new(64, 64).unwrap();
        let tile = VectorTile::default();
        draw_vector_map(
            &mut target,
            &[(placement(64.0), &tile)],
            &style(Color::WHITE),
            1.0,
        );
        assert_eq!(target.pixel(32, 32).unwrap().red(), 0);
    }
}
