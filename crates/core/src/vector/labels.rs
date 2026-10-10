//! Point labels for place, peak, and water names: chosen by importance,
//! kept clear of each other, the track, and screen overlays, and drawn with a
//! halo.

use tiny_skia::{Color, Pixmap, Rect};

use super::mvt::{GeomType, Layer, VectorTile};
use crate::render::TextFont;
use crate::tiles::TilePlacement;

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

struct Candidate {
    text: String,
    priority: (u8, f64),
    size_factor: f32,
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
    let mut candidates = Vec::new();
    for (placement, tile) in tiles {
        for layer in &tile.layers {
            let kind = match layer.name.as_str() {
                "place" | "mountain_peak" | "water_name" => layer.name.as_str(),
                _ => continue,
            };
            collect(layer, kind, placement, style, &project, &mut candidates);
        }
    }
    candidates.sort_by(|a, b| {
        a.priority
            .0
            .cmp(&b.priority.0)
            .then(a.priority.1.total_cmp(&b.priority.1))
            .then_with(|| a.text.cmp(&b.text))
    });

    let ratio = style.pixel_ratio;
    let mut placed: Vec<(PlacedLabel, Rect)> = Vec::new();
    for c in candidates {
        if placed.len() >= MAX_LABELS {
            break;
        }
        if placed
            .iter()
            .any(|(p, b)| p.text == c.text && distance(center(b), c.anchor) < DUPLICATE_PX * ratio)
        {
            continue;
        }
        let size = style.size * c.size_factor * c.scale * ratio;
        if size < MIN_SIZE_PX * ratio {
            continue;
        }
        let w = style.font.measure(&c.text, size);
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
            || touches_track(&bounds, keepout)
            || keepout.areas.iter().any(|a| touches_area(&bounds, a))
        {
            continue;
        }
        let fade = ((c.scale - FAR_SCALE) / (NEAR_SCALE - FAR_SCALE)).clamp(0.0, 1.0);
        placed.push((
            PlacedLabel {
                text: c.text,
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

/// Draw placed labels with a halo so they read over any map.
pub fn draw_labels(pixmap: &mut Pixmap, labels: &[PlacedLabel], style: &LabelStyle) {
    for label in labels {
        let fade = |c: Color| {
            Color::from_rgba(c.red(), c.green(), c.blue(), c.alpha() * label.opacity).unwrap_or(c)
        };
        let (color, halo) = (fade(style.color), fade(style.halo));
        let r = halo_radius(label.size);
        for (dx, dy) in [
            (-1.0, -1.0),
            (0.0, -1.0),
            (1.0, -1.0),
            (-1.0, 0.0),
            (1.0, 0.0),
            (-1.0, 1.0),
            (0.0, 1.0),
            (1.0, 1.0),
        ] {
            let (x, y) = (label.x + dx * r, label.y + dy * r);
            style.font.draw(pixmap, &label.text, x, y, label.size, halo);
        }
        let (x, y) = (label.x, label.y);
        style
            .font
            .draw(pixmap, &label.text, x, y, label.size, color);
    }
}

fn collect(
    layer: &Layer,
    kind: &str,
    placement: &TilePlacement,
    style: &LabelStyle,
    project: &impl Fn(&TilePlacement, f64, f64) -> Option<(f32, f32, f32)>,
    out: &mut Vec<Candidate>,
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
                let projected = project(placement, px as f64 / extent, py as f64 / extent);
                let Some((x, y, scale)) = projected else {
                    continue;
                };
                out.push(Candidate {
                    text: text.to_string(),
                    priority: (tier, rank),
                    size_factor,
                    anchor: (x, y),
                    scale,
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

/// Whether any track segment passes through `bounds` grown by the tracks'
/// half-width.
fn touches_track(bounds: &Rect, keepout: &Keepout) -> bool {
    let w = keepout.track_half_width;
    let grown = (
        bounds.left() - w,
        bounds.top() - w,
        bounds.right() + w,
        bounds.bottom() + w,
    );
    keepout
        .tracks
        .iter()
        .flat_map(|t| t.windows(2))
        .any(|s| crosses(grown, s[0], s[1]))
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
