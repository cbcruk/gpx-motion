//! Choosing which labels to show and where, flat or in 3D.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use tiny_skia::Rect;

use super::draw::halo_radius;
use super::keepout::{center, overlaps, Keepout, KeepoutGrid};
use super::line_labels::{Glyph, Polyline, MAX_TOTAL_BEND, SHAPED_MAX_TOTAL_BEND};
use super::line_stitch::stitch;
use super::memory::{LabelKey, LabelMemory, KEY_ZOOM};
use super::sources::{line_order, point_order, LabelSource, LabelSources, LineSource};
use super::style::{
    LabelStyle, DUPLICATE_PX, EDGE_FADE_PX, FADING_MIN_SIZE, FAR_OPACITY, FAR_SCALE,
    LABEL_FAR_FACTOR, LINE_REPEAT_PX, LINE_SIZE_FACTOR, MAX_LABELS, MIN_SIZE_PX, NEAR_SCALE,
    PADDING_PX,
};
use crate::geo::camera::{Projector, NEAR};
use crate::geo::Camera;
use crate::render::text::needs_shaping;
use crate::terrain::Terrain;
use crate::tiles::TilePlacement;
use crate::vector::key_hash::KeyHash;
use crate::vector::mvt::VectorTile;

/// A label placed on screen, ready for [`draw_labels`](super::draw_labels).
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

struct Candidate<'a> {
    source: &'a LabelSource,
    anchor: (f32, f32),
    scale: f32,
    /// Whether the anchor is on the canvas; one just past its edge may only
    /// fade out there.
    on_canvas: bool,
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
        let view = SteadyView {
            camera,
            projector: camera.projector(),
            style,
            size,
            terrain,
            memory,
            unit: 2f64.powi(KEY_ZOOM) / 2f64.powf(camera.zoom),
            fade: memory.fade_frames.max(1),
        };
        let candidates = view.point_candidates(&points);
        let lines = view.line_keys(&all_lines);
        let mut placer = Placer::new(style, size, keepout);
        placer.reserved = view.reserved(&candidates, &lines);
        let mut levels: HashMap<LabelKey, u32, KeyHash> = HashMap::default();
        view.place_points(&mut placer, &candidates, &mut levels);
        let copies = view.place_lines(&mut placer, &lines, &mut levels);
        memory.levels = levels;
        memory.copies = copies;
        memory.primed = true;
        placer.finish()
    }
}

/// A point label that may show in a steadied 3D view.
struct SteadyPoint<'a> {
    candidate: Candidate<'a>,
    /// What [`LabelMemory`] knows it by.
    key: LabelKey,
    /// Its world point and ground height.
    at: (f64, f64, f64),
}

/// One view of [`LabelSources::place_3d_steady_parts`]: how it projects,
/// and what the memory of the view before says about each label.
struct SteadyView<'a> {
    camera: &'a Camera,
    projector: Projector,
    style: &'a LabelStyle,
    size: (f32, f32),
    terrain: Option<&'a Terrain>,
    memory: &'a LabelMemory,
    /// Label keys per world pixel, so a key holds while the camera zooms.
    unit: f64,
    /// Frames a label takes to fade in or out, at least one.
    fade: u32,
}

impl SteadyView<'_> {
    /// Fade level the label with `key` had in the view before (`0` if none).
    fn level(&self, key: &LabelKey) -> u32 {
        self.memory.levels.get(key).copied().unwrap_or(0)
    }

    /// Highest fade level any of a line label's `keys` had.
    fn line_level(&self, keys: &[LabelKey]) -> u32 {
        keys.iter().map(|k| self.level(k)).max().unwrap_or(0)
    }

    /// Copies a line label drew in the view before (`0` if it didn't show).
    fn kept_copies(&self, keys: &[LabelKey]) -> usize {
        match self.line_level(keys) {
            0 => 0,
            _ => keys
                .iter()
                .filter_map(|k| self.memory.copies.get(k))
                .copied()
                .max()
                .unwrap_or(0),
        }
    }

    /// Fade level of a label that shows now, after `was`.
    fn next_level(&self, was: u32) -> u32 {
        if self.memory.primed {
            (was + 1).min(self.fade)
        } else {
            self.fade
        }
    }

    /// Point labels that project near enough the canvas, with their keys
    /// and world points.
    fn point_candidates<'s>(&self, points: &[&'s LabelSource]) -> Vec<SteadyPoint<'s>> {
        let size = self.size;
        points
            .iter()
            .copied()
            .filter_map(|source| {
                let p = &source.placement;
                let (wx, wy) = (
                    p.dx + source.point.0 * p.size,
                    p.dy + source.point.1 * p.size,
                );
                let h = self.terrain.map_or(0.0, |t| t.height_at_world(wx, wy));
                let (sx, sy, scale) = project_3d(&self.projector, wx, wy, h)?;
                let slack = EDGE_FADE_PX * self.style.pixel_ratio;
                let within = |v: f32, max: f32, slack: f32| (-slack..=max + slack).contains(&v);
                if !within(sx, size.0, slack) || !within(sy, size.1, slack) {
                    return None;
                }
                let on_canvas = within(sx, size.0, 0.0) && within(sy, size.1, 0.0);
                let key = (
                    source.key,
                    (wx * self.unit).floor() as i64,
                    (wy * self.unit).floor() as i64,
                );
                let candidate = Candidate {
                    source,
                    anchor: (sx, sy),
                    scale,
                    on_canvas,
                };
                Some(SteadyPoint {
                    candidate,
                    key,
                    at: (wx, wy, h),
                })
            })
            .collect()
    }

    /// Shown line labels with their keys, in placing order.
    ///
    /// Line labels are remembered by name and where each of their tile
    /// pieces' middles is (all of them, so the memory holds while tiles come
    /// and go).
    fn line_keys<'s>(&self, lines: &[&'s LineSource]) -> Vec<(&'s LineSource, Vec<LabelKey>)> {
        let mut lines: Vec<(&LineSource, Vec<LabelKey>)> = lines
            .iter()
            .copied()
            .filter(|line| line.shown(self.style))
            .map(|line| {
                let keys = line
                    .marks
                    .iter()
                    .map(|m| {
                        (
                            line.key,
                            (m.x * self.unit).floor() as i64,
                            (m.y * self.unit).floor() as i64,
                        )
                    })
                    .collect();
                (line, keys)
            })
            .collect();
        lines.sort_by_cached_key(|(line, keys)| (line.rank, self.line_level(keys) == 0));
        lines
    }

    /// Places under the cap held for labels shown last view.
    ///
    /// Those labels keep their places (those fading just off the canvas too,
    /// as they may come back); new ones get what is left.
    fn reserved(
        &self,
        candidates: &[SteadyPoint],
        lines: &[(&LineSource, Vec<LabelKey>)],
    ) -> usize {
        let kept_points: HashSet<&LabelKey, KeyHash> = candidates
            .iter()
            .filter(|c| self.level(&c.key) > 0)
            .map(|c| &c.key)
            .collect();
        kept_points.len()
            + lines
                .iter()
                .map(|(_, keys)| self.kept_copies(keys))
                .sum::<usize>()
    }

    /// Place the point labels that fit, then fade out those that dropped
    /// out, recording each one's new level in `levels`.
    fn place_points(
        &self,
        placer: &mut Placer,
        candidates: &[SteadyPoint],
        levels: &mut HashMap<LabelKey, u32, KeyHash>,
    ) {
        // Looking along the line of sight is costly, so only labels that
        // would otherwise go in are checked.
        let in_sight = |&(x, y, h): &(f64, f64, f64)| {
            !self
                .terrain
                .is_some_and(|t| t.hides_point(self.camera, x, y, h))
        };
        let fade = self.fade;
        let mut order: Vec<usize> = (0..candidates.len()).collect();
        // Stable: remembered labels go first within their tier only.
        order.sort_by_key(|&i| {
            (
                candidates[i].candidate.source.priority.0,
                self.level(&candidates[i].key) == 0,
            )
        });
        for i in order {
            let SteadyPoint { candidate, key, at } = &candidates[i];
            if levels.contains_key(key) {
                continue;
            }
            let next = self.next_level(self.level(key));
            if !candidate.on_canvas {
                continue;
            }
            let opacity = next as f32 / fade as f32;
            let kept = self.level(key) > 0;
            if placer.place_where(candidate, opacity, false, kept, || in_sight(at)) {
                levels.insert(*key, next);
            }
        }
        // Labels that dropped out (pushed aside, behind a hill, or leaving the
        // screen) fade away where they still fit, partly off the canvas too.
        for SteadyPoint { candidate, key, .. } in candidates {
            let was = self.level(key);
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
    }

    /// Place the line labels that fit, then fade out those that dropped out,
    /// recording each one's new level in `levels`. Returns how many copies
    /// of each were drawn.
    fn place_lines(
        &self,
        placer: &mut Placer,
        lines: &[(&LineSource, Vec<LabelKey>)],
        levels: &mut HashMap<LabelKey, u32, KeyHash>,
    ) -> HashMap<LabelKey, usize, KeyHash> {
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
        for (line, keys) in lines {
            if keys.iter().any(|k| levels.contains_key(k)) {
                continue;
            }
            let next = self.next_level(self.line_level(keys));
            let drawn = self.place_line(placer, line, next, false, self.kept_copies(keys));
            if drawn > 0 {
                remember(levels, keys, next, drawn);
            }
        }
        // Line labels that dropped out fade away where they still fit. One
        // that doesn't is still remembered at the level it would have faded
        // to, so if a copy comes back (the line's copies move in and out of
        // view as the camera turns) it carries on from there instead of
        // fading in from nothing.
        for (line, keys) in lines {
            let was = self.line_level(keys);
            if was <= 1 || keys.iter().any(|k| levels.contains_key(k)) {
                continue;
            }
            let drawn = self.place_line(placer, line, was - 1, true, usize::MAX);
            remember(levels, keys, was - 1, drawn);
        }
        copies
    }

    /// World point and ground height of tile-normalized `(x, y)` in `p`.
    fn world(&self, p: &TilePlacement, x: f64, y: f64) -> (f64, f64, f64) {
        let (wx, wy) = (p.dx + x * p.size, p.dy + y * p.size);
        (
            wx,
            wy,
            self.terrain.map_or(0.0, |t| t.height_at_world(wx, wy)),
        )
    }

    /// Place copies of `line` at fade `level` in each of its screen runs
    /// (see [`Placer::place_line`]); returns how many were placed.
    fn place_line(
        &self,
        placer: &mut Placer,
        line: &LineSource,
        level: u32,
        fading: bool,
        kept: usize,
    ) -> usize {
        let project = |p: &TilePlacement, x: f64, y: f64| {
            let (wx, wy, h) = self.world(p, x, y);
            project_3d(&self.projector, wx, wy, h)
        };
        let margin = LINE_REPEAT_PX * self.style.pixel_ratio;
        let mut copies = 0;
        for run in project_line(line, &project, self.size, margin) {
            // A line label whose stretch of line a hill hides is left out.
            let hidden = |a: usize, b: usize, t: f32| {
                self.terrain.is_some_and(|ground| {
                    let (pa, pb) = (line.points[a], line.points[b]);
                    let t = t as f64;
                    let (x, y) = (pa.0 + (pb.0 - pa.0) * t, pa.1 + (pb.1 - pa.1) * t);
                    let (wx, wy, h) = self.world(&line.placement, x, y);
                    ground.hides_point(self.camera, wx, wy, h)
                })
            };
            let opacity = level as f32 / self.fade as f32;
            let kept = kept.saturating_sub(copies);
            copies += placer.place_line(line, &run, opacity, fading, kept, hidden);
        }
        copies
    }
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
        let line_run = LineRun {
            line,
            run,
            polyline: Polyline::new(run.points.clone()),
            fade,
            fading,
            spacing: LINE_REPEAT_PX * self.style.pixel_ratio,
            hidden,
        };
        let mut placed = 0;
        for center in line_run.centers() {
            let fresh = placed >= kept;
            if fresh && !self.has_room() {
                break;
            }
            let Some((label, shown)) = self.line_copy(&line_run, center) else {
                continue;
            };
            self.add(label, shown);
            self.fresh += usize::from(fresh);
            placed += 1;
        }
        placed
    }

    /// The copy of a line label centered `center` along its run, with the
    /// boxes its drawn glyphs take, if it fits there.
    fn line_copy<H: Fn(usize, usize, f32) -> bool>(
        &self,
        line_run: &LineRun<H>,
        center: f32,
    ) -> Option<(PlacedLabel, Vec<Rect>)> {
        let (style, text) = (self.style, line_run.line.text.as_str());
        let (run, polyline, fading) = (line_run.run, &line_run.polyline, line_run.fading);
        let ratio = style.pixel_ratio;
        // Size and fade follow the depth where this copy sits.
        let k = polyline
            .lengths
            .partition_point(|&l| l <= center)
            .clamp(1, run.points.len() - 1);
        let scale = (run.scales[k - 1] + run.scales[k]) / 2.0;
        let size = style.size * LINE_SIZE_FACTOR * scale * ratio;
        let smallest = if fading { FADING_MIN_SIZE } else { 1.0 };
        if size < MIN_SIZE_PX * smallest * ratio {
            return None;
        }
        let advances: Vec<f32> = line_run.line.advances.iter().map(|a| a * size).collect();
        let height = style.font.line_height(size);
        let pad = PADDING_PX * ratio + halo_radius(size);
        let bend = if needs_shaping(text) {
            SHAPED_MAX_TOTAL_BEND
        } else {
            MAX_TOTAL_BEND
        };
        let glyphs = polyline.layout(&advances, center, bend)?;
        let mid = glyphs[glyphs.len() / 2];
        if self.near_same(text, (mid.x, mid.y), line_run.spacing * 0.9) {
            return None;
        }
        // A fading copy may run off the canvas, or under what labels keep
        // clear of (the track, overlays drawn over it), as long as some
        // glyphs still show: it slips away rather than vanishing. Shaped
        // text can't lose single glyphs, so it must stay clear.
        let boxes = glyph_boxes(&glyphs, &advances, height, pad)?;
        let covered = if fading {
            self.fading_cover(line_run, &glyphs, &boxes)?
        } else if boxes.iter().all(|r| self.free(r, false)) {
            Vec::new()
        } else {
            return None;
        };
        let probes = [glyphs[0], mid, glyphs[glyphs.len() - 1]];
        let out_of_sight = !fading && probes.iter().any(|g| line_run.hidden_at(g));
        if out_of_sight {
            return None;
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
            opacity: (FAR_OPACITY + (1.0 - FAR_OPACITY) * far) * line_run.fade,
            glyphs: glyphs.iter().map(|g| (g.x, g.y, g.angle)).collect(),
            covered: if covered.contains(&true) {
                covered
            } else {
                Vec::new()
            },
        };
        Some((label, shown))
    }

    /// Which glyphs of a fading line label copy to leave undrawn, or `None`
    /// when too few would still show.
    ///
    /// A fading copy's glyphs that something would be drawn over or under
    /// (the keepout, another label, a hill in front) are left out, so it
    /// never overlaps anything while it goes.
    fn fading_cover<H: Fn(usize, usize, f32) -> bool>(
        &self,
        line_run: &LineRun<H>,
        glyphs: &[Glyph],
        boxes: &[Rect],
    ) -> Option<Vec<bool>> {
        let text = line_run.line.text.as_str();
        let fits = |covered: &[bool]| {
            let shows = boxes
                .iter()
                .zip(covered)
                .any(|(r, &c)| self.touches_canvas(r) && !c);
            shows && (!covered.contains(&true) || !needs_shaping(text))
        };
        let mut covered: Vec<bool> = boxes
            .iter()
            .map(|r| {
                self.touches_canvas(r)
                    && (!self.clear_of_keepout(r, true)
                        || self.boxes.iter().any(|(_, b)| overlaps(b, r)))
            })
            .collect();
        if !fits(&covered) {
            return None;
        }
        // The costly look along the line of sight comes last, only for
        // glyphs that would still show.
        for ((r, g), c) in boxes.iter().zip(glyphs).zip(&mut covered) {
            if !*c && self.touches_canvas(r) {
                *c = line_run.hidden_at(g);
            }
        }
        fits(&covered).then_some(covered)
    }

    /// Whether `r` overlaps the canvas at all.
    fn touches_canvas(&self, r: &Rect) -> bool {
        r.right() > 0.0 && r.bottom() > 0.0 && r.left() < self.canvas.0 && r.top() < self.canvas.1
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

/// A line's screen run that [`Placer::place_line`] labels.
struct LineRun<'r, H> {
    line: &'r LineSource,
    run: &'r Run,
    polyline: Polyline,
    /// Share of full opacity the copies get.
    fade: f32,
    /// Whether the label is on its way out.
    fading: bool,
    /// Least screen distance between copies of the name.
    spacing: f32,
    /// Whether the ground `t` of the way from line point `a` to `b` is out
    /// of sight, as `hidden(a, b, t)`.
    hidden: H,
}

impl<H: Fn(usize, usize, f32) -> bool> LineRun<'_, H> {
    /// Whether the ground under glyph `g` is out of sight.
    fn hidden_at(&self, g: &Glyph) -> bool {
        let sources = &self.run.sources;
        (self.hidden)(sources[g.segment], sources[g.segment + 1], g.t)
    }

    /// Where copies may be centered along the run, in its screen pixels,
    /// best first.
    fn centers(&self) -> Vec<f32> {
        let (line, run, polyline) = (self.line, self.run, &self.polyline);
        let length = polyline.length();
        // Copies may sit every half `spacing` (in the line's own pixels)
        // around the middle of each tile piece joined into the line, within
        // the piece (then at its ends), nearest a middle first, wherever this
        // run shows them:
        // so each stays on its stretch of line as the view, and the tiles
        // loaded for it, change. Copies of one name still keep `spacing`
        // apart on screen.
        let total = line.lengths.last().copied().unwrap_or(0.0);
        let step = self.spacing as f64 / 2.0;
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
        positions
            .into_iter()
            .filter_map(|(_, _, s)| on_run(s))
            .filter(|c| (0.0..=length).contains(c))
            .collect()
    }
}

/// Screen box of each glyph of a line label, padded by `pad`, or `None` if
/// any is degenerate.
fn glyph_boxes(glyphs: &[Glyph], advances: &[f32], height: f32, pad: f32) -> Option<Vec<Rect>> {
    glyphs
        .iter()
        .zip(advances)
        .map(|(g, &advance)| {
            let (sin, cos) = (g.angle.sin().abs(), g.angle.cos().abs());
            let hw = cos * advance / 2.0 + sin * height / 2.0 + pad;
            let hh = sin * advance / 2.0 + cos * height / 2.0 + pad;
            Rect::from_ltrb(g.x - hw, g.y - hh, g.x + hw, g.y + hh)
        })
        .collect()
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

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::TileId;
    use crate::vector::labels::draw_labels;
    use crate::vector::labels::test_support::*;
    use crate::vector::mvt::Layer;
    use tiny_skia::Pixmap;

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
        use crate::vector::mvt::Value;
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
        use crate::vector::mvt::{Layer, Value};
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
    fn labels_fade_out_behind_a_hill() {
        use crate::test_support::hill::{hill_camera, hill_origin, terrain};
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
        use crate::test_support::hill::{hill_camera, hill_origin, terrain};
        use crate::vector::mvt::Value;
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
    fn road_names_follow_their_road() {
        use crate::vector::mvt::Value;
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
    fn road_name_copies_stay_put_on_the_road_as_the_view_pans() {
        use crate::vector::mvt::Value;
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
}
