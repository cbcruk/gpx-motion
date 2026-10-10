//! Point labels for place, peak, and water names: chosen by importance,
//! kept clear of each other, the track, and screen overlays, and drawn with a
//! halo.

use tiny_skia::{Color, Pixmap, Rect};

use super::mvt::{GeomType, Layer, VectorTile};
use crate::geo::camera::NEAR;
use crate::geo::Camera;
use crate::render::TextFont;
use crate::tiles::TilePlacement;

/// The OpenMapTiles layers labels come from; decode just these with
/// [`VectorTile::decode_layers`] when only labels are needed.
pub const LABEL_LAYERS: [&str; 3] = ["place", "mountain_peak", "water_name"];

/// 3D labels farther than this many times the camera distance are dropped
/// (they would sit in the horizon fog).
const LABEL_FAR_FACTOR: f64 = 2.0;

/// Most labels drawn in one view, so dense cities stay readable.
const MAX_LABELS: usize = 60;
/// Same-named labels closer than this many CSS pixels are duplicates (tile
/// buffers repeat features near tile edges).
const DUPLICATE_PX: f32 = 200.0;
/// Clear space kept around each label (beyond its halo), in CSS pixels.
const PADDING_PX: f32 = 3.0;
/// Labels that would draw smaller than this many CSS pixels are skipped.
const MIN_SIZE_PX: f32 = 8.0;
/// Size factor at and above which a label is fully opaque; it fades to
/// [`FAR_OPACITY`] as the factor drops to [`FAR_SCALE`] (distant 3D labels).
const NEAR_SCALE: f32 = 0.9;
const FAR_SCALE: f32 = 0.6;
const FAR_OPACITY: f32 = 0.35;

/// How map labels look and which name they show.
#[derive(Debug, Clone)]
pub struct LabelStyle {
    /// Font with glyphs for the labels' script (e.g. a Korean font); labels
    /// it can't render are skipped.
    pub font: TextFont,
    /// Base text size in CSS pixels; cities draw larger, neighbourhoods
    /// smaller.
    pub size: f32,
    /// Text color.
    pub color: Color,
    /// Outline drawn around the text for legibility over the map.
    pub halo: Color,
    /// Prefer the `name:<language>` property (e.g. `ko`, `en`) over `name`.
    pub language: Option<String>,
    /// Device pixels per CSS pixel.
    pub pixel_ratio: f32,
}

impl LabelStyle {
    /// Light text with a dark halo over a dark map `background`, or the
    /// reverse over a light one.
    pub fn contrasting(
        font: TextFont,
        size: f32,
        background: Color,
        language: Option<String>,
        pixel_ratio: f32,
    ) -> Self {
        let luminance =
            0.2126 * background.red() + 0.7152 * background.green() + 0.0722 * background.blue();
        let (color, halo) = if luminance < 0.5 {
            (
                Color::from_rgba8(0xe6, 0xe6, 0xe6, 0xff),
                Color::from_rgba8(0x10, 0x10, 0x10, 0xb0),
            )
        } else {
            (
                Color::from_rgba8(0x33, 0x33, 0x33, 0xff),
                Color::from_rgba8(0xff, 0xff, 0xff, 0xc0),
            )
        };
        Self {
            font,
            size,
            color,
            halo,
            language,
            pixel_ratio,
        }
    }
}

/// A label placed on screen, ready for [`draw_labels`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLabel {
    /// The name drawn.
    pub text: String,
    /// Left edge of the text in pixels.
    pub x: f32,
    /// Top edge of the text in pixels.
    pub y: f32,
    /// Text size in pixels.
    pub size: f32,
    /// `0..=1`; distant 3D labels fade.
    pub opacity: f32,
}

/// What labels must not cover.
#[derive(Debug, Clone, Default)]
pub struct Keepout {
    /// Screen rectangles (overlays, panels).
    pub rects: Vec<Rect>,
    /// Track polylines in screen points, widened by `track_half_width`.
    pub tracks: Vec<Vec<(f32, f32)>>,
    /// Half the drawn track width plus a margin, in pixels.
    pub track_half_width: f32,
    /// Filled screen polygons (e.g. 3D curtains under the track); a label
    /// may neither cross their edges nor sit inside them.
    pub areas: Vec<Vec<(f32, f32)>>,
}

impl Keepout {
    /// Keep labels off overlay `rects`, the drawn `tracks` (screen polylines
    /// `track_width` wide, plus a margin), and filled `areas` such as 3D
    /// curtains.
    pub fn around(
        rects: &[Rect],
        tracks: Vec<Vec<(f32, f32)>>,
        areas: Vec<Vec<(f32, f32)>>,
        track_width: f32,
        pixel_ratio: f32,
    ) -> Self {
        Self {
            rects: rects.to_vec(),
            tracks,
            track_half_width: track_width / 2.0 + 2.0 * pixel_ratio,
            areas,
        }
    }
}

/// One labelled point from a tile, before it is placed on screen.
#[derive(Debug, Clone)]
struct LabelSource {
    text: String,
    priority: (u8, f64),
    size_factor: f32,
    placement: TilePlacement,
    /// Tile-normalized position (`0..=1`).
    point: (f64, f64),
}

/// The labelled points of some tiles, most important first: everything about
/// the labels except where the view puts them, so a moving view can reuse it
/// across frames instead of reading the tiles again.
#[derive(Debug, Clone, Default)]
pub struct LabelSources(Vec<LabelSource>);

struct Candidate<'a> {
    source: &'a LabelSource,
    anchor: (f32, f32),
    scale: f32,
}

/// Choose labels for places, peaks, and water bodies in `tiles` (OpenMapTiles
/// schema), most important first, skipping any that would overlap a placed
/// label, leave the canvas, or touch `keepout`.
///
/// `project` maps a tile placement and tile-normalized coordinates (`0..=1`)
/// to a screen point plus a size factor (1 in 2D; smaller with distance in
/// 3D, where far labels also fade), or `None` to drop the point (e.g. behind
/// the camera or in the fog). Equal-priority labels are ordered by name, so
/// placement is stable while the view moves slightly.
pub fn place_labels(
    tiles: &[(TilePlacement, &VectorTile)],
    style: &LabelStyle,
    project: impl Fn(&TilePlacement, f64, f64) -> Option<(f32, f32, f32)>,
    canvas: (f32, f32),
    keepout: &Keepout,
) -> Vec<PlacedLabel> {
    LabelSources::collect(tiles, style).place(style, project, canvas, keepout)
}

impl LabelSources {
    /// Read the place, peak, and water names in `tiles` (OpenMapTiles
    /// schema) that `style`'s font can draw, most important first; equal
    /// priorities are ordered by name.
    pub fn collect(tiles: &[(TilePlacement, &VectorTile)], style: &LabelStyle) -> Self {
        let mut sources = Vec::new();
        for (placement, tile) in tiles {
            for layer in &tile.layers {
                let kind = match layer.name.as_str() {
                    "place" | "mountain_peak" | "water_name" => layer.name.as_str(),
                    _ => continue,
                };
                collect(layer, kind, placement, style, &mut sources);
            }
        }
        sources.sort_by(|a, b| {
            a.priority
                .0
                .cmp(&b.priority.0)
                .then(a.priority.1.total_cmp(&b.priority.1))
                .then_with(|| a.text.cmp(&b.text))
        });
        Self(sources)
    }

    /// Place these labels for a view; see [`place_labels`] for `project`.
    pub fn place(
        &self,
        style: &LabelStyle,
        project: impl Fn(&TilePlacement, f64, f64) -> Option<(f32, f32, f32)>,
        canvas: (f32, f32),
        keepout: &Keepout,
    ) -> Vec<PlacedLabel> {
        let candidates = self.0.iter().filter_map(|source| {
            let (x, y, scale) = project(&source.placement, source.point.0, source.point.1)?;
            Some(Candidate {
                source,
                anchor: (x, y),
                scale,
            })
        });
        place_candidates(candidates, style, canvas, keepout)
    }

    /// [`LabelSources::place`] for ground tiles under `camera`; see
    /// [`place_labels_3d`].
    pub fn place_3d(
        &self,
        camera: &Camera,
        style: &LabelStyle,
        canvas: (u32, u32),
        keepout: &Keepout,
        ground: impl Fn(f64, f64) -> f64,
    ) -> Vec<PlacedLabel> {
        let project = |p: &TilePlacement, x: f64, y: f64| {
            let (wx, wy) = (p.dx + x * p.size, p.dy + y * p.size);
            let view = camera.view(wx, wy, ground(wx, wy));
            if view.depth > camera.distance * LABEL_FAR_FACTOR || view.depth < NEAR {
                return None;
            }
            let (sx, sy) = camera.to_screen(view);
            let scale = (camera.distance / view.depth).clamp(0.6, 1.2) as f32;
            Some((sx as f32, sy as f32, scale))
        };
        let size = (canvas.0 as f32, canvas.1 as f32);
        self.place(style, project, size, keepout)
    }
}

/// Greedily place `candidates` (most important first).
fn place_candidates<'a>(
    candidates: impl Iterator<Item = Candidate<'a>>,
    style: &LabelStyle,
    canvas: (f32, f32),
    keepout: &Keepout,
) -> Vec<PlacedLabel> {
    let ratio = style.pixel_ratio;
    let grid = KeepoutGrid::new(keepout, canvas);
    let mut placed: Vec<(PlacedLabel, Rect)> = Vec::new();
    for c in candidates {
        if placed.len() >= MAX_LABELS {
            break;
        }
        if placed.iter().any(|(p, b)| {
            p.text == c.source.text && distance(center(b), c.anchor) < DUPLICATE_PX * ratio
        }) {
            continue;
        }
        let size = style.size * c.source.size_factor * c.scale * ratio;
        if size < MIN_SIZE_PX * ratio {
            continue;
        }
        let w = style.font.measure(&c.source.text, size);
        let h = style.font.line_height(size);
        let (x, y) = (c.anchor.0 - w / 2.0, c.anchor.1 - h / 2.0);
        let pad = PADDING_PX * ratio + halo_radius(size);
        let Some(bounds) = Rect::from_xywh(x - pad, y - pad, w + 2.0 * pad, h + 2.0 * pad) else {
            continue;
        };
        let inside = bounds.left() >= 0.0
            && bounds.top() >= 0.0
            && bounds.right() <= canvas.0
            && bounds.bottom() <= canvas.1;
        if !inside
            || placed.iter().any(|(_, b)| overlaps(b, &bounds))
            || keepout.rects.iter().any(|r| overlaps(r, &bounds))
            || grid.touches(&bounds)
        {
            continue;
        }
        let fade = ((c.scale - FAR_SCALE) / (NEAR_SCALE - FAR_SCALE)).clamp(0.0, 1.0);
        placed.push((
            PlacedLabel {
                text: c.source.text.clone(),
                x,
                y,
                size,
                opacity: FAR_OPACITY + (1.0 - FAR_OPACITY) * fade,
            },
            bounds,
        ));
    }
    placed.into_iter().map(|(label, _)| label).collect()
}

/// [`place_labels`] for tiles already placed in screen pixels (a flat map).
pub fn place_labels_flat(
    tiles: &[(TilePlacement, &VectorTile)],
    style: &LabelStyle,
    canvas: (u32, u32),
    keepout: &Keepout,
) -> Vec<PlacedLabel> {
    let project = |p: &TilePlacement, x: f64, y: f64| {
        Some(((p.dx + x * p.size) as f32, (p.dy + y * p.size) as f32, 1.0))
    };
    let size = (canvas.0 as f32, canvas.1 as f32);
    place_labels(tiles, style, project, size, keepout)
}

/// [`place_labels`] for ground tiles (world-pixel placements) under `camera`,
/// standing on `ground` (world-pixel height at a world point), smaller with
/// distance and dropped toward the horizon fog.
pub fn place_labels_3d(
    tiles: &[(TilePlacement, &VectorTile)],
    camera: &Camera,
    style: &LabelStyle,
    canvas: (u32, u32),
    keepout: &Keepout,
    ground: impl Fn(f64, f64) -> f64,
) -> Vec<PlacedLabel> {
    LabelSources::collect(tiles, style).place_3d(camera, style, canvas, keepout, ground)
}

/// Draw placed labels with a halo so they read over any map.
pub fn draw_labels(pixmap: &mut Pixmap, labels: &[PlacedLabel], style: &LabelStyle) {
    for label in labels {
        let fade = |c: Color| {
            Color::from_rgba(c.red(), c.green(), c.blue(), c.alpha() * label.opacity).unwrap_or(c)
        };
        let (color, halo) = (fade(style.color), fade(style.halo));
        let r = halo_radius(label.size);
        let mut layers: Vec<(f32, f32, Color)> = [
            (-1.0, -1.0),
            (0.0, -1.0),
            (1.0, -1.0),
            (-1.0, 0.0),
            (1.0, 0.0),
            (-1.0, 1.0),
            (0.0, 1.0),
            (1.0, 1.0),
        ]
        .iter()
        .map(|(dx, dy)| (label.x + dx * r, label.y + dy * r, halo))
        .collect();
        layers.push((label.x, label.y, color));
        style
            .font
            .draw_layers(pixmap, &label.text, label.size, &layers);
    }
}

fn collect(
    layer: &Layer,
    kind: &str,
    placement: &TilePlacement,
    style: &LabelStyle,
    out: &mut Vec<LabelSource>,
) {
    let extent = layer.extent.max(1) as f64;
    for feature in layer
        .features
        .iter()
        .filter(|f| f.geom_type == GeomType::Point)
    {
        let text = style
            .language
            .as_ref()
            .and_then(|lang| layer.property(feature, &format!("name:{lang}")))
            .or_else(|| layer.property(feature, "name"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty() && style.font.covers(t));
        let Some(text) = text else {
            continue;
        };
        let class = layer.property(feature, "class").and_then(|v| v.as_str());
        let rank = layer
            .property(feature, "rank")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::MAX);
        let (tier, size_factor) = match (kind, class) {
            ("place", Some("country")) => (0, 1.4),
            ("place", Some("state" | "province")) => (1, 1.3),
            ("place", Some("city")) => (2, 1.35),
            ("place", Some("town")) => (3, 1.15),
            ("place", Some("suburb" | "village")) => (4, 1.0),
            ("water_name", _) => (5, 1.0),
            ("mountain_peak", _) => (6, 0.9),
            ("place", Some("quarter" | "neighbourhood" | "hamlet")) => (7, 0.9),
            _ => (8, 0.85),
        };
        for part in feature.parts() {
            for &(px, py) in &part {
                out.push(LabelSource {
                    text: text.to_string(),
                    priority: (tier, rank),
                    size_factor,
                    placement: *placement,
                    point: (px as f64 / extent, py as f64 / extent),
                });
            }
        }
    }
}

fn halo_radius(size: f32) -> f32 {
    (size / 10.0).max(1.0)
}

fn center(r: &Rect) -> (f32, f32) {
    ((r.left() + r.right()) / 2.0, (r.top() + r.bottom()) / 2.0)
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.left() < b.right() && b.left() < a.right() && a.top() < b.bottom() && b.top() < a.bottom()
}

/// Edge length in pixels of the [`KeepoutGrid`] cells.
const CELL_PX: f32 = 32.0;

/// A keepout's track segments and areas bucketed by the canvas cells their
/// bounding boxes cover (segments grown by the track half-width), so each
/// label tests only the shapes near it. For finite screen coordinates of a
/// sane size it gives the same answers as testing every shape, since a shape
/// can only touch a label its bounding box overlaps.
struct KeepoutGrid<'a> {
    keepout: &'a Keepout,
    cols: usize,
    rows: usize,
    /// Per cell: `(track, segment start)` pairs.
    segments: Vec<Vec<(usize, usize)>>,
    /// Per cell: area indices.
    areas: Vec<Vec<usize>>,
}

impl<'a> KeepoutGrid<'a> {
    fn new(keepout: &'a Keepout, canvas: (f32, f32)) -> Self {
        let cols = ((canvas.0 / CELL_PX).ceil() as usize).max(1);
        let rows = ((canvas.1 / CELL_PX).ceil() as usize).max(1);
        let mut grid = Self {
            keepout,
            cols,
            rows,
            segments: vec![Vec::new(); cols * rows],
            areas: vec![Vec::new(); cols * rows],
        };
        let w = keepout.track_half_width;
        for (t, track) in keepout.tracks.iter().enumerate() {
            for (i, s) in track.windows(2).enumerate() {
                let (a, b) = (s[0], s[1]);
                let bbox = (
                    a.0.min(b.0) - w,
                    a.1.min(b.1) - w,
                    a.0.max(b.0) + w,
                    a.1.max(b.1) + w,
                );
                for cell in grid.cells(bbox) {
                    grid.segments[cell].push((t, i));
                }
            }
        }
        for (i, area) in keepout.areas.iter().enumerate() {
            let bbox = area.iter().fold(
                (
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ),
                |(x0, y0, x1, y1), &(x, y)| (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            );
            for cell in grid.cells(bbox) {
                grid.areas[cell].push(i);
            }
        }
        grid
    }

    /// Indices of the cells overlapping the box `(x0, y0, x1, y1)`, clamped
    /// to the canvas: none for a box off it, every cell along an axis with
    /// a NaN corner (conservative).
    fn cells(&self, (x0, y0, x1, y1): (f32, f32, f32, f32)) -> Vec<usize> {
        let span = |lo: f32, hi: f32, n: usize| {
            let first = (lo / CELL_PX).floor().max(0.0);
            let last = (hi / CELL_PX).floor().min(n as f32 - 1.0);
            (first <= last).then_some((first as usize, last as usize))
        };
        let (Some((c0, c1)), Some((r0, r1))) = (span(x0, x1, self.cols), span(y0, y1, self.rows))
        else {
            return Vec::new();
        };
        (r0..=r1)
            .flat_map(|r| (c0..=c1).map(move |c| r * self.cols + c))
            .collect()
    }

    /// Whether a track (grown by its half-width) or an area touches `bounds`.
    fn touches(&self, bounds: &Rect) -> bool {
        let w = self.keepout.track_half_width;
        let grown = (
            bounds.left() - w,
            bounds.top() - w,
            bounds.right() + w,
            bounds.bottom() + w,
        );
        let rect = (bounds.left(), bounds.top(), bounds.right(), bounds.bottom());
        self.cells(rect).into_iter().any(|cell| {
            self.segments[cell].iter().any(|&(t, i)| {
                let track = &self.keepout.tracks[t];
                crosses(grown, track[i], track[i + 1])
            }) || self.areas[cell]
                .iter()
                .any(|&a| touches_area(bounds, &self.keepout.areas[a]))
        })
    }
}

/// Whether `bounds` crosses the polygon's edges or lies inside it.
fn touches_area(bounds: &Rect, area: &[(f32, f32)]) -> bool {
    if area.len() < 3 {
        return false;
    }
    let rect = (bounds.left(), bounds.top(), bounds.right(), bounds.bottom());
    let mut edges = area.iter().zip(area.iter().cycle().skip(1));
    edges.any(|(&a, &b)| crosses(rect, a, b)) || contains(area, center(bounds))
}

/// Even-odd point-in-polygon test.
fn contains(area: &[(f32, f32)], (px, py): (f32, f32)) -> bool {
    let mut inside = false;
    for (&(ax, ay), &(bx, by)) in area.iter().zip(area.iter().cycle().skip(1)) {
        if (ay > py) != (by > py) && px < ax + (py - ay) / (by - ay) * (bx - ax) {
            inside = !inside;
        }
    }
    inside
}

/// Whether segment `a`–`b` passes through the rectangle `(x0, y0, x1, y1)`
/// (Liang–Barsky clipping).
fn crosses((x0, y0, x1, y1): (f32, f32, f32, f32), a: (f32, f32), b: (f32, f32)) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [
        (-dx, a.0 - x0),
        (dx, x1 - a.0),
        (-dy, a.1 - y0),
        (dy, y1 - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::TileId;
    use crate::vector::mvt::{TestPoint, Value};

    fn style() -> LabelStyle {
        LabelStyle {
            font: TextFont::bundled().clone(),
            size: 14.0,
            color: Color::WHITE,
            halo: Color::BLACK,
            language: Some("en".into()),
            pixel_ratio: 1.0,
        }
    }

    fn place(name: &str, class: &str, x: i32, y: i32) -> TestPoint {
        (
            (x, y),
            vec![
                ("name", Value::String(name.to_string())),
                ("class", Value::String(class.to_string())),
            ],
        )
    }

    fn tile(points: &[TestPoint]) -> VectorTile {
        VectorTile {
            layers: vec![Layer::test_points("place", points)],
        }
    }

    fn placement() -> TilePlacement {
        TilePlacement {
            id: TileId {
                zoom: 0,
                x: 0,
                y: 0,
            },
            dx: 0.0,
            dy: 0.0,
            size: 512.0,
        }
    }

    fn run(tile: &VectorTile, keepout: &Keepout) -> Vec<PlacedLabel> {
        let tiles = [(placement(), tile)];
        let project = |p: &TilePlacement, x: f64, y: f64| {
            Some(((p.dx + x * p.size) as f32, (p.dy + y * p.size) as f32, 1.0))
        };
        place_labels(&tiles, &style(), project, (512.0, 512.0), keepout)
    }

    #[test]
    fn keepout_grid_matches_testing_every_shape() {
        // A small deterministic generator, so the test needs no dependency.
        let mut seed = 0x2545_f491_u32;
        let mut next = move |lo: f32, hi: f32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            lo + (seed as f32 / u32::MAX as f32) * (hi - lo)
        };
        let canvas = (300.0, 200.0);
        let tracks: Vec<Vec<(f32, f32)>> = (0..3)
            .map(|_| {
                (0..40)
                    .map(|_| (next(-50.0, 350.0), next(-50.0, 250.0)))
                    .collect()
            })
            .collect();
        let areas: Vec<Vec<(f32, f32)>> = (0..8)
            .map(|_| {
                let (x, y) = (next(-20.0, 320.0), next(-20.0, 220.0));
                (0..4)
                    .map(|_| (x + next(-15.0, 15.0), y + next(-15.0, 15.0)))
                    .collect()
            })
            .collect();
        let keepout = Keepout::around(&[], tracks, areas, 1.0, 0.5);
        let grid = KeepoutGrid::new(&keepout, canvas);
        let brute = |b: &Rect| {
            let w = keepout.track_half_width;
            let grown = (b.left() - w, b.top() - w, b.right() + w, b.bottom() + w);
            keepout
                .tracks
                .iter()
                .flat_map(|t| t.windows(2))
                .any(|s| crosses(grown, s[0], s[1]))
                || keepout.areas.iter().any(|a| touches_area(b, a))
        };
        let (mut hits, mut misses) = (0, 0);
        for _ in 0..2000 {
            let (w, h) = (next(1.0, 12.0), next(1.0, 8.0));
            let Some(b) = Rect::from_xywh(next(0.0, 300.0 - w), next(0.0, 200.0 - h), w, h) else {
                continue;
            };
            let expected = brute(&b);
            assert_eq!(grid.touches(&b), expected, "{b:?}");
            if expected {
                hits += 1;
            } else {
                misses += 1;
            }
        }
        assert!(hits > 100 && misses > 100, "{hits} hits, {misses} misses");
    }

    #[test]
    fn contrasting_colors_follow_the_background() {
        let font = TextFont::bundled().clone();
        let dark = LabelStyle::contrasting(font.clone(), 12.0, Color::BLACK, None, 1.0);
        assert!(dark.color.red() > 0.5 && dark.halo.red() < 0.5);
        let light = LabelStyle::contrasting(font, 12.0, Color::WHITE, None, 1.0);
        assert!(light.color.red() < 0.5 && light.halo.red() > 0.5);
    }

    #[test]
    fn straight_down_3d_labels_match_the_flat_ones() {
        let t = tile(&[
            place("Metropolis", "city", 2060, 2050),
            place("Elsewhere", "town", 600, 600),
            place("Smallville", "village", 3500, 3000),
        ]);
        let lens = crate::geo::Lens {
            pitch_deg: 0.0,
            bearing_deg: 0.0,
            fov_deg: 40.0,
        };
        let camera = Camera::new(0.0, 0.0, 1.0, 512, 512, lens);
        let tiles = [(placement(), &t)];
        let keepout = Keepout::default();
        let flat = place_labels_flat(&tiles, &style(), (512, 512), &keepout);
        let tilted = place_labels_3d(&tiles, &camera, &style(), (512, 512), &keepout, |_, _| 0.0);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat.len(), tilted.len());
        for (a, b) in flat.iter().zip(&tilted) {
            assert_eq!(a.text, b.text);
            assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3);
            assert_eq!(a.size, b.size);
        }
    }

    #[test]
    fn more_important_places_win_collisions() {
        let t = tile(&[
            place("Smallville", "village", 2048, 2048),
            place("Metropolis", "city", 2060, 2050),
            place("Elsewhere", "town", 600, 600),
        ]);
        let names: Vec<_> = run(&t, &Keepout::default())
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(names, vec!["Metropolis", "Elsewhere"]);
    }

    #[test]
    fn prefers_the_requested_language_and_skips_missing_glyphs() {
        let mut korean = place("서울", "city", 2048, 1000);
        korean.1.push(("name:en", Value::String("Seoul".into())));
        let t = tile(&[korean, place("부산", "city", 2048, 3000)]);
        let names: Vec<_> = run(&t, &Keepout::default())
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(names, vec!["Seoul"]);
    }

    #[test]
    fn keeps_clear_of_the_track_rects_and_edges() {
        let t = tile(&[
            place("Crossed", "city", 2048, 2048),
            place("Covered", "city", 2048, 400),
            place("Edge", "city", 10, 3000),
            place("Free", "city", 3000, 3000),
        ]);
        let keepout = Keepout {
            rects: vec![Rect::from_ltrb(0.0, 0.0, 512.0, 80.0).unwrap()],
            tracks: vec![vec![(0.0, 256.0), (512.0, 256.0)]],
            track_half_width: 2.0,
            areas: Vec::new(),
        };
        let names: Vec<_> = run(&t, &keepout).into_iter().map(|l| l.text).collect();
        assert_eq!(names, vec!["Free"]);
    }

    #[test]
    fn avoids_areas_and_fades_or_drops_distant_labels() {
        let t = tile(&[
            place("Inside", "city", 2048, 2048),
            place("Far", "city", 600, 600),
        ]);
        let keepout = Keepout {
            areas: vec![vec![
                (156.0, 156.0),
                (356.0, 156.0),
                (356.0, 356.0),
                (156.0, 356.0),
            ]],
            ..Keepout::default()
        };
        let names: Vec<_> = run(&t, &keepout).into_iter().map(|l| l.text).collect();
        assert_eq!(names, vec!["Far"]);

        let tiles = [(placement(), &t)];
        let at = |scale: f32| {
            move |p: &TilePlacement, x: f64, y: f64| {
                Some((
                    (p.dx + x * p.size) as f32,
                    (p.dy + y * p.size) as f32,
                    scale,
                ))
            }
        };
        let canvas = (512.0, 512.0);
        let none = Keepout::default();
        let faded = place_labels(&tiles, &style(), at(0.7), canvas, &none);
        assert!(faded.iter().all(|l| l.opacity < 1.0 && l.opacity > 0.3));
        let tiny = LabelStyle {
            size: 5.0,
            ..style()
        };
        assert!(place_labels(&tiles, &tiny, at(1.0), canvas, &none).is_empty());
    }

    #[test]
    fn drops_duplicates_from_neighbouring_tiles() {
        let t = tile(&[
            place("Twin", "town", 2048, 2048),
            place("Twin", "town", 2100, 2600),
        ]);
        assert_eq!(run(&t, &Keepout::default()).len(), 1);
    }

    #[test]
    fn draws_labels() {
        let mut pixmap = Pixmap::new(200, 60).unwrap();
        pixmap.fill(Color::from_rgba8(0x40, 0x40, 0x40, 0xff));
        let before = pixmap.data().to_vec();
        let label = PlacedLabel {
            text: "Seoul".into(),
            x: 10.0,
            y: 10.0,
            size: 20.0,
            opacity: 1.0,
        };
        draw_labels(&mut pixmap, &[label], &style());
        assert_ne!(before, pixmap.data());
    }
}
