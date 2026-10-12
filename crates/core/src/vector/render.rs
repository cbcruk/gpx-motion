//! Rasterizing vector tiles with a [`MapStyle`], flat or as a ground plane
//! under a perspective camera.

use tiny_skia::{
    BlendMode, Color, FillRule, LineCap, LineJoin, Paint as SkPaint, PathBuilder, Pixmap,
    PixmapPaint, Stroke, StrokeDash, Transform,
};

use super::mvt::{GeomType, Layer, VectorTile};
use super::style::{Cap, Join, MapStyle, Paint, Rule};
use crate::color::scale_alpha;
use crate::geo::camera::{Camera, ViewPoint, NEAR};
use crate::geo::clip::{self, clip_ring_to_rect, clip_segment, Edges};
use crate::geo::mercator::TILE_SIZE;
use crate::tiles::TilePlacement;

/// Rasterize vector tiles onto `target` with `style`, blended at `opacity`.
///
/// Rules are drawn in style order across every tile, so a road in one tile is
/// never painted under water from its neighbor. Tile buffers overlap at the
/// edges, which is invisible with opaque style colors. Overzoomed placements
/// (`size > 256`) scale the vector geometry, so it stays sharp at any zoom.
///
/// `finer` is a finer tile zoom's tiles fading in over `tiles` with their
/// opacity (see [`crate::geo::mercator::tile_levels`]); the two maps blend
/// before the whole is drawn at `opacity`.
pub fn draw_vector_map(
    target: &mut Pixmap,
    tiles: &[(TilePlacement, &VectorTile)],
    finer: Option<(&[(TilePlacement, &VectorTile)], f32)>,
    style: &MapStyle,
    opacity: f32,
) {
    let flat = |p: &TilePlacement, x: f64, y: f64| ViewPoint {
        x: p.dx + x * p.size,
        y: p.dy + y * p.size,
        depth: NEAR,
    };
    let screen = |v: ViewPoint| (v.x, v.y);
    let zoom = |p: &TilePlacement| f64::from(p.id.zoom) + (p.size / TILE_SIZE).log2();
    let widths = Widths {
        zoom,
        depth: None,
        unit: 1.0,
    };
    let draw = |tiles: &[_]| paint_layers(target, tiles, style, flat, screen, &widths, false);
    let Some(base) = draw(tiles) else {
        return;
    };
    let finer = finer.and_then(|(tiles, mix)| Some((draw(tiles)?, mix)));
    composite(target, base, finer, opacity);
}

/// Rasterize vector tiles as a ground plane seen through a perspective camera.
///
/// `tiles` carry world-pixel placements from [`Camera::ground_tiles`].
/// Geometry is clipped at the camera's near plane before projection, so large
/// polygons reaching behind the camera still fill correctly. Line widths follow
/// the camera zoom and shrink with distance (and grow up close) relative to
/// the camera target. `finer` fades in a finer tile zoom as in
/// [`draw_vector_map`]. Extrusion rules are left out: a 3D view raises them
/// (see [`crate::draw_buildings`]).
pub fn draw_vector_map_3d(
    target: &mut Pixmap,
    tiles: &[(TilePlacement, &VectorTile)],
    finer: Option<(&[(TilePlacement, &VectorTile)], f32)>,
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
        unit: 2f64.powf(camera.zoom - camera.detail_zoom),
    };
    let draw = |tiles: &[_]| paint_layers(target, tiles, style, ground, screen, &widths, true);
    let Some(base) = draw(tiles) else {
        return;
    };
    let finer = finer.and_then(|(tiles, mix)| Some((draw(tiles)?, mix)));
    composite(target, base, finer, opacity);
}

/// Draw the map layer `base`, with `finer` blended over it at its opacity,
/// onto `target` at `opacity`.
fn composite(target: &mut Pixmap, mut base: Pixmap, finer: Option<(Pixmap, f32)>, opacity: f32) {
    let draw = |onto: &mut Pixmap, layer: &Pixmap, opacity: f32| {
        onto.draw_pixmap(
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
    };
    if let Some((finer, mix)) = finer {
        draw(&mut base, &finer, mix);
    }
    draw(target, &base, opacity);
}

/// Line width multipliers are rounded to steps of `1 / WIDTH_STEPS`, so lines
/// at similar depths share one path.
const WIDTH_STEPS: f32 = 8.0;
/// Range of the depth-based line width multiplier.
const WIDTH_SCALE: (f32, f32) = (0.25, 1.5);

/// How line widths scale in [`paint_layers`].
struct Widths<Z> {
    /// World zoom a tile's lines are drawn at, for [`MapStyle::line_scale`].
    zoom: Z,
    /// Depth at which lines keep their width (the camera distance); lines
    /// scale inversely with depth around it. `None` keeps widths flat.
    depth: Option<f64>,
    /// Units of tile placements (`dx`, `dy`, `size`) per device pixel of
    /// the map at its zoom, for offsets given in pixels.
    unit: f64,
}

/// Shared rule loop, painting a layer the size of `target`: `to_view` maps
/// tile-normalized coordinates (`0..=1`) to camera space, and `to_screen`
/// maps unclipped camera space to pixels. With `raised`, extrusion rules
/// are skipped.
fn paint_layers<Z: Fn(&TilePlacement) -> f64>(
    target: &Pixmap,
    tiles: &[(TilePlacement, &VectorTile)],
    style: &MapStyle,
    to_view: impl Fn(&TilePlacement, f64, f64) -> ViewPoint,
    to_screen: impl Fn(ViewPoint) -> (f64, f64),
    widths: &Widths<Z>,
    raised: bool,
) -> Option<Pixmap> {
    let painter = LayerPainter {
        style,
        to_view,
        to_screen,
        widths,
        viewport: (target.width() as f32, target.height() as f32),
    };
    let mut layer = Pixmap::new(target.width(), target.height())?;
    layer.fill(style.background);
    for rule in style
        .rules
        .iter()
        .filter(|r| !raised || r.extrude.is_none())
    {
        for (placement, tile) in tiles {
            let Some(source) = tile.layer(&rule.layer) else {
                continue;
            };
            painter.paint_tile(&mut layer, rule, placement, source);
        }
    }
    Some(layer)
}

/// Number of line width buckets in [`paint_layers`] (bucket `0` holds fills).
fn width_buckets() -> usize {
    (WIDTH_SCALE.1 * WIDTH_STEPS) as usize
}

/// How a rule draws on one tile at its zoom.
struct RuleLook {
    paint: Paint,
    /// Whether the paint is see-through, so it must keep to its own tile.
    translucent: bool,
    /// Line width multiplier at the tile's zoom.
    tile_scale: f32,
    /// Line blur in device pixels.
    blur: f32,
    /// Gap inside a casing line, in device pixels.
    gap: f32,
    /// How far past the viewport a line may reach and still show.
    margin: f32,
    /// Whether edges are anti-aliased (fills may turn it off).
    anti_alias: bool,
    /// A fill's offset, in tile-normalized units of the placement.
    shift: (f64, f64),
}

/// The projection and line widths [`paint_layers`] draws tiles with.
struct LayerPainter<'a, V, S, Z> {
    style: &'a MapStyle,
    to_view: V,
    to_screen: S,
    widths: &'a Widths<Z>,
    /// Size of the layer in pixels.
    viewport: (f32, f32),
}

impl<V, S, Z> LayerPainter<'_, V, S, Z>
where
    V: Fn(&TilePlacement, f64, f64) -> ViewPoint,
    S: Fn(ViewPoint) -> (f64, f64),
    Z: Fn(&TilePlacement) -> f64,
{
    /// Draw `rule` for the features of `source`, one tile's layer, onto
    /// `layer`.
    fn paint_tile(
        &self,
        layer: &mut Pixmap,
        rule: &Rule,
        placement: &TilePlacement,
        source: &Layer,
    ) {
        let Some(look) = self.look(rule, placement) else {
            return;
        };
        let builders = self.trace(rule, placement, source, &look);
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
                    anti_alias: look.anti_alias,
                    blur: look.blur * scale,
                    gap: look.gap * scale,
                };
                paint_path(layer, &path, look.paint, scale * look.tile_scale, line);
            }
        }
    }

    /// How `rule` draws on the tile at `placement`, or `None` when it
    /// doesn't draw there at all.
    fn look(&self, rule: &Rule, placement: &TilePlacement) -> Option<RuleLook> {
        let style = self.style;
        let zoom = (self.widths.zoom)(placement);
        let maplibre_zoom = style.maplibre_zoom(zoom);
        if !rule.draws_at(maplibre_zoom) {
            return None;
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
            return None;
        }
        // Neighbouring tiles overlap by a buffer; translucent paint would
        // blend twice there, so it keeps to its own tile. Polygon outlines
        // always do: their rings run along where the tile cut them.
        let translucent = alpha(paint) < 1.0;
        let tile_scale = style
            .curve_width(rule, zoom)
            .unwrap_or_else(|| style.line_scale(zoom));
        // Pixel extras in device pixels at the map's own scale.
        let pixels = |curve: &Option<crate::vector::expr::Curve>| {
            curve.as_ref().map_or(0.0, |c| {
                c.at(maplibre_zoom).max(0.0) as f32 * style.pixel_ratio
            })
        };
        let (blur, gap) = (pixels(&rule.blur), pixels(&rule.gap));
        let margin = match rule.paint {
            Paint::Fill(_) => 0.0,
            Paint::Line { width, .. } => {
                let sides = if gap > 0.0 { 2.0 } else { 1.0 };
                (sides * width * tile_scale + gap) * WIDTH_SCALE.1 / 2.0 + 1.0
            }
        };
        let anti_alias = rule
            .antialias
            .as_ref()
            .is_none_or(|a| a.at(maplibre_zoom) >= 0.5);
        let shift = rule.translate.as_ref().map_or((0.0, 0.0), |t| {
            let (x, y) = t.at(maplibre_zoom);
            let per = f64::from(style.pixel_ratio) * self.widths.unit / placement.size;
            (x * per, y * per)
        });
        Some(RuleLook {
            paint,
            translucent,
            tile_scale,
            blur,
            gap,
            margin,
            anti_alias,
            shift,
        })
    }

    /// Bucket of [`width_buckets`] a projected line run's width falls in, by
    /// its mean depth.
    fn bucket_of(&self, run: &[ViewPoint]) -> usize {
        let depth = run.iter().map(|v| v.depth).sum::<f64>() / run.len().max(1) as f64;
        let scale = self
            .widths
            .depth
            .map_or(1.0, |d| (d / depth.max(NEAR)) as f32)
            .clamp(WIDTH_SCALE.0, WIDTH_SCALE.1);
        ((scale * WIDTH_STEPS).round() as usize).clamp(1, width_buckets())
    }

    /// Screen paths of the features of `source` that `rule` draws: fills in
    /// the first, lines in the one for their width bucket.
    fn trace(
        &self,
        rule: &Rule,
        placement: &TilePlacement,
        source: &Layer,
        look: &RuleLook,
    ) -> Vec<PathBuilder> {
        let fill = matches!(rule.paint, Paint::Fill(_));
        let wanted = |g: GeomType| match g {
            GeomType::Polygon => fill || rule.outlines,
            GeomType::LineString => !fill,
            _ => false,
        };
        let viewport = Viewport {
            width: self.viewport.0,
            height: self.viewport.1,
            margin: look.margin,
        };
        let (shift, translucent) = (look.shift, look.translucent);
        let extent = f64::from(source.extent.max(1));
        let mut builders: Vec<PathBuilder> =
            (0..=width_buckets()).map(|_| PathBuilder::new()).collect();
        for feature in source.features.iter().filter(|f| wanted(f.geom_type)) {
            if !rule.matches(source, feature) {
                continue;
            }
            let polygon = feature.geom_type == GeomType::Polygon;
            for part in feature.parts() {
                let mut points: Vec<(f64, f64)> = part
                    .iter()
                    .map(|&(x, y)| (f64::from(x) / extent, f64::from(y) / extent))
                    .collect();
                let to_view = |points: &[(f64, f64)]| -> Vec<ViewPoint> {
                    points
                        .iter()
                        .map(|&(x, y)| (self.to_view)(placement, x + shift.0, y + shift.1))
                        .collect()
                };
                if fill {
                    if translucent {
                        points = clip_to_tile(&points);
                    }
                    let ring = clip_ring(&to_view(&points));
                    append(&mut builders[0], &ring, &self.to_screen, &viewport, true);
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
                        let builder = &mut builders[self.bucket_of(&run)];
                        append(builder, &run, &self.to_screen, &viewport, false);
                    }
                }
            }
        }
        builders
    }
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
    let fade = |c: Color| scale_alpha(c, opacity.clamp(0.0, 1.0));
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
    clip_ring_to_rect(ring, [0.0, 0.0, 1.0, 1.0])
}

/// The parts of a polyline inside the tile square (`0..=1`), each a run of
/// points (Liang–Barsky per segment).
///
/// The square is [`Edges::Closed`]: a line along the edge two tiles share
/// is stroked by both rather than by neither.
fn clip_line_to_tile(points: &[(f64, f64)]) -> Vec<Vec<(f64, f64)>> {
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut open = false;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let Some((t0, t1)) = clip_segment(a, b, [0.0, 0.0, 1.0, 1.0], Edges::Closed) else {
            open = false;
            continue;
        };
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
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
    /// Whether to antialias (fills may turn it off).
    anti_alias: bool,
    /// Device pixels in from a line's edges over which it fades out.
    blur: f32,
    /// Device pixels left open down a line's middle, between two lines.
    gap: f32,
}

fn paint_path(
    layer: &mut Pixmap,
    path: &tiny_skia::Path,
    paint: Paint,
    width_scale: f32,
    line: LineLook,
) {
    let mut sk = SkPaint {
        anti_alias: line.anti_alias,
        ..Default::default()
    };
    match paint {
        Paint::Fill(color) => {
            sk.set_color(color);
            layer.fill_path(path, &sk, FillRule::Winding, Transform::identity(), None);
        }
        Paint::Line { color, width } => {
            let width = width * width_scale;
            if width <= 0.0 {
                return;
            }
            sk.set_color(blurred(color, width, line.blur));
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
            // Lines either side of a gap end flat: a path ends at every tile
            // cut, where a round or square cap would arc across the gap.
            let cap = if line.gap > 0.0 {
                Some(Cap::Butt)
            } else {
                line.cap
            };
            let stroke = |width: f32, dash: Option<StrokeDash>| Stroke {
                width,
                line_cap: match cap {
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
            if line.gap <= 0.0 {
                layer.stroke_path(path, &sk, &stroke(width, dash), Transform::identity(), None);
                return;
            }
            // Two lines either side of the gap: one wide line with its
            // middle cleared, drawn apart from the layer so the clearing
            // leaves what is under it.
            let outer = line.gap + 2.0 * width;
            let Some((x0, y0, mut part)) = scratch(layer, path, outer) else {
                return;
            };
            let to_part = Transform::from_translate(-(x0 as f32), -(y0 as f32));
            part.stroke_path(path, &sk, &stroke(outer, dash), to_part, None);
            let clear = SkPaint {
                blend_mode: BlendMode::Clear,
                anti_alias: true,
                ..Default::default()
            };
            part.stroke_path(path, &clear, &stroke(line.gap, None), to_part, None);
            layer.draw_pixmap(
                x0,
                y0,
                part.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }
}

/// `color` faded by how much of a line `width` pixels wide a `blur` (from
/// its edges in) leaves: MapLibre ramps the line's alpha over `blur`
/// pixels inside each edge; this spreads the same total over the line.
fn blurred(color: Color, width: f32, blur: f32) -> Color {
    let half = width / 2.0;
    if blur <= 0.0 || half <= 0.0 {
        return color;
    }
    let kept = if blur <= half {
        1.0 - blur / (2.0 * half)
    } else {
        half / (2.0 * blur)
    };
    let mut faded = color;
    faded.set_alpha(color.alpha() * kept);
    faded
}

/// A blank pixmap covering `path` widened by a `width` stroke, clipped to
/// `layer`, with its top-left corner in the layer.
fn scratch(layer: &Pixmap, path: &tiny_skia::Path, width: f32) -> Option<(i32, i32, Pixmap)> {
    let bounds = path.bounds();
    let pad = width + 2.0;
    let x0 = (bounds.left() - pad).floor().max(0.0) as i32;
    let y0 = (bounds.top() - pad).floor().max(0.0) as i32;
    let x1 = ((bounds.right() + pad).ceil() as i32).min(layer.width() as i32);
    let y1 = ((bounds.bottom() + pad).ceil() as i32).min(layer.height() as i32);
    let part = Pixmap::new((x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32)?;
    Some((x0, y0, part))
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
pub(crate) fn clip_ring(ring: &[ViewPoint]) -> Vec<ViewPoint> {
    clip::clip_ring(ring, |v| v.depth >= NEAR, near_crossing)
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
        draw_vector_map(&mut target, &[], None, &style(Color::WHITE), 1.0);
        let px = target.pixel(5, 5).unwrap();
        assert_eq!((px.red(), px.green(), px.blue()), (0, 0, 0));
    }

    #[test]
    fn zero_opacity_keeps_target() {
        let mut target = Pixmap::new(64, 64).unwrap();
        target.fill(Color::WHITE);
        draw_vector_map(&mut target, &[], None, &style(Color::WHITE), 0.0);
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
        let at_zoom_14 = TilePlacement::whole(TileId {
            zoom: 14,
            x: 0,
            y: 0,
        });
        draw_vector_map(&mut target, &[(at_zoom_14, tile)], None, &style, 1.0);
        target
    }

    #[test]
    fn extrusions_lie_flat_in_2d_and_leave_3d_ground() {
        use super::super::extrusion::{Extrusion, Height};
        use super::super::mvt::Layer;
        use crate::geo::camera::{Camera, Lens};
        let tile = VectorTile {
            layers: vec![Layer::test_square("building", 0, 4096)],
        };
        let mut style = style(Color::WHITE);
        style.rules[0].layer = "building".into();
        style.rules[0].extrude = Some(Extrusion {
            height: Height::Meters(10.0),
            base: Height::Meters(0.0),
            gradient: true,
        });
        let mut flat = Pixmap::new(64, 64).unwrap();
        draw_vector_map(&mut flat, &[(placement(64.0), &tile)], None, &style, 1.0);
        assert_eq!(flat.pixel(32, 32).unwrap().red(), 255);
        let lens = Lens {
            pitch_deg: 0.0,
            ..Lens::default()
        };
        let camera = Camera::new(0.0, 0.0, 0.0, 64, 64, lens);
        let (x, y) = camera.target();
        let around = TilePlacement {
            dx: x - 32.0,
            dy: y - 32.0,
            ..placement(64.0)
        };
        let mut ground = Pixmap::new(64, 64).unwrap();
        draw_vector_map_3d(&mut ground, &[(around, &tile)], None, &style, &camera, 1.0);
        assert_eq!(
            ground.pixel(32, 32).unwrap().red(),
            0,
            "the background only"
        );
    }

    #[test]
    fn maplibre_fill_antialias_and_translate_apply() {
        use super::super::mvt::Layer;
        use serde_json::json;
        // A square with edges mid-pixel, at x 62.5 and 187.5.
        let square = VectorTile {
            layers: vec![Layer::test_square("landuse", 1000, 3000)],
        };
        let fill = |paint: serde_json::Value| {
            let layer =
                json!({"id": "f", "type": "fill", "source-layer": "landuse", "paint": paint});
            render_maplibre(layer, &square)
        };
        let red = |p: &Pixmap, x: u32| p.pixel(x, 128).unwrap().red();
        let smooth = fill(json!({"fill-color": "#fff"}));
        assert!((1..255).contains(&red(&smooth, 62)), "blended edge");
        let sharp = fill(json!({"fill-color": "#fff", "fill-antialias": false}));
        assert!(matches!(red(&sharp, 62), 0 | 255), "hard edge");
        // Antialiasing back on past zoom 12 (this view is at 13).
        let stepped = fill(json!({"fill-color": "#fff",
            "fill-antialias": ["step", ["zoom"], false, 12, true]}));
        assert_eq!(red(&stepped, 62), red(&smooth, 62));
        // Moved 10 CSS pixels right and down.
        let moved = fill(json!({"fill-color": "#fff", "fill-translate": [10, 10]}));
        assert_eq!((red(&smooth, 66), red(&moved, 66)), (255, 0));
        assert_eq!((red(&smooth, 195), red(&moved, 195)), (0, 255));
    }

    #[test]
    fn maplibre_line_gap_and_blur_apply() {
        use super::super::mvt::{Layer, Value};
        use serde_json::json;
        let props = [("class", Value::String("primary".into()))];
        let road = VectorTile {
            layers: vec![Layer::test_line("transportation", 0, 4096, 2048, &props)],
        };
        let line = |paint: serde_json::Value| {
            let layer = json!({"id": "l", "type": "line", "source-layer": "transportation",
                "paint": paint});
            render_maplibre(layer, &road)
        };
        let red = |p: &Pixmap, y: u32| p.pixel(128, y).unwrap().red();
        let plain = line(json!({"line-color": "#fff", "line-width": 4}));
        assert_eq!(red(&plain, 128), 255);
        // Two 4 px lines either side of a 6 px gap.
        let cased = line(json!({"line-color": "#fff", "line-width": 4, "line-gap-width": 6}));
        assert_eq!(red(&cased, 128), 0, "open middle");
        assert_eq!((red(&cased, 123), red(&cased, 133)), (255, 255));
        assert_eq!((red(&cased, 119), red(&cased, 137)), (0, 0));
        // Round ends would arc across the gap where tiles cut the line.
        let short = VectorTile {
            layers: vec![Layer::test_line("transportation", 1024, 3072, 2048, &props)],
        };
        let layer = json!({"id": "l", "type": "line", "source-layer": "transportation",
            "layout": {"line-cap": "round"},
            "paint": {"line-color": "#fff", "line-width": 4, "line-gap-width": 6}});
        let ends = render_maplibre(layer, &short);
        assert_eq!(
            ends.pixel(197, 128).unwrap().red(),
            0,
            "no arc past the end"
        );
        // Blurred 2 px in from each edge of a 4 px line: half as strong.
        let soft = line(json!({"line-color": "#fff", "line-width": 4, "line-blur": 2}));
        assert!(red(&soft, 128).abs_diff(128) <= 2, "{}", red(&soft, 128));
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
        draw_vector_map(
            &mut both,
            &[(first, &square), (next, &square)],
            None,
            &style,
            1.0,
        );
        let reds: Vec<u8> = (60..200)
            .map(|x| both.pixel(x, 128).unwrap().red())
            .collect();
        let (lo, hi) = (reds.iter().min().unwrap(), reds.iter().max().unwrap());
        assert!(hi - lo <= 2, "uniform across the tile edge: {reds:?}");
    }

    #[test]
    fn a_finer_level_blends_in_before_the_map_opacity() {
        use super::super::mvt::Layer;
        let square = VectorTile {
            layers: vec![Layer::test_square("water", -512, 4608)],
        };
        let base: [(TilePlacement, &VectorTile); 0] = [];
        let finer = [(placement(256.0), &square)];
        let red = |opacity: f32| {
            let mut target = Pixmap::new(64, 64).unwrap();
            target.fill(Color::WHITE);
            let mix = Some((&finer[..], 0.5));
            draw_vector_map(&mut target, &base, mix, &style(Color::WHITE), opacity);
            target.pixel(32, 32).unwrap().red()
        };
        // Halfway from the black background to the white square, then that
        // over white at half opacity.
        assert!(red(1.0).abs_diff(128) <= 1, "{}", red(1.0));
        assert!(red(0.5).abs_diff(191) <= 1, "{}", red(0.5));
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
        let at_zoom_14 = TilePlacement::whole(TileId {
            zoom: 14,
            x: 0,
            y: 0,
        });
        draw_vector_map(&mut target, &[(at_zoom_14, &tile)], None, &style, 1.0);
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
            None,
            &style(Color::WHITE),
            1.0,
        );
        assert_eq!(target.pixel(32, 32).unwrap().red(), 0);
    }
}
