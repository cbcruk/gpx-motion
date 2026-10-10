use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint as SkPaint, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};

use super::mvt::{GeomType, VectorTile};
use super::style::{MapStyle, Paint};
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
        let want = match rule.paint {
            Paint::Fill(_) => GeomType::Polygon,
            Paint::Line { .. } => GeomType::LineString,
        };
        for (placement, tile) in tiles {
            let Some(source) = tile.layer(&rule.layer) else {
                continue;
            };
            let tile_scale = style.line_scale((widths.zoom)(placement));
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
            for feature in source.features.iter().filter(|f| f.geom_type == want) {
                let class = source.property(feature, "class").and_then(|v| v.as_str());
                if !rule.accepts(class) {
                    continue;
                }
                for part in feature.parts() {
                    let view: Vec<ViewPoint> = part
                        .iter()
                        .map(|&(x, y)| to_view(placement, x as f64 / extent, y as f64 / extent))
                        .collect();
                    if want == GeomType::Polygon {
                        let ring = clip_ring(&view);
                        append(&mut builders[0], &ring, &to_screen, &viewport, true);
                    } else {
                        for run in clip_line(&view) {
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
                    paint_path(&mut layer, &path, rule.paint, scale * tile_scale);
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

fn paint_path(layer: &mut Pixmap, path: &tiny_skia::Path, paint: Paint, width_scale: f32) {
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
            let stroke = Stroke {
                width: width * width_scale,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Default::default()
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
            rules: vec![Rule {
                layer: "water".into(),
                classes: Vec::new(),
                paint: Paint::Fill(fill),
            }],
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
