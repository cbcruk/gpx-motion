//! Map labels: place, peak, and water names at points, and road and river
//! names along their lines; chosen by importance, kept clear of each other,
//! the track, and screen overlays, and drawn with a halo.

use tiny_skia::{Color, Pixmap, Rect};

use super::line_labels::{Polyline, MAX_TOTAL_BEND, SHAPED_MAX_TOTAL_BEND};
use super::line_stitch::{clip_to_tile, stitch, Stitch};
use super::mvt::{Feature, GeomType, Layer, VectorTile};
use crate::geo::camera::{Projector, NEAR};
use crate::geo::Camera;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use super::key_hash::KeyHash;

use crate::render::text::needs_shaping;
use crate::render::TextFont;
use crate::terrain::Terrain;
use crate::tiles::TilePlacement;

/// The OpenMapTiles layers labels come from; decode just these with
/// [`VectorTile::decode_layers`] when only labels are needed.
pub const LABEL_LAYERS: [&str; 5] = [
    "place",
    "mountain_peak",
    "water_name",
    "transportation_name",
    "waterway",
];

/// 3D labels farther than this many times the camera distance are dropped
/// (they would sit in the horizon fog).
const LABEL_FAR_FACTOR: f64 = 2.0;

/// Line labels (road and river names) are this much smaller than point
/// labels of the base size.
const LINE_SIZE_FACTOR: f32 = 0.85;
/// Gap between repeats of a line label along a long line, in CSS pixels.
const LINE_REPEAT_PX: f32 = 280.0;

/// Most labels drawn in one view, so dense cities stay readable. In a
/// sequence of views, labels shown in the view before keep their places
/// first and new ones fill the rest (see [`LabelSources::place_3d_steady`]).
const MAX_LABELS: usize = 60;
/// Same-named labels closer than this many CSS pixels are duplicates (tile
/// buffers repeat features near tile edges).
const DUPLICATE_PX: f32 = 200.0;
/// Clear space kept around each label (beyond its halo), in CSS pixels.
const PADDING_PX: f32 = 3.0;
/// Labels that would draw smaller than this many CSS pixels are skipped.
const MIN_SIZE_PX: f32 = 8.0;
/// Smallest size, as a share of [`MIN_SIZE_PX`], a label already shown keeps
/// fading out at, so one hovering at the limit doesn't blink.
const FADING_MIN_SIZE: f32 = 0.75;
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
    /// For a label that follows a line (a road or river name), each
    /// character's center and angle (radians); empty for a point label,
    /// which `x` and `y` place instead.
    pub glyphs: Vec<(f32, f32, f32)>,
    /// Which of `glyphs` to leave undrawn because they lie where labels keep
    /// clear of (a line label fading out across an overlay or the track);
    /// empty when every glyph shows.
    pub covered: Vec<bool>,
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
    /// Hash of `text`, as remembered in a [`LabelMemory`].
    key: u64,
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
pub struct LabelSources {
    points: Vec<LabelSource>,
    lines: Vec<LineSource>,
}

/// A named line (a road or river) to label along its course.
#[derive(Debug, Clone)]
struct LineSource {
    text: String,
    /// Hash of `text`, as remembered in a [`LabelMemory`] apart from point
    /// labels.
    key: u64,
    /// Lower draws first: major roads before minor ones.
    rank: u8,
    /// Where `points` are: always at the origin with size 1, as the points
    /// are already in the tiles' pixels (a line may span several tiles).
    placement: TilePlacement,
    /// World zoom of the tiles the line came from.
    zoom: f64,
    /// Points in the tiles' pixels (`dx + x × size` of each tile), clipped
    /// to the tiles and joined across their edges.
    points: Vec<(f64, f64)>,
    /// Whether each end (start, end) lies on a tile edge, where the line may
    /// go on in a neighbouring tile.
    open: (bool, bool),
    /// Size of the tiles in their pixels.
    tile_size: f64,
    /// The middle of each tile's piece joined into this line, which
    /// doesn't move whatever other tiles are loaded.
    marks: Vec<PieceMark>,
    /// Each piece's middle as a distance along the line, and half the
    /// piece's length: copies of the label sit at fixed steps around these,
    /// so they stay put on the line while the view (and the tiles loaded
    /// for it) changes.
    anchors: Vec<(f64, f64)>,
    /// Distance along the line at each point, in the tiles' pixels.
    lengths: Vec<f64>,
    /// Advance of each glyph of `text` at size 1 (they scale with size).
    advances: Vec<f32>,
}

struct Candidate<'a> {
    source: &'a LabelSource,
    anchor: (f32, f32),
    scale: f32,
    /// Whether the anchor is on the canvas; one just past its edge may only
    /// fade out there.
    on_canvas: bool,
}

/// How far past the canvas edge (CSS pixels) a label's anchor may go while
/// it fades out, rather than vanishing as it leaves.
const EDGE_FADE_PX: f32 = 48.0;

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

    /// Place these labels for a view; see [`place_labels`] for `project`.
    pub fn place(
        &self,
        style: &LabelStyle,
        project: impl Fn(&TilePlacement, f64, f64) -> Option<(f32, f32, f32)>,
        canvas: (f32, f32),
        keepout: &Keepout,
    ) -> Vec<PlacedLabel> {
        let candidates = self.points.iter().filter_map(|source| {
            let (x, y, scale) = project(&source.placement, source.point.0, source.point.1)?;
            Some(Candidate {
                source,
                anchor: (x, y),
                scale,
                on_canvas: true,
            })
        });
        let mut placer = Placer::new(style, canvas, keepout);
        for c in candidates {
            if !placer.has_room() {
                break;
            }
            placer.place(&c, 1.0);
        }
        let margin = LINE_REPEAT_PX * style.pixel_ratio;
        for line in self.lines.iter().filter(|l| l.shown(style)) {
            for run in project_line(line, &project, canvas, margin) {
                placer.place_line(line, &run, 1.0, false, 0, |_, _, _| false);
            }
        }
        placer.finish()
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
        let projector = camera.projector();
        let project = |p: &TilePlacement, x: f64, y: f64| {
            let (wx, wy) = (p.dx + x * p.size, p.dy + y * p.size);
            project_3d(&projector, wx, wy, ground(wx, wy))
        };
        let size = (canvas.0 as f32, canvas.1 as f32);
        self.place(style, project, size, keepout)
    }
}

/// Screen point and size factor of a ground point raised `h` under a
/// camera, or `None` behind the camera or toward the horizon fog.
fn project_3d(projector: &Projector, wx: f64, wy: f64, h: f64) -> Option<(f32, f32, f32)> {
    let view = projector.view(wx, wy, h);
    let camera = projector.camera();
    if view.depth > camera.distance * LABEL_FAR_FACTOR || view.depth < NEAR {
        return None;
    }
    let (sx, sy) = projector.to_screen(view);
    let scale = (camera.distance / view.depth).clamp(0.6, 1.2) as f32;
    Some((sx as f32, sy as f32, scale))
}

/// What a sequence of views (video frames, or a turning camera) remembers
/// about its labels, so they hold still from one view to the next.
///
/// Within a priority tier, labels shown in the previous view are placed
/// first, so a label of similar importance can't push them out and back in
/// as the camera moves; a more important label still wins. Labels fade in
/// and out over `fade_frames` views (the first view shows everything at
/// once), including ones the terrain starts to hide.
#[derive(Debug, Clone, Default)]
pub struct LabelMemory {
    /// Fade level (`1..=fade_frames`) of each label shown last view, keyed
    /// by name and coarse place so two places sharing a name stay apart.
    levels: HashMap<LabelKey, u32, KeyHash>,
    /// Copies each line label (by each of its keys) drew last view.
    copies: HashMap<LabelKey, usize, KeyHash>,
    fade_frames: u32,
    primed: bool,
}

/// A label's name and its position on a coarse world grid.
type LabelKey = (u64, i64, i64);

/// A stable hash of a label's name for [`LabelKey`]s.
fn name_key(kind: &str, text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (kind, text).hash(&mut hasher);
    hasher.finish()
}

/// Grid of [`LabelKey`] positions: world pixels at this zoom (`256·2^KEY_ZOOM`
/// cells per side, tens of meters each), far finer than places sharing a
/// name are apart.
const KEY_ZOOM: i32 = 12;

impl LabelMemory {
    /// An empty memory whose labels fade over `fade_frames` views (`0` or
    /// `1`: no fade).
    pub fn new(fade_frames: u32) -> Self {
        Self {
            levels: HashMap::default(),
            copies: HashMap::default(),
            fade_frames,
            primed: false,
        }
    }
}

impl LabelSources {
    /// [`LabelSources::place_3d`] for one view in a sequence, steadied by
    /// `memory` (see [`LabelMemory`]). Over `terrain`, labels stand on the
    /// ground and those the hills hide are left out (after fading).
    pub fn place_3d_steady(
        &self,
        camera: &Camera,
        style: &LabelStyle,
        canvas: (u32, u32),
        keepout: &Keepout,
        terrain: Option<&Terrain>,
        memory: &mut LabelMemory,
    ) -> Vec<PlacedLabel> {
        Self::place_3d_steady_parts(&[self], camera, style, canvas, keepout, terrain, memory)
    }

    /// [`LabelSources::place_3d_steady`] for labels kept in several parts
    /// (e.g. one per tile), as if [merged](LabelSources::merge) but without
    /// copying them.
    pub fn place_3d_steady_parts(
        parts: &[&LabelSources],
        camera: &Camera,
        style: &LabelStyle,
        canvas: (u32, u32),
        keepout: &Keepout,
        terrain: Option<&Terrain>,
        memory: &mut LabelMemory,
    ) -> Vec<PlacedLabel> {
        let mut points: Vec<&LabelSource> = parts.iter().flat_map(|p| &p.points).collect();
        points.sort_by(|a, b| point_order(a, b));
        // Pieces of a line kept in different parts (tiles) join up here; a
        // single part was joined when it was collected.
        let stitched: Vec<Cow<LineSource>> = match parts {
            [one] => one.lines.iter().map(Cow::Borrowed).collect(),
            _ => stitch(parts.iter().flat_map(|p| &p.lines)),
        };
        let mut all_lines: Vec<&LineSource> = stitched.iter().map(|l| l.as_ref()).collect();
        all_lines.sort_by(|a, b| line_order(a, b));
        let size = (canvas.0 as f32, canvas.1 as f32);
        let unit = 2f64.powi(KEY_ZOOM) / 2f64.powf(camera.zoom);
        let projector = camera.projector();
        // (candidate, its key, its world point)
        let candidates: Vec<(Candidate, LabelKey, (f64, f64, f64))> = points
            .iter()
            .copied()
            .filter_map(|source| {
                let p = &source.placement;
                let (wx, wy) = (
                    p.dx + source.point.0 * p.size,
                    p.dy + source.point.1 * p.size,
                );
                let h = terrain.map_or(0.0, |t| t.height_at_world(wx, wy));
                let (sx, sy, scale) = project_3d(&projector, wx, wy, h)?;
                let slack = EDGE_FADE_PX * style.pixel_ratio;
                let within = |v: f32, max: f32, slack: f32| (-slack..=max + slack).contains(&v);
                if !within(sx, size.0, slack) || !within(sy, size.1, slack) {
                    return None;
                }
                let on_canvas = within(sx, size.0, 0.0) && within(sy, size.1, 0.0);
                let key = (
                    source.key,
                    (wx * unit).floor() as i64,
                    (wy * unit).floor() as i64,
                );
                let candidate = Candidate {
                    source,
                    anchor: (sx, sy),
                    scale,
                    on_canvas,
                };
                Some((candidate, key, (wx, wy, h)))
            })
            .collect();
        // Looking along the line of sight is costly, so only labels that
        // would otherwise go in are checked.
        let in_sight =
            |&(x, y, h): &(f64, f64, f64)| !terrain.is_some_and(|t| t.hides_point(camera, x, y, h));
        let fade = memory.fade_frames.max(1);
        let level = |key: &LabelKey| memory.levels.get(key).copied().unwrap_or(0);

        let mut order: Vec<usize> = (0..candidates.len()).collect();
        // Stable: remembered labels go first within their tier only.
        order.sort_by_key(|&i| {
            (
                candidates[i].0.source.priority.0,
                level(&candidates[i].1) == 0,
            )
        });
        // Line labels, remembered by name and where each of their tile
        // pieces' middles is (all of them, so the memory holds while tiles
        // come and go).
        let mut lines: Vec<(&LineSource, Vec<LabelKey>)> = all_lines
            .iter()
            .copied()
            .filter(|line| line.shown(style))
            .map(|line| {
                let keys = line
                    .marks
                    .iter()
                    .map(|m| {
                        (
                            line.key,
                            (m.x * unit).floor() as i64,
                            (m.y * unit).floor() as i64,
                        )
                    })
                    .collect();
                (line, keys)
            })
            .collect();
        let line_level = |keys: &[LabelKey]| keys.iter().map(level).max().unwrap_or(0);
        lines.sort_by_cached_key(|(line, keys)| (line.rank, line_level(keys) == 0));
        let kept_copies = |keys: &[LabelKey]| match line_level(keys) {
            0 => 0,
            _ => keys
                .iter()
                .filter_map(|k| memory.copies.get(k))
                .copied()
                .max()
                .unwrap_or(0),
        };
        // Labels shown last view keep their places under the cap (those
        // fading just off the canvas too, as they may come back); new ones
        // get what is left.
        let kept_points: HashSet<&LabelKey, KeyHash> = candidates
            .iter()
            .filter(|(_, key, _)| level(key) > 0)
            .map(|(_, key, _)| key)
            .collect();
        let reserved = kept_points.len()
            + lines
                .iter()
                .map(|(_, keys)| kept_copies(keys))
                .sum::<usize>();
        let mut placer = Placer::new(style, size, keepout);
        placer.reserved = reserved;
        let mut levels: HashMap<LabelKey, u32, KeyHash> = HashMap::default();
        for i in order {
            let (candidate, key, at) = &candidates[i];
            if levels.contains_key(key) {
                continue;
            }
            let next = if memory.primed {
                (level(key) + 1).min(fade)
            } else {
                fade
            };
            if !candidate.on_canvas {
                continue;
            }
            let opacity = next as f32 / fade as f32;
            let kept = level(key) > 0;
            if placer.place_where(candidate, opacity, false, kept, || in_sight(at)) {
                levels.insert(*key, next);
            }
        }
        // Labels that dropped out (pushed aside, behind a hill, or leaving the
        // screen) fade away where they still fit, partly off the canvas too.
        for (candidate, key, _) in &candidates {
            let was = level(key);
            if was <= 1 || levels.contains_key(key) {
                continue;
            }
            if placer.place_where(
                candidate,
                (was - 1) as f32 / fade as f32,
                true,
                true,
                || true,
            ) {
                levels.insert(*key, was - 1);
            }
        }
        // A line label whose stretch of line a hill hides is left out.
        let world = |p: &TilePlacement, x: f64, y: f64| {
            let (wx, wy) = (p.dx + x * p.size, p.dy + y * p.size);
            (wx, wy, terrain.map_or(0.0, |t| t.height_at_world(wx, wy)))
        };
        let project = |p: &TilePlacement, x: f64, y: f64| {
            let (wx, wy, h) = world(p, x, y);
            project_3d(&projector, wx, wy, h)
        };
        let margin = LINE_REPEAT_PX * style.pixel_ratio;
        let place_line =
            |placer: &mut Placer, line: &LineSource, level: u32, fading: bool, kept: usize| {
                let mut copies = 0;
                for run in project_line(line, &project, size, margin) {
                    let hidden = |a: usize, b: usize, t: f32| {
                        terrain.is_some_and(|ground| {
                            let (pa, pb) = (line.points[a], line.points[b]);
                            let t = t as f64;
                            let (x, y) = (pa.0 + (pb.0 - pa.0) * t, pa.1 + (pb.1 - pa.1) * t);
                            let (wx, wy, h) = world(&line.placement, x, y);
                            ground.hides_point(camera, wx, wy, h)
                        })
                    };
                    let opacity = level as f32 / fade as f32;
                    let kept = kept.saturating_sub(copies);
                    copies += placer.place_line(line, &run, opacity, fading, kept, hidden);
                }
                copies
            };
        let mut copies: HashMap<LabelKey, usize, KeyHash> = HashMap::default();
        let mut remember = |levels: &mut HashMap<LabelKey, u32, KeyHash>,
                            keys: &[LabelKey],
                            level: u32,
                            drawn: usize| {
            for key in keys {
                levels.insert(*key, level);
                copies.insert(*key, drawn);
            }
        };
        for (line, keys) in &lines {
            if keys.iter().any(|k| levels.contains_key(k)) {
                continue;
            }
            let next = if memory.primed {
                (line_level(keys) + 1).min(fade)
            } else {
                fade
            };
            let drawn = place_line(&mut placer, line, next, false, kept_copies(keys));
            if drawn > 0 {
                remember(&mut levels, keys, next, drawn);
            }
        }
        // Line labels that dropped out fade away where they still fit. One
        // that doesn't is still remembered at the level it would have faded
        // to, so if a copy comes back (the line's copies move in and out of
        // view as the camera turns) it carries on from there instead of
        // fading in from nothing.
        for (line, keys) in &lines {
            let was = line_level(keys);
            if was <= 1 || keys.iter().any(|k| levels.contains_key(k)) {
                continue;
            }
            let drawn = place_line(&mut placer, line, was - 1, true, usize::MAX);
            remember(&mut levels, keys, was - 1, drawn);
        }
        memory.levels = levels;
        memory.copies = copies;
        memory.primed = true;
        placer.finish()
    }
}

impl LineSource {
    /// Whether the line is named at its tile's zoom: minor roads only up
    /// close, so wide views stay readable (as MapLibre styles do).
    fn shown(&self, style: &LabelStyle) -> bool {
        let zoom = self.zoom - (style.pixel_ratio as f64).log2();
        zoom >= LINE_MIN_ZOOM[self.rank.min(5) as usize]
    }
}

/// The middle of one tile's piece of a line (in the tiles' pixels) and
/// half its length.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PieceMark {
    x: f64,
    y: f64,
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

/// How far apart (in 4096ths of a tile) two pieces of a line cut at a tile
/// edge may end and still join: tiles simplify their geometry separately.
const STITCH_UNITS: f64 = 4.0;

/// Lowest (CSS-pixel) world zoom naming lines of each rank: highways,
/// primary roads, rivers, secondary roads, minor roads, paths.
const LINE_MIN_ZOOM: [f64; 6] = [0.0, 12.0, 12.0, 13.5, 15.0, 16.0];

/// Point labels most important first, equal priorities by name.
fn point_order(a: &LabelSource, b: &LabelSource) -> std::cmp::Ordering {
    a.priority
        .0
        .cmp(&b.priority.0)
        .then(a.priority.1.total_cmp(&b.priority.1))
        .then_with(|| a.text.cmp(&b.text))
}

/// Line labels by road rank, then name.
fn line_order(a: &LineSource, b: &LineSource) -> std::cmp::Ordering {
    a.rank.cmp(&b.rank).then_with(|| a.text.cmp(&b.text))
}

/// One on-screen stretch of a line.
struct Run {
    points: Vec<(f32, f32)>,
    /// Index into the line's points of each screen point.
    sources: Vec<usize>,
    /// Size factor at each screen point.
    scales: Vec<f32>,
}

/// A line's screen runs under `project`, split where points drop out, less
/// the runs wholly off `canvas` (by more than `margin`).
fn project_line(
    line: &LineSource,
    project: &impl Fn(&TilePlacement, f64, f64) -> Option<(f32, f32, f32)>,
    canvas: (f32, f32),
    margin: f32,
) -> Vec<Run> {
    let empty = || Run {
        points: Vec::new(),
        sources: Vec::new(),
        scales: Vec::new(),
    };
    let mut runs = vec![empty()];
    for (i, &(x, y)) in line.points.iter().enumerate() {
        match project(&line.placement, x, y) {
            Some((sx, sy, scale)) => {
                if let Some(run) = runs.last_mut() {
                    run.points.push((sx, sy));
                    run.sources.push(i);
                    run.scales.push(scale);
                }
            }
            None => runs.push(empty()),
        }
    }
    runs.retain(|run| {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in &run.points {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let on_canvas =
            x1 >= -margin && y1 >= -margin && x0 <= canvas.0 + margin && y0 <= canvas.1 + margin;
        run.points.len() > 1 && on_canvas
    });
    runs
}

/// Labels placed so far in one view, and what new ones must stay clear of.
struct Placer<'a> {
    style: &'a LabelStyle,
    canvas: (f32, f32),
    keepout: &'a Keepout,
    grid: KeepoutGrid<'a>,
    placed: Vec<PlacedLabel>,
    /// Screen boxes the placed labels take, with the label each belongs to.
    boxes: Vec<(usize, Rect)>,
    /// Places under [`MAX_LABELS`] held for labels shown in the view before.
    reserved: usize,
    /// New labels (and copies) placed so far, which the rest of the cap takes.
    fresh: usize,
}

impl<'a> Placer<'a> {
    fn new(style: &'a LabelStyle, canvas: (f32, f32), keepout: &'a Keepout) -> Self {
        Self {
            style,
            canvas,
            keepout,
            grid: KeepoutGrid::new(keepout, canvas),
            placed: Vec::new(),
            boxes: Vec::new(),
            reserved: 0,
            fresh: 0,
        }
    }

    /// Whether a label named `text` was placed within `radius` of `at`.
    fn near_same(&self, text: &str, at: (f32, f32), radius: f32) -> bool {
        self.boxes
            .iter()
            .any(|(i, b)| self.placed[*i].text == text && distance(center(b), at) < radius)
    }

    /// Whether `bounds` is on the canvas (or, `partly_off`, at least
    /// touches it) and clear of placed labels and the keepout.
    fn free(&self, bounds: &Rect, partly_off: bool) -> bool {
        self.clear_of_keepout(bounds, partly_off)
            && !self.boxes.iter().any(|(_, b)| overlaps(b, bounds))
    }

    /// Whether `bounds` is on the canvas (or, `partly_off`, at least
    /// touches it) and clear of the keepout (but perhaps over another
    /// label).
    fn clear_of_keepout(&self, bounds: &Rect, partly_off: bool) -> bool {
        let inside = if partly_off {
            bounds.right() > 0.0
                && bounds.bottom() > 0.0
                && bounds.left() < self.canvas.0
                && bounds.top() < self.canvas.1
        } else {
            bounds.left() >= 0.0
                && bounds.top() >= 0.0
                && bounds.right() <= self.canvas.0
                && bounds.bottom() <= self.canvas.1
        };
        inside
            && !self.keepout.rects.iter().any(|r| overlaps(r, bounds))
            && !self.grid.touches(bounds)
    }

    fn add(&mut self, label: PlacedLabel, boxes: impl IntoIterator<Item = Rect>) {
        let index = self.placed.len();
        self.boxes.extend(boxes.into_iter().map(|b| (index, b)));
        self.placed.push(label);
    }

    /// Place `line`'s name along its screen `run` at `fade` of its
    /// opacity: copies at fixed steps around the middle of each tile piece
    /// of the line, nearest a middle first, wherever they fit and at least
    /// [`LINE_REPEAT_PX`] from another copy of the name. `hidden(a, b, t)`
    /// tells whether the ground `t` of the way from line point `a` to `b` is
    /// out of sight; a copy is left out if it is under the text's ends or
    /// middle. Returns how many copies were placed.
    ///
    /// A `fading` label (on its way out) may run off the canvas and draw
    /// smaller for its last few frames, so it fades instead of vanishing;
    /// its glyphs that would overlap what labels keep clear of, another
    /// label, or a hill in front are left undrawn. The first `kept` copies
    /// (as many as it drew in the view before) take places held for them
    /// under [`MAX_LABELS`]; further ones count as new.
    fn place_line(
        &mut self,
        line: &LineSource,
        run: &Run,
        fade: f32,
        fading: bool,
        kept: usize,
        hidden: impl Fn(usize, usize, f32) -> bool,
    ) -> usize {
        let (style, text) = (self.style, line.text.as_str());
        let ratio = style.pixel_ratio;
        let polyline = Polyline::new(run.points.clone());
        let length = polyline.length();
        let spacing = LINE_REPEAT_PX * ratio;
        // Copies may sit every half `spacing` (in the line's own pixels)
        // around the middle of each tile piece joined into the line, within
        // the piece (then at its ends), nearest a middle first, wherever this
        // run shows them:
        // so each stays on its stretch of line as the view, and the tiles
        // loaded for it, change. Copies of one name still keep `spacing`
        // apart on screen.
        let total = line.lengths.last().copied().unwrap_or(0.0);
        let step = spacing as f64 / 2.0;
        let mut positions: Vec<(i64, usize, f64)> = Vec::new();
        for (i, &(middle, half)) in line.anchors.iter().enumerate() {
            let n = (half / step).floor() as i64;
            positions.extend((-n..=n).map(|k| (k.abs(), i, middle + k as f64 * step)));
            // The piece's ends (where it meets the next tile's) come last,
            // for a name that only fits across two short pieces.
            positions.extend([(n + 1, i, middle - half), (n + 1, i, middle + half)]);
        }
        positions.sort_by_key(|&(rank, piece, _)| (rank, piece));
        let first = run.sources.first().copied().unwrap_or(0);
        let on_run = |s: f64| -> Option<f32> {
            let n = line.lengths.len();
            if n < 2 || !(0.0..=total).contains(&s) {
                return None;
            }
            let k = line.lengths.partition_point(|&l| l <= s).clamp(1, n - 1) - 1;
            let span = (line.lengths[k + 1] - line.lengths[k]).max(f64::EPSILON);
            let t = ((s - line.lengths[k]) / span) as f32;
            let j = k.checked_sub(first)?;
            (run.sources.get(j) == Some(&k) && run.sources.get(j + 1) == Some(&(k + 1)))
                .then(|| polyline.lengths[j] + (polyline.lengths[j + 1] - polyline.lengths[j]) * t)
        };
        let centers: Vec<f32> = positions
            .into_iter()
            .filter_map(|(_, _, s)| on_run(s))
            .filter(|c| (0.0..=length).contains(c))
            .collect();
        let mut placed = 0;
        for center in centers {
            let fresh = placed >= kept;
            if fresh && !self.has_room() {
                break;
            }
            // Size and fade follow the depth where this copy sits.
            let k = polyline
                .lengths
                .partition_point(|&l| l <= center)
                .clamp(1, run.points.len() - 1);
            let scale = (run.scales[k - 1] + run.scales[k]) / 2.0;
            let size = style.size * LINE_SIZE_FACTOR * scale * ratio;
            let smallest = if fading { FADING_MIN_SIZE } else { 1.0 };
            if size < MIN_SIZE_PX * smallest * ratio {
                continue;
            }
            let advances: Vec<f32> = line.advances.iter().map(|a| a * size).collect();
            let height = style.font.line_height(size);
            let pad = PADDING_PX * ratio + halo_radius(size);
            let bend = if needs_shaping(text) {
                SHAPED_MAX_TOTAL_BEND
            } else {
                MAX_TOTAL_BEND
            };
            let Some(glyphs) = polyline.layout(&advances, center, bend) else {
                continue;
            };
            let mid = glyphs[glyphs.len() / 2];
            if self.near_same(text, (mid.x, mid.y), spacing * 0.9) {
                continue;
            }
            let boxes: Option<Vec<Rect>> = glyphs
                .iter()
                .zip(&advances)
                .map(|(g, &advance)| {
                    let (sin, cos) = (g.angle.sin().abs(), g.angle.cos().abs());
                    let hw = cos * advance / 2.0 + sin * height / 2.0 + pad;
                    let hh = sin * advance / 2.0 + cos * height / 2.0 + pad;
                    Rect::from_ltrb(g.x - hw, g.y - hh, g.x + hw, g.y + hh)
                })
                .collect();
            let on_canvas = |r: &Rect| {
                r.right() > 0.0
                    && r.bottom() > 0.0
                    && r.left() < self.canvas.0
                    && r.top() < self.canvas.1
            };
            // A fading copy may run off the canvas, or under what labels keep
            // clear of (the track, overlays drawn over it), as long as some
            // glyphs still show: it slips away rather than vanishing. Shaped
            // text can't lose single glyphs, so it must stay clear.
            let Some(boxes) = boxes else {
                continue;
            };
            // A fading copy's glyphs that something would be drawn over or
            // under (the keepout, another label, a hill in front) are left
            // out, so it never overlaps anything while it goes.
            // The costly look along the line of sight comes last, only for
            // glyphs that would still show.
            let mut covered: Vec<bool> = Vec::new();
            let fading_fits = |covered: &[bool]| {
                let shows = boxes.iter().zip(covered).any(|(r, &c)| on_canvas(r) && !c);
                shows && (!covered.contains(&true) || !needs_shaping(text))
            };
            if fading {
                covered = boxes
                    .iter()
                    .map(|r| {
                        on_canvas(r)
                            && (!self.clear_of_keepout(r, true)
                                || self.boxes.iter().any(|(_, b)| overlaps(b, r)))
                    })
                    .collect();
                if !fading_fits(&covered) {
                    continue;
                }
                for ((r, g), c) in boxes.iter().zip(&glyphs).zip(&mut covered) {
                    if !*c && on_canvas(r) {
                        *c = hidden(run.sources[g.segment], run.sources[g.segment + 1], g.t);
                    }
                }
                if !fading_fits(&covered) {
                    continue;
                }
            } else if !boxes.iter().all(|r| self.free(r, false)) {
                continue;
            }
            let probes = [glyphs[0], mid, glyphs[glyphs.len() - 1]];
            let out_of_sight = !fading
                && probes.iter().any(|g| {
                    let (a, b) = (run.sources[g.segment], run.sources[g.segment + 1]);
                    hidden(a, b, g.t)
                });
            if out_of_sight {
                continue;
            }
            // Only the glyphs drawn take room.
            let shown: Vec<Rect> = boxes
                .iter()
                .enumerate()
                .filter(|&(i, _)| !covered.get(i).copied().unwrap_or(false))
                .map(|(_, &b)| b)
                .collect();
            let far = ((scale - FAR_SCALE) / (NEAR_SCALE - FAR_SCALE)).clamp(0.0, 1.0);
            let label = PlacedLabel {
                text: text.to_string(),
                x: glyphs[0].x,
                y: glyphs[0].y,
                size,
                opacity: (FAR_OPACITY + (1.0 - FAR_OPACITY) * far) * fade,
                glyphs: glyphs.iter().map(|g| (g.x, g.y, g.angle)).collect(),
                covered: if covered.contains(&true) {
                    covered
                } else {
                    Vec::new()
                },
            };
            self.add(label, shown);
            self.fresh += usize::from(fresh);
            placed += 1;
        }
        placed
    }

    /// Whether a new label still fits under [`MAX_LABELS`], past the places
    /// held for those shown before.
    fn has_room(&self) -> bool {
        self.fresh + self.reserved < MAX_LABELS
    }

    /// Place `c` at `fade` (`0..=1`) of its opacity if it fits: on the
    /// canvas, clear of the labels placed so far and the keepout, and not a
    /// duplicate of a nearby same-named label. Returns whether it did.
    fn place(&mut self, c: &Candidate, fade: f32) -> bool {
        self.place_where(c, fade, false, false, || true)
    }

    /// [`Placer::place`], also requiring `visible()`, which is asked only
    /// once the label is known to fit; `fading` lets the label hang off the
    /// canvas and draw smaller (as one on its way out does), and a `kept`
    /// label (shown in the view before) takes a place held for it under
    /// [`MAX_LABELS`] instead of a new one's.
    fn place_where(
        &mut self,
        c: &Candidate,
        fade: f32,
        fading: bool,
        kept: bool,
        visible: impl FnOnce() -> bool,
    ) -> bool {
        let style = self.style;
        let ratio = style.pixel_ratio;
        let full = !kept && !self.has_room();
        if full || self.near_same(&c.source.text, c.anchor, DUPLICATE_PX * ratio) {
            return false;
        }
        let size = style.size * c.source.size_factor * c.scale * ratio;
        let smallest = if fading { FADING_MIN_SIZE } else { 1.0 };
        if size < MIN_SIZE_PX * smallest * ratio {
            return false;
        }
        let w = style.font.measure(&c.source.text, size);
        let h = style.font.line_height(size);
        let (x, y) = (c.anchor.0 - w / 2.0, c.anchor.1 - h / 2.0);
        let pad = PADDING_PX * ratio + halo_radius(size);
        let Some(bounds) = Rect::from_xywh(x - pad, y - pad, w + 2.0 * pad, h + 2.0 * pad) else {
            return false;
        };
        if !self.free(&bounds, fading) || !visible() {
            return false;
        }
        let far = ((c.scale - FAR_SCALE) / (NEAR_SCALE - FAR_SCALE)).clamp(0.0, 1.0);
        let label = PlacedLabel {
            text: c.source.text.clone(),
            x,
            y,
            size,
            opacity: (FAR_OPACITY + (1.0 - FAR_OPACITY) * far) * fade,
            glyphs: Vec::new(),
            covered: Vec::new(),
        };
        self.add(label, [bounds]);
        self.fresh += usize::from(!kept);
        true
    }

    fn finish(self) -> Vec<PlacedLabel> {
        self.placed
    }
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

/// Where [`draw_labels`] moves covered glyphs: far enough off any canvas.
const OFF_CANVAS_PX: f32 = 1e6;

/// Draw placed labels with a halo so they read over any map.
pub fn draw_labels(pixmap: &mut Pixmap, labels: &[PlacedLabel], style: &LabelStyle) {
    for label in labels {
        let fade = |c: Color| {
            Color::from_rgba(c.red(), c.green(), c.blue(), c.alpha() * label.opacity).unwrap_or(c)
        };
        let (color, halo) = (fade(style.color), fade(style.halo));
        let r = halo_radius(label.size);
        if !label.glyphs.is_empty() {
            // Covered glyphs go far off the canvas, which clips them away.
            let glyphs: Vec<(f32, f32, f32)> = label
                .glyphs
                .iter()
                .enumerate()
                .map(|(i, &g)| match label.covered.get(i) {
                    Some(true) => (-OFF_CANVAS_PX, -OFF_CANVAS_PX, g.2),
                    _ => g,
                })
                .collect();
            let (text, size) = (&label.text, label.size);
            style
                .font
                .draw_turned(pixmap, text, size, &glyphs, color, Some((halo, r)));
            continue;
        }
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

/// Width in pixels of the halo around a label drawn at pixel size `size`.
pub fn halo_radius(size: f32) -> f32 {
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
        let bytes = include_bytes!("../../assets/test-arabic.ttf").to_vec();
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
    fn contrasting_colors_follow_the_background() {
        let font = TextFont::bundled().clone();
        let dark = LabelStyle::contrasting(font.clone(), 12.0, Color::BLACK, None, 1.0);
        assert!(dark.color.red() > 0.5 && dark.halo.red() < 0.5);
        let light = LabelStyle::contrasting(font, 12.0, Color::WHITE, None, 1.0);
        assert!(light.color.red() < 0.5 && light.halo.red() > 0.5);
    }

    fn straight_down() -> Camera {
        let lens = crate::geo::Lens {
            pitch_deg: 0.0,
            bearing_deg: 0.0,
            fov_deg: 40.0,
        };
        Camera::new(0.0, 0.0, 1.0, 512, 512, lens)
    }

    fn steady(t: &VectorTile, memory: &mut LabelMemory) -> Vec<PlacedLabel> {
        let tiles = [(placement(), t)];
        LabelSources::collect(&tiles, &style()).place_3d_steady(
            &straight_down(),
            &style(),
            (512, 512),
            &Keepout::default(),
            None,
            memory,
        )
    }

    fn names(labels: &[PlacedLabel]) -> Vec<&str> {
        labels.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn a_label_leaving_the_screen_fades_out_past_the_edge() {
        // Near the right edge of the straight-down view (world x 470 of 512).
        let town = place("Edgeton", "town", 3760, 2048);
        let tiles_owned = tile(&[town]);
        let tiles = [(placement(), &tiles_owned)];
        let sources = LabelSources::collect(&tiles, &style());
        // The same camera panned 50 px west: the town's anchor lands just
        // past the right edge.
        let lens = crate::geo::Lens {
            pitch_deg: 0.0,
            bearing_deg: 0.0,
            fov_deg: 40.0,
        };
        let panned = Camera::new(0.0, -50.0 / 512.0 * 360.0, 1.0, 512, 512, lens);
        let mut memory = LabelMemory::new(3);
        let run = |camera: &Camera, memory: &mut LabelMemory| {
            sources.place_3d_steady(
                camera,
                &style(),
                (512, 512),
                &Keepout::default(),
                None,
                memory,
            )
        };
        let full = run(&straight_down(), &mut memory);
        assert_eq!(names(&full), ["Edgeton"]);
        let full = full[0].opacity;
        let mut fading = Vec::new();
        for _ in 0..3 {
            let labels = run(&panned, &mut memory);
            fading.push(
                labels
                    .first()
                    .map(|l| ((l.opacity / full * 3.0).round(), l.x)),
            );
        }
        assert_eq!(fading[0].map(|f| f.0), Some(2.0));
        assert!(fading[0].is_some_and(|f| f.1 > 470.0), "hangs off the edge");
        assert_eq!(fading[1].map(|f| f.0), Some(1.0));
        assert_eq!(fading[2], None);
        // A label isn't started there, though.
        assert!(run(&panned, &mut LabelMemory::new(3)).is_empty());
    }

    #[test]
    fn a_road_name_slides_under_an_overlay_instead_of_vanishing() {
        use super::super::mvt::Value;
        // A road across the left half of the view.
        let road = Layer::test_line(
            "transportation_name",
            0,
            2048,
            2048,
            &[
                ("name", Value::String("Main Street".into())),
                ("class", Value::String("motorway".into())),
            ],
        );
        let t = VectorTile { layers: vec![road] };
        let tiles = [(placement(), &t)];
        let sources = LabelSources::collect(&tiles, &style());
        let edge = 128.0;
        let cover = Keepout {
            rects: vec![Rect::from_xywh(0.0, 0.0, edge, 512.0).unwrap()],
            ..Keepout::default()
        };
        let run = |keepout: &Keepout, memory: &mut LabelMemory| {
            let camera = straight_down();
            sources.place_3d_steady(&camera, &style(), (512, 512), keepout, None, memory)
        };
        let mut memory = LabelMemory::new(3);
        let shown = run(&Keepout::default(), &mut memory);
        assert_eq!(shown.len(), 1);
        assert!(shown[0].covered.is_empty());
        let full = shown[0].opacity;
        assert!(
            shown[0].glyphs.iter().any(|g| g.0 < edge)
                && shown[0].glyphs.iter().any(|g| g.0 > edge)
        );
        // A panel drawn over the road's left end from now on: the name fades
        // with its glyphs under the panel left out.
        let mut fades = Vec::new();
        for _ in 0..3 {
            let labels = run(&cover, &mut memory);
            if let Some(l) = labels.first() {
                assert_eq!(l.covered.len(), l.glyphs.len());
                for (g, &covered) in l.glyphs.iter().zip(&l.covered) {
                    if g.0 < edge - 20.0 {
                        assert!(covered, "{g:?}");
                    } else if g.0 > edge + 20.0 {
                        assert!(!covered, "{g:?}");
                    }
                }
            }
            fades.push(labels.first().map(|l| (l.opacity / full * 3.0).round()));
        }
        assert_eq!(fades, [Some(2.0), Some(1.0), None]);
        assert!(
            run(&cover, &mut LabelMemory::new(3)).is_empty(),
            "not started there"
        );
    }

    #[test]
    fn a_fading_road_name_leaves_out_glyphs_under_another_label() {
        use super::super::mvt::{Layer, Value};
        let road = Layer::test_line(
            "transportation_name",
            0,
            2048,
            2048,
            &[
                ("name", Value::String("Main Street".into())),
                ("class", Value::String("motorway".into())),
            ],
        );
        let run = |layers: Vec<Layer>, memory: &mut LabelMemory| {
            let t = VectorTile { layers };
            let tiles = [(placement(), &t)];
            LabelSources::collect(&tiles, &style()).place_3d_steady(
                &straight_down(),
                &style(),
                (512, 512),
                &Keepout::default(),
                None,
                memory,
            )
        };
        let mut memory = LabelMemory::new(3);
        let shown = run(vec![road.clone()], &mut memory);
        assert_eq!(shown.len(), 1);
        let glyphs = &shown[0].glyphs;
        let (mx, my, _) = glyphs[glyphs.len() / 2];
        // A city name lands on the middle of the road's name.
        let city = place("Ox", "town", (mx * 8.0) as i32, (my * 8.0) as i32);
        let both = || {
            vec![
                road.clone(),
                tile(std::slice::from_ref(&city)).layers.remove(0),
            ]
        };
        let labels = run(both(), &mut memory);
        let city_label = labels.iter().find(|l| l.text == "Ox").unwrap();
        let road_label = labels.iter().find(|l| l.text == "Main Street").unwrap();
        assert!(road_label.opacity < shown[0].opacity, "fading");
        let covered = &road_label.covered;
        assert_eq!(covered.len(), road_label.glyphs.len());
        assert!(
            covered[road_label.glyphs.len() / 2],
            "the middle is left out"
        );
        assert!(covered.contains(&false), "the ends still show");
        // What still shows stays off the city's name.
        let size = city_label.size;
        let width = style().font.measure("Ox", size);
        let (cx0, cx1) = (city_label.x, city_label.x + width);
        for (g, &c) in road_label.glyphs.iter().zip(covered) {
            if !c {
                assert!(g.0 < cx0 - 2.0 || g.0 > cx1 + 2.0, "{g:?} vs {cx0}..{cx1}");
            }
        }
    }

    #[test]
    fn labels_shown_before_keep_their_place_under_the_cap() {
        // More cities than the cap, in rows above and below a town.
        let rows = [30, 75, 120, 165, 210, 300, 345, 390, 435, 480];
        let mut points: Vec<TestPoint> = rows
            .iter()
            .flat_map(|&y| (0..7).map(move |c| (40 + 72 * c, y)))
            .enumerate()
            .map(|(i, (x, y))| place(&format!("C{i}"), "city", x * 8, y * 8))
            .collect();
        let town = place("Oldtown", "town", 256 * 8, 256 * 8);
        let mut memory = LabelMemory::new(1);
        let alone = steady(&tile(std::slice::from_ref(&town)), &mut memory);
        assert_eq!(names(&alone), ["Oldtown"]);
        points.push(town);
        let crowded = tile(&points);
        let fresh = steady(&crowded, &mut LabelMemory::new(1));
        assert_eq!(fresh.len(), MAX_LABELS);
        assert!(
            !names(&fresh).contains(&"Oldtown"),
            "a new town waits for room"
        );
        // The town shown before keeps its place; the cities take the rest.
        let kept = steady(&crowded, &mut memory);
        assert!(names(&kept).contains(&"Oldtown"));
        assert_eq!(kept.len(), MAX_LABELS);
        // And the count holds view after view.
        assert_eq!(steady(&crowded, &mut memory).len(), MAX_LABELS);
    }

    #[test]
    fn labels_shown_before_keep_their_place_within_a_tier() {
        let old = place("Oldtown", "town", 2048, 2048);
        let new = place("Newtown", "town", 2060, 2050);
        let mut memory = LabelMemory::new(1);
        steady(&tile(std::slice::from_ref(&old)), &mut memory);
        let both = tile(&[old, new]);
        assert_eq!(names(&steady(&both, &mut memory)), ["Oldtown"]);
        assert_eq!(names(&steady(&both, &mut LabelMemory::new(1))), ["Newtown"]);
    }

    #[test]
    fn a_more_important_label_still_wins() {
        let minor = place("Smallville", "village", 2048, 2048);
        let major = place("Metropolis", "city", 2060, 2050);
        let mut memory = LabelMemory::new(1);
        steady(&tile(std::slice::from_ref(&minor)), &mut memory);
        assert_eq!(
            names(&steady(&tile(&[minor, major]), &mut memory)),
            ["Metropolis"]
        );
    }

    #[test]
    fn labels_fade_out_behind_a_hill() {
        use crate::terrain::mesh::tests::{hill_camera, hill_origin, terrain};
        let hill = terrain();
        let cam = hill_camera(75.0);
        let origin = hill_origin();
        // A peak label just past the summit, which the hill hides.
        let t = VectorTile {
            layers: vec![Layer::test_points(
                "place",
                &[place("Behind", "town", 2048, 1560)],
            )],
        };
        let tiles = [(
            TilePlacement {
                id: TileId {
                    zoom: 14,
                    x: 0,
                    y: 0,
                },
                dx: origin.0,
                dy: origin.1,
                size: 512.0,
            },
            &t,
        )];
        let sources = LabelSources::collect(&tiles, &style());
        let mut memory = LabelMemory::new(3);
        let mut run = |terrain: Option<&Terrain>| {
            let labels = sources.place_3d_steady(
                &cam,
                &style(),
                (320, 240),
                &Keepout::default(),
                terrain,
                &mut memory,
            );
            labels.first().map(|l| l.opacity)
        };
        let full = run(None).expect("visible on a flat map");
        let fading: Vec<Option<f32>> = (0..3).map(|_| run(Some(&hill))).collect();
        let ratio = |o: Option<f32>| o.map(|o| (o / full * 3.0).round() as u32);
        assert_eq!(
            fading.into_iter().map(ratio).collect::<Vec<_>>(),
            [Some(2), Some(1), None]
        );
    }

    #[test]
    fn road_names_hide_behind_a_hill_between_points() {
        use super::super::mvt::Value;
        use crate::terrain::mesh::tests::{hill_camera, hill_origin, terrain};
        let hill = terrain();
        let cam = hill_camera(75.0);
        let origin = hill_origin();
        // Two points both in sight, with the hill between them in the middle.
        let road = Layer::test_line(
            "transportation_name",
            1200,
            2900,
            1500,
            &[
                ("name", Value::String("Ridge Road".into())),
                ("class", Value::String("primary".into())),
            ],
        );
        let t = VectorTile { layers: vec![road] };
        let tiles = [(
            TilePlacement {
                id: TileId {
                    zoom: 14,
                    x: 0,
                    y: 0,
                },
                dx: origin.0,
                dy: origin.1,
                size: 512.0,
            },
            &t,
        )];
        let sources = LabelSources::collect(&tiles, &style());
        let place = |terrain: Option<&Terrain>| {
            sources.place_3d_steady(
                &cam,
                &style(),
                (320, 240),
                &Keepout::default(),
                terrain,
                &mut LabelMemory::new(1),
            )
        };
        assert!(!place(None).is_empty(), "shown on a flat map");
        assert!(place(Some(&hill)).is_empty(), "the hill hides its middle");
        // In a sequence it fades out behind the hill instead of vanishing.
        let mut memory = LabelMemory::new(3);
        let mut step = |terrain: Option<&Terrain>| {
            let labels = sources.place_3d_steady(
                &cam,
                &style(),
                (320, 240),
                &Keepout::default(),
                terrain,
                &mut memory,
            );
            labels.iter().map(|l| l.opacity).fold(0.0, f32::max)
        };
        let full = step(None);
        let fading: Vec<u32> = (0..3)
            .map(|_| (step(Some(&hill)) / full * 3.0).round() as u32)
            .collect();
        assert_eq!(fading, [2, 1, 0]);
    }

    #[test]
    fn places_sharing_a_name_are_remembered_apart() {
        let here = place("Springfield", "town", 600, 600);
        let there = place("Springfield", "village", 3000, 3000);
        let major = place("Metropolis", "city", 3010, 3005);
        let mut memory = LabelMemory::new(4);
        steady(&tile(&[here]), &mut memory);
        let next = steady(&tile(&[there, major]), &mut memory);
        assert_eq!(names(&next), ["Metropolis"]);
        assert!(next[0].opacity < 1.0, "a new place fades in");
    }

    #[test]
    fn new_labels_fade_in_after_the_first_view() {
        let a = place("Elsewhere", "town", 600, 600);
        let b = place("Metropolis", "city", 3000, 3000);
        let mut memory = LabelMemory::new(4);
        let first = steady(&tile(std::slice::from_ref(&a)), &mut memory);
        assert_eq!(first[0].opacity, 1.0, "the first view shows everything");
        let mut seen = Vec::new();
        for _ in 0..5 {
            let labels = steady(&tile(&[a.clone(), b.clone()]), &mut memory);
            let opacity = |name: &str| labels.iter().find(|l| l.text == name).unwrap().opacity;
            assert_eq!(opacity("Elsewhere"), 1.0);
            seen.push(opacity("Metropolis"));
        }
        assert_eq!(seen, [0.25, 0.5, 0.75, 1.0, 1.0]);
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
    fn covered_glyphs_are_left_out() {
        let draw = |covered: Vec<bool>| {
            let mut pixmap = Pixmap::new(120, 40).unwrap();
            pixmap.fill(Color::BLACK);
            let label = PlacedLabel {
                text: "HH".into(),
                x: 20.0,
                y: 10.0,
                size: 20.0,
                opacity: 1.0,
                glyphs: vec![(30.0, 20.0, 0.0), (90.0, 20.0, 0.0)],
                covered,
            };
            draw_labels(&mut pixmap, &[label], &style());
            let lit = |x0: u32, x1: u32| {
                (x0..x1)
                    .flat_map(|x| (0..40).map(move |y| (x, y)))
                    .filter(|&(x, y)| pixmap.pixel(x, y).unwrap().red() > 128)
                    .count()
            };
            (lit(0, 60), lit(60, 120))
        };
        let (left, right) = draw(Vec::new());
        assert!(left > 0 && right > 0);
        assert_eq!(draw(vec![true, false]), (0, right));
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
            glyphs: Vec::new(),
            covered: Vec::new(),
        };
        draw_labels(&mut pixmap, &[label], &style());
        assert_ne!(before, pixmap.data());
    }

    #[test]
    fn road_names_follow_their_road() {
        use super::super::mvt::Value;
        let road = Layer::test_line(
            "transportation_name",
            400,
            3700,
            2048,
            &[
                ("name", Value::String("Main Street".into())),
                ("class", Value::String("motorway".into())),
            ],
        );
        let t = VectorTile { layers: vec![road] };
        let labels = run(&t, &Keepout::default());
        assert!(!labels.is_empty());
        let label = &labels[0];
        assert_eq!(label.text, "Main Street");
        assert_eq!(label.glyphs.len(), "Main Street".chars().count());
        assert!(label
            .glyphs
            .iter()
            .all(|g| (g.1 - 256.0).abs() < 0.5 && g.2.abs() < 1e-6));
        assert!(
            label.glyphs.windows(2).all(|g| g[1].0 > g[0].0),
            "left to right"
        );
        // The road under the label keeps it off: a track across it.
        let keepout = Keepout {
            tracks: vec![vec![(0.0, 256.0), (512.0, 256.0)]],
            track_half_width: 2.0,
            ..Keepout::default()
        };
        assert!(run(&t, &keepout).is_empty());
        let mut pixmap = Pixmap::new(512, 512).unwrap();
        draw_labels(&mut pixmap, &labels, &style());
        assert!(pixmap.pixels().iter().any(|p| p.alpha() > 0));
    }

    #[test]
    fn a_road_cut_at_a_tile_edge_is_named_across_it() {
        use super::super::mvt::Value;
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
    fn road_name_copies_stay_put_on_the_road_as_the_view_pans() {
        use super::super::mvt::Value;
        let road = Layer::test_line(
            "transportation_name",
            0,
            4096,
            2048,
            &[
                ("name", Value::String("Main Street".into())),
                ("class", Value::String("motorway".into())),
            ],
        );
        let t = VectorTile { layers: vec![road] };
        let world_middles = |pan: f32| -> Vec<f32> {
            let project = |p: &TilePlacement, x: f64, y: f64| {
                Some((
                    (p.dx + x * p.size) as f32 - pan,
                    (p.dy + y * p.size) as f32,
                    1.0,
                ))
            };
            let tiles = [(placement(), &t)];
            let mut middles: Vec<f32> = place_labels(
                &tiles,
                &style(),
                project,
                (512.0, 512.0),
                &Keepout::default(),
            )
            .iter()
            .map(|l| l.glyphs[l.glyphs.len() / 2].0 + pan)
            .collect();
            middles.sort_by(f32::total_cmp);
            middles
        };
        let (still, panned) = (world_middles(0.0), world_middles(37.0));
        assert!(!still.is_empty() && !panned.is_empty());
        // Every copy seen in both views is at the same place on the road.
        let shared: Vec<&f32> = panned
            .iter()
            .filter(|m| still.iter().any(|s| (*s - **m).abs() < 1.0))
            .collect();
        assert!(!shared.is_empty(), "{still:?} vs {panned:?}");
        assert!(panned
            .iter()
            .all(|m| still.iter().any(|s| (s - m).abs() < 1.0)
                || *m < 37.0 + 60.0
                || *m > 512.0 - 60.0));
    }

    #[test]
    fn road_name_copies_hold_when_off_screen_tiles_change() {
        use super::super::mvt::Value;
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
        use super::super::mvt::Value;
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
