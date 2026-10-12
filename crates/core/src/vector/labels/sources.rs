//! Label sources collected from vector tiles: point names, and road and river lines with their arc lengths.

use std::borrow::Cow;

use super::line_stitch::{clip_to_tile, stitch, Stitch};
use super::memory::name_key;
use super::style::{LabelStyle, LINE_MIN_ZOOM, STITCH_UNITS};
use crate::tiles::TilePlacement;
use crate::vector::mvt::{Feature, GeomType, Layer, VectorTile};

/// The OpenMapTiles layers labels come from; decode just these with
/// [`VectorTile::decode_layers`] when only labels are needed.
pub const LABEL_LAYERS: [&str; 5] = [
    "place",
    "mountain_peak",
    "water_name",
    "transportation_name",
    "waterway",
];

/// One labelled point from a tile, before it is placed on screen.
#[derive(Debug, Clone)]
pub(super) struct LabelSource {
    pub(super) text: String,
    /// Hash of `text`, as remembered in a [`LabelMemory`](super::LabelMemory).
    pub(super) key: u64,
    pub(super) priority: (u8, f64),
    pub(super) size_factor: f32,
    pub(super) placement: TilePlacement,
    /// Tile-normalized position (`0..=1`).
    pub(super) point: (f64, f64),
}

/// The labelled points of some tiles, most important first: everything about
/// the labels except where the view puts them, so a moving view can reuse it
/// across frames instead of reading the tiles again.
#[derive(Debug, Clone, Default)]
pub struct LabelSources {
    pub(super) points: Vec<LabelSource>,
    pub(super) lines: Vec<LineSource>,
}

/// A named line (a road or river) to label along its course.
#[derive(Debug, Clone)]
pub(super) struct LineSource {
    pub(super) text: String,
    /// Hash of `text`, as remembered in a [`LabelMemory`](super::LabelMemory)
    /// apart from point labels.
    pub(super) key: u64,
    /// Lower draws first: major roads before minor ones.
    pub(super) rank: u8,
    /// Where `points` are: always at the origin with size 1, as the points
    /// are already in the tiles' pixels (a line may span several tiles).
    pub(super) placement: TilePlacement,
    /// World zoom of the tiles the line came from.
    zoom: f64,
    /// Points in the tiles' pixels (`dx + x × size` of each tile), clipped
    /// to the tiles and joined across their edges.
    pub(super) points: Vec<(f64, f64)>,
    /// Whether each end (start, end) lies on a tile edge, where the line may
    /// go on in a neighbouring tile.
    open: (bool, bool),
    /// Size of the tiles in their pixels.
    tile_size: f64,
    /// The middle of each tile's piece joined into this line, which
    /// doesn't move whatever other tiles are loaded.
    pub(super) marks: Vec<PieceMark>,
    /// Each piece's middle as a distance along the line, and half the
    /// piece's length: copies of the label sit at fixed steps around these,
    /// so they stay put on the line while the view (and the tiles loaded
    /// for it) changes.
    pub(super) anchors: Vec<(f64, f64)>,
    /// Distance along the line at each point, in the tiles' pixels.
    pub(super) lengths: Vec<f64>,
    /// Advance of each glyph of `text` at size 1 (they scale with size).
    pub(super) advances: Vec<f32>,
}

impl LabelSources {
    /// Read the place, peak, and water names in `tiles` (OpenMapTiles
    /// schema) that `style`'s font can draw, most important first; equal
    /// priorities are ordered by name.
    pub fn collect(tiles: &[(TilePlacement, &VectorTile)], style: &LabelStyle) -> Self {
        let mut sources = Vec::new();
        let mut lines = Vec::new();
        for (placement, tile) in tiles {
            for layer in &tile.layers {
                let name = layer.name.as_str();
                if matches!(name, "place" | "mountain_peak" | "water_name") {
                    collect(layer, name, placement, style, &mut sources);
                }
                if matches!(name, "transportation_name" | "waterway" | "water_name") {
                    collect_lines(layer, placement, style, &mut lines);
                }
            }
        }
        sources.sort_by(point_order);
        let mut lines: Vec<LineSource> = stitch(lines.iter())
            .into_iter()
            .map(Cow::into_owned)
            .collect();
        lines.sort_by(line_order);
        Self {
            points: sources,
            lines,
        }
    }

    /// One set of labels from several (e.g. collected one tile at a time and
    /// kept), in the order [`LabelSources::collect`] gives.
    pub fn merge<'a>(parts: impl IntoIterator<Item = &'a LabelSources>) -> Self {
        let mut merged = Self::default();
        let parts: Vec<&LabelSources> = parts.into_iter().collect();
        for part in &parts {
            merged.points.extend(part.points.iter().cloned());
        }
        merged.lines = stitch(parts.iter().flat_map(|p| &p.lines))
            .into_iter()
            .map(Cow::into_owned)
            .collect();
        merged.points.sort_by(point_order);
        merged.lines.sort_by(line_order);
        merged
    }
}

impl LineSource {
    /// Whether the line is named at its tile's zoom: minor roads only up
    /// close, so wide views stay readable (as MapLibre styles do).
    pub(super) fn shown(&self, style: &LabelStyle) -> bool {
        let zoom = self.zoom - (style.pixel_ratio as f64).log2();
        zoom >= LINE_MIN_ZOOM[self.rank.min(5) as usize]
    }
}

/// The middle of one tile's piece of a line (in the tiles' pixels) and
/// half its length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PieceMark {
    pub(super) x: f64,
    pub(super) y: f64,
    half: f64,
}

impl Stitch for LineSource {
    type Mark = PieceMark;

    fn group(&self) -> (u64, u8, u64) {
        (self.key, self.rank, u64::from(self.placement.id.zoom))
    }

    fn marks(&self) -> &[PieceMark] {
        &self.marks
    }

    fn points(&self) -> &[(f64, f64)] {
        &self.points
    }

    fn tolerance(&self) -> f64 {
        STITCH_UNITS / 4096.0 * self.tile_size
    }

    fn open(&self) -> (bool, bool) {
        self.open
    }

    fn with_points(
        &self,
        points: Vec<(f64, f64)>,
        open: (bool, bool),
        marks: Vec<PieceMark>,
    ) -> Self {
        let lengths = arc_lengths(&points);
        Self {
            anchors: anchors(&points, &lengths, &marks),
            lengths,
            points,
            open,
            marks,
            text: self.text.clone(),
            advances: self.advances.clone(),
            ..*self
        }
    }
}

/// Point labels most important first, equal priorities by name.
pub(super) fn point_order(a: &LabelSource, b: &LabelSource) -> std::cmp::Ordering {
    a.priority
        .0
        .cmp(&b.priority.0)
        .then(a.priority.1.total_cmp(&b.priority.1))
        .then_with(|| a.text.cmp(&b.text))
}

/// Line labels by road rank, then name.
pub(super) fn line_order(a: &LineSource, b: &LineSource) -> std::cmp::Ordering {
    a.rank.cmp(&b.rank).then_with(|| a.text.cmp(&b.text))
}

/// Read the named line features of a `transportation_name`, `waterway`, or
/// `water_name` layer.
fn collect_lines(
    layer: &Layer,
    placement: &TilePlacement,
    style: &LabelStyle,
    out: &mut Vec<LineSource>,
) {
    let extent = layer.extent.max(1) as f64;
    for feature in layer
        .features
        .iter()
        .filter(|f| f.geom_type == GeomType::LineString)
    {
        let Some(text) = label_text(layer, feature, style) else {
            continue;
        };
        let class = layer.property(feature, "class").and_then(|v| v.as_str());
        let rank = match (layer.name.as_str(), class) {
            (_, Some("motorway" | "trunk")) => 0,
            (_, Some("primary")) => 1,
            ("waterway" | "water_name", _) => 2,
            (_, Some("secondary" | "tertiary")) => 3,
            (_, Some("minor" | "service")) => 4,
            _ => 5,
        };
        let p = placement;
        let edge = |v: f64| v <= 0.0 || v >= 1.0;
        for part in feature.parts() {
            let points: Vec<(f64, f64)> = part
                .iter()
                .map(|&(x, y)| (x as f64 / extent, y as f64 / extent))
                .collect();
            for piece in clip_to_tile(&points) {
                let (first, last) = (piece[0], piece[piece.len() - 1]);
                let open = (edge(first.0) || edge(first.1), edge(last.0) || edge(last.1));
                let points: Vec<(f64, f64)> = piece
                    .into_iter()
                    .map(|(x, y)| (p.dx + x * p.size, p.dy + y * p.size))
                    .collect();
                let lengths = arc_lengths(&points);
                let total = lengths.last().copied().unwrap_or(0.0);
                let (x, y) = point_along(&points, &lengths, total / 2.0);
                let marks = vec![PieceMark {
                    x,
                    y,
                    half: total / 2.0,
                }];
                out.push(LineSource {
                    advances: style.font.advances(text, 1.0),
                    text: text.to_string(),
                    key: name_key("line", text),
                    rank,
                    placement: TilePlacement {
                        dx: 0.0,
                        dy: 0.0,
                        size: 1.0,
                        ..*p
                    },
                    zoom: p.id.zoom as f64 + (p.size / 256.0).log2(),
                    anchors: anchors(&points, &lengths, &marks),
                    lengths,
                    points,
                    open,
                    marks,
                    tile_size: p.size,
                });
            }
        }
    }
}

/// Distance along `points` at each of them.
fn arc_lengths(points: &[(f64, f64)]) -> Vec<f64> {
    let mut walked = 0.0;
    let mut lengths = Vec::with_capacity(points.len());
    lengths.push(0.0);
    for w in points.windows(2) {
        walked += (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1);
        lengths.push(walked);
    }
    lengths.truncate(points.len());
    lengths
}

/// The point `s` along `points` (whose distances along are `lengths`).
fn point_along(points: &[(f64, f64)], lengths: &[f64], s: f64) -> (f64, f64) {
    let n = points.len();
    if n < 2 {
        return points.first().copied().unwrap_or((0.0, 0.0));
    }
    let k = lengths.partition_point(|&l| l <= s).clamp(1, n - 1) - 1;
    let span = (lengths[k + 1] - lengths[k]).max(f64::EPSILON);
    let t = ((s - lengths[k]) / span).clamp(0.0, 1.0);
    let (a, b) = (points[k], points[k + 1]);
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Where each piece's middle (`marks`) falls along `points`, as a distance
/// along it, with half the piece's length.
fn anchors(points: &[(f64, f64)], lengths: &[f64], marks: &[PieceMark]) -> Vec<(f64, f64)> {
    marks
        .iter()
        .map(|m| {
            let nearest = points
                .windows(2)
                .zip(lengths)
                .map(|(w, &start)| {
                    let (a, b) = (w[0], w[1]);
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let len2 = (dx * dx + dy * dy).max(f64::EPSILON);
                    let t = (((m.x - a.0) * dx + (m.y - a.1) * dy) / len2).clamp(0.0, 1.0);
                    let (px, py) = (a.0 + dx * t, a.1 + dy * t);
                    ((px - m.x).hypot(py - m.y), start + t * len2.sqrt())
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            (nearest.map_or(0.0, |n| n.1), m.half)
        })
        .collect()
}

/// A feature's label: the preferred name if present, else the local one,
/// falling back to its Latin form when the font can't draw the name (lacks
/// its glyphs, or its script needs shaping and that isn't built in). `None`
/// when the font can't draw either.
fn label_text<'a>(layer: &'a Layer, feature: &Feature, style: &LabelStyle) -> Option<&'a str> {
    let name = |key: &str| {
        layer
            .property(feature, key)
            .and_then(|v| v.as_str())
            .map(str::trim)
    };
    style
        .language
        .as_ref()
        .and_then(|lang| name(&format!("name:{lang}")))
        .or_else(|| name("name"))
        .and_then(|t| {
            if style.font.draws(t) {
                Some(t)
            } else {
                name("name:latin")
            }
        })
        .filter(|t| !t.is_empty() && style.font.draws(t))
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
        let Some(text) = label_text(layer, feature, style) else {
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
                    key: name_key("point", text),
                    priority: (tier, rank),
                    size_factor,
                    placement: *placement,
                    point: (px as f64 / extent, py as f64 / extent),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::TextFont;
    use crate::tiles::TileId;
    use crate::vector::labels::test_support::*;
    use crate::vector::labels::{place_labels, Keepout};
    use crate::vector::mvt::Value;

    #[test]
    fn names_the_font_cannot_draw_fall_back_to_latin() {
        let cairo = (
            (2048, 2048),
            vec![
                ("name", Value::String("القاهرة".into())),
                ("name:latin", Value::String("Cairo".into())),
                ("class", Value::String("city".into())),
            ],
        );
        let mut plain = style();
        plain.language = None;
        let t = tile(&[cairo]);
        let tiles = [(placement(), &t)];
        let names: Vec<String> = LabelSources::collect(&tiles, &plain)
            .place(
                &plain,
                |p, x, y| Some(((p.dx + x * p.size) as f32, (p.dy + y * p.size) as f32, 1.0)),
                (512.0, 512.0),
                &Keepout::default(),
            )
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(names, ["Cairo"]);
        if !cfg!(feature = "shaping") {
            return;
        }
        // A font with the Arabic letters shows the Arabic name.
        let bytes = include_bytes!("../../../assets/test-arabic.ttf").to_vec();
        let arabic = LabelStyle {
            font: TextFont::from_bytes(bytes, 0).unwrap(),
            ..plain
        };
        let names: Vec<String> = LabelSources::collect(&tiles, &arabic)
            .place(
                &arabic,
                |p, x, y| Some(((p.dx + x * p.size) as f32, (p.dy + y * p.size) as f32, 1.0)),
                (512.0, 512.0),
                &Keepout::default(),
            )
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(names, ["القاهرة"]);
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
    fn a_road_cut_at_a_tile_edge_is_named_across_it() {
        use crate::vector::mvt::Value;
        let props = || {
            vec![
                ("name", Value::String("Ring".into())),
                ("class", Value::String("motorway".into())),
            ]
        };
        // Each tile holds a stretch too short for the name (with its buffer
        // past the shared edge); together they fit it.
        let west = VectorTile {
            layers: vec![Layer::test_line(
                "transportation_name",
                4000,
                4400,
                2048,
                &props(),
            )],
        };
        let east = VectorTile {
            layers: vec![Layer::test_line(
                "transportation_name",
                -300,
                100,
                2048,
                &props(),
            )],
        };
        let east_tile = TilePlacement {
            id: TileId {
                zoom: 0,
                x: 1,
                y: 0,
            },
            dx: 512.0,
            ..placement()
        };
        let project = |p: &TilePlacement, x: f64, y: f64| {
            Some(((p.dx + x * p.size) as f32, (p.dy + y * p.size) as f32, 1.0))
        };
        let place = |tiles: &[(TilePlacement, &VectorTile)]| {
            place_labels(
                tiles,
                &style(),
                project,
                (1024.0, 512.0),
                &Keepout::default(),
            )
        };
        assert!(place(&[(placement(), &west)]).is_empty());
        assert!(place(&[(east_tile, &east)]).is_empty());
        let labels = place(&[(placement(), &west), (east_tile, &east)]);
        assert_eq!(labels.len(), 1, "{labels:?}");
        let middle = labels[0].glyphs[labels[0].glyphs.len() / 2];
        assert!(
            (middle.0 - 512.0).abs() < 30.0,
            "across the edge: {middle:?}"
        );
    }

    #[test]
    fn road_name_copies_hold_when_off_screen_tiles_change() {
        use crate::vector::mvt::Value;
        let props = || {
            vec![
                ("name", Value::String("Main Street".into())),
                ("class", Value::String("motorway".into())),
            ]
        };
        // One straight road across a row of tiles, each holding its piece
        // (with the buffer past both edges).
        let pieces: Vec<(TilePlacement, VectorTile)> = (0..4u32)
            .map(|x| {
                let tile = TilePlacement {
                    id: TileId { zoom: 2, x, y: 0 },
                    dx: x as f64 * 512.0,
                    ..placement()
                };
                let layer = Layer::test_line("transportation_name", -200, 4300, 2048, &props());
                (
                    tile,
                    VectorTile {
                        layers: vec![layer],
                    },
                )
            })
            .collect();
        // A flat view over the first two tiles, the others off screen.
        let flat = |tiles: &[(TilePlacement, &VectorTile)]| {
            let parts: Vec<LabelSources> = tiles
                .iter()
                .map(|t| LabelSources::collect(std::slice::from_ref(t), &style()))
                .collect();
            let refs: Vec<&LabelSources> = parts.iter().collect();
            let project = |p: &TilePlacement, x: f64, y: f64| {
                Some((
                    (p.dx + x * p.size) as f32 - 256.0,
                    (p.dy + y * p.size) as f32,
                    1.0,
                ))
            };
            let mut xs: Vec<f32> = LabelSources::merge(refs)
                .place(&style(), project, (512.0, 512.0), &Keepout::default())
                .iter()
                .map(|l| l.glyphs[l.glyphs.len() / 2].0)
                .collect();
            xs.sort_by(f32::total_cmp);
            xs
        };
        let tiles: Vec<(TilePlacement, &VectorTile)> =
            pieces.iter().map(|(p, t)| (*p, t)).collect();
        let two = flat(&tiles[..2]);
        let four = flat(&tiles);
        assert!(!two.is_empty());
        assert_eq!(two, four, "off-screen tiles don't move the copies");
    }

    #[test]
    fn minor_road_names_wait_for_a_close_zoom() {
        use crate::vector::mvt::Value;
        let lane = |class: &str| {
            Layer::test_line(
                "transportation_name",
                400,
                3700,
                2048,
                &[
                    ("name", Value::String("Back Lane".into())),
                    ("class", Value::String(class.into())),
                ],
            )
        };
        let at_zoom = |zoom: u32, layer: Layer| {
            let t = VectorTile {
                layers: vec![layer],
            };
            let tile = TilePlacement {
                id: TileId { zoom, x: 0, y: 0 },
                ..placement()
            };
            let project = |p: &TilePlacement, x: f64, y: f64| {
                Some(((p.dx + x * p.size) as f32, (p.dy + y * p.size) as f32, 1.0))
            };
            let tiles = [(tile, &t)];
            let labels = place_labels(
                &tiles,
                &style(),
                project,
                (512.0, 512.0),
                &Keepout::default(),
            );
            labels.len()
        };
        // 512 px tiles: one zoom deeper than the tile id.
        assert_eq!(at_zoom(12, lane("minor")), 0);
        assert!(at_zoom(14, lane("minor")) > 0);
        assert!(at_zoom(5, lane("motorway")) > 0);
    }
}
