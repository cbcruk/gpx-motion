//! The 3D scene drawn on the CPU: the raised track, its relief, and the
//! horizon fog.

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint, PathBuilder,
    Pixmap, Point, Rect, Shader, SpreadMode, Stroke, Transform,
};

use super::frame::FrameStyle;
use super::occlusion::{DepthPoint, NearDepth};
use super::ramp::elevation_color;
use crate::anim::{Elevation, Sample, Timeline};
use crate::color::{scale_alpha, with_alpha};
use crate::geo::camera::Camera;
use crate::geo::camera::NEAR;
use crate::geo::mercator::{lat_to_pixel, lon_to_pixel};
use crate::gpx::TrackPoint;
use crate::math::lerp;
use crate::terrain::{DepthMap, Terrain};

/// Raises track elevations into the 3D scene.
///
/// Heights are measured from the track's lowest point (which sits on the map
/// plane) and multiplied by `exaggeration`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Relief {
    /// Elevation in meters that stands on the map plane (the track's lowest).
    pub base_m: f64,
    /// Vertical scale applied to heights above `base_m`.
    pub exaggeration: f64,
}

/// Most automatic exaggeration over terrain: the automatic factor is tuned
/// to the track's climb and would spike the hills around a flat route.
pub(crate) const TERRAIN_MAX_EXAGGERATION: f64 = 2.0;

impl Relief {
    /// The same relief with the exaggeration capped at 2×, for drawing over
    /// terrain (the automatic factor would spike the hills around a flat
    /// route).
    #[must_use]
    pub fn over_terrain(mut self) -> Self {
        self.exaggeration = self.exaggeration.min(TERRAIN_MAX_EXAGGERATION);
        self
    }

    /// Pick an exaggeration that makes the climb readable: the relief is
    /// stretched toward ~12% of the track's horizontal extent, between 1× and
    /// 6×. Flat or elevation-less tracks get 1× over a zero base.
    pub fn auto(timeline: &Timeline) -> Self {
        let Some(elevation) = timeline.elevation() else {
            return Self {
                base_m: 0.0,
                exaggeration: 1.0,
            };
        };
        let relief = elevation.max - elevation.min;
        let extent = horizontal_extent_m(timeline);
        let exaggeration = if relief < 1.0 {
            1.0
        } else {
            (0.12 * extent / relief).clamp(1.0, 6.0)
        };
        Self {
            base_m: elevation.min,
            exaggeration,
        }
    }

    /// Scene height in meters of every timeline vertex, using its smoothed
    /// elevation.
    pub fn heights_m(&self, timeline: &Timeline) -> Vec<f64> {
        let elevation = timeline.elevation();
        (0..timeline.len())
            .map(|i| self.height_m(elevation.map(|e| e.vertex(i))))
            .collect()
    }

    /// Scene height in meters for an elevation (`None` stays on the ground).
    pub fn height_m(&self, ele: Option<f64>) -> f64 {
        ele.map_or(0.0, |e| (e - self.base_m).max(0.0) * self.exaggeration)
    }
}

/// What a 3D scene is seen through and stands on: the camera, the relief
/// that raises elevations, and optionally the terrain under the route.
#[derive(Clone, Copy)]
pub struct SceneView<'a> {
    /// The camera the scene is seen through.
    pub camera: &'a Camera,
    /// How track elevations are raised.
    pub relief: Relief,
    /// Ground heights under the track; `None` draws on a flat map.
    pub terrain: Option<&'a Terrain>,
    /// Depth of the drawn terrain (from [`crate::terrain::draw_terrain`]);
    /// track parts behind the hills are left out.
    pub occlusion: Option<&'a DepthMap>,
}

/// Draw the track in 3D: the route as a faint raised line with a ghost
/// curtain, the tail ending at `u` as a colored curtain and bright line over
/// it, and the marker with a drop line to its ground shadow.
///
/// `tail_u` is the tail length as a fraction of progress; the tail fades
/// toward its start into the ghost like the 2D tail. `1.0` or more draws all
/// progress so far at full strength, with no ghost under it.
///
/// Curtain quads are depth-sorted back to front; lines and the marker are
/// drawn last so they stay legible. With a terrain depth map
/// ([`SceneView::occlusion`]) the track is hidden behind hills per pixel:
/// there the tail shows only as a faint thin line, and the rest of the
/// track and every curtain not at all.
pub fn draw_track_3d(
    pixmap: &mut Pixmap,
    timeline: &Timeline,
    view: &SceneView,
    u: f64,
    tail_u: f64,
    style: &FrameStyle,
) {
    let track = TrackLift::new(timeline, view, u, tail_u, style);
    let done = vec![track.prepare(timeline.window(track.u0, u))];
    let mut ghost = vec![track.prepare(timeline.window(u, 1.0))];
    if tail_u < 1.0 {
        ghost.push(track.prepare(timeline.window(0.0, u)));
    }
    let masks = view.occlusion.map_or_else(TrackMasks::default, |terrain| {
        track.occlusion_masks(pixmap.width(), pixmap.height(), terrain, &done, &ghost)
    });
    track.draw_curtains(pixmap, &done, &ghost, masks.curtain.as_ref());
    track.draw_lines(pixmap, &done, &ghost, &masks);
    track.draw_marker(pixmap, timeline, u);
}

/// A curtain run ready to fill: its mean depth, outline, and color.
type CurtainQuad = (f64, tiny_skia::Path, Color);

/// Masks each kind of track part is drawn through over a terrain depth map;
/// all `None` on a flat map.
#[derive(Default)]
struct TrackMasks {
    curtain: Option<Mask>,
    ghost: Option<Mask>,
    tail: Option<Mask>,
    /// Where the tail is behind a hill, for its faint line.
    hidden_tail: Option<Mask>,
}

/// What [`draw_track_3d`] shares between its steps: how samples are colored,
/// faded, and lifted onto the scene.
struct TrackLift<'a> {
    view: SceneView<'a>,
    style: &'a FrameStyle,
    /// Elevation coloring, when enabled and the track has elevations.
    elevation: Option<&'a Elevation>,
    tail_u: f64,
    /// Progress where the tail starts.
    u0: f64,
    /// Progress the tail spans, never zero.
    span: f64,
}

impl<'a> TrackLift<'a> {
    fn new(
        timeline: &'a Timeline,
        view: &SceneView<'a>,
        u: f64,
        tail_u: f64,
        style: &'a FrameStyle,
    ) -> Self {
        let u0 = if tail_u >= 1.0 {
            0.0
        } else {
            (u - tail_u).max(0.0)
        };
        Self {
            view: *view,
            style,
            elevation: timeline.elevation().filter(|_| style.color_by_elevation),
            tail_u,
            u0,
            span: (u - u0).max(f64::EPSILON),
        }
    }

    /// Tail color of the segment `a`–`b`, by elevation when enabled.
    fn color_of(&self, a: &Sample, b: &Sample) -> Color {
        match (self.elevation, a.ele, b.ele) {
            (Some(e), Some(ea), Some(eb)) => {
                let t = e.normalize((ea + eb) / 2.0);
                elevation_color((t * COLOR_LEVELS).round() / COLOR_LEVELS)
            }
            _ => self.style.tail_color,
        }
    }

    /// Opacity factor of a tail segment ending at `b`, fading toward the
    /// tail's start.
    fn recency(&self, b: &Sample) -> f32 {
        if self.tail_u >= 1.0 {
            1.0
        } else {
            let r = ((b.u - self.u0) / self.span).clamp(0.0, 1.0);
            ((r * FADE_LEVELS).ceil() / FADE_LEVELS) as f32
        }
    }

    /// Terrain height in world pixels at a coordinate (`0` on a flat map).
    fn ground(&self, lat: f64, lon: f64) -> f64 {
        self.view.terrain.map_or(0.0, |t| t.height_at(lat, lon))
    }

    /// Height of a sample in world pixels, never below the ground.
    fn lift(&self, s: &Sample) -> f64 {
        self.view
            .camera
            .meters_to_px(self.view.relief.height_m(s.ele), s.lat)
            .max(self.ground(s.lat, s.lon))
    }

    /// Screen position of a sample at its lifted height.
    fn raised(&self, s: &Sample) -> Option<(f64, f64)> {
        self.view.camera.project(s.lat, s.lon, self.lift(s))
    }

    /// Screen position and view depth of a sample at height `h`, unless it is
    /// behind the near plane.
    fn at_depth(&self, s: &Sample, h: f64) -> Option<DepthPoint> {
        let camera = self.view.camera;
        let point = camera.view(
            lon_to_pixel(s.lon, camera.zoom),
            lat_to_pixel(s.lat, camera.zoom),
            h,
        );
        (point.depth >= NEAR).then(|| {
            let (x, y) = camera.to_screen(point);
            (x, y, point.depth)
        })
    }

    /// Whether the lifted sample is behind the terrain.
    fn hidden(&self, s: &Sample) -> bool {
        let camera = self.view.camera;
        self.view.occlusion.is_some_and(|depth| {
            let (x, y) = (
                lon_to_pixel(s.lon, camera.zoom),
                lat_to_pixel(s.lat, camera.zoom),
            );
            let point = camera.view(x, y, self.lift(s));
            if point.depth < NEAR {
                return false;
            }
            let (sx, sy) = camera.to_screen(point);
            depth.hides(sx, sy, point.depth)
        })
    }

    /// Ready a window of samples for drawing: densified over terrain, then
    /// thinned on screen.
    fn prepare(&self, samples: Vec<Sample>) -> Vec<Sample> {
        // Over terrain, long segments are split so the line follows the ground
        // instead of cutting through hills.
        let samples = match self.view.terrain {
            Some(_) => densify(samples, self.view.camera, TERRAIN_STEP_PX),
            None => samples,
        };
        thin(samples, |s| self.raised(s))
    }

    /// Masks of the pixels where curtains, ghost line, and tail are in sight
    /// over the terrain depth map `terrain`, plus where the tail is hidden.
    fn occlusion_masks(
        &self,
        w: u32,
        h: u32,
        terrain: &DepthMap,
        done: &[Vec<Sample>],
        ghost: &[Vec<Sample>],
    ) -> TrackMasks {
        // Each kind of part is drawn through a mask of the pixels where it is
        // in sight (by its own depth, so a near part never lets a far one
        // through), and the tail also faintly where hidden.
        let width = f64::from(self.style.tail_width);
        let (mut curtains, mut ghosts, mut tails) = (
            NearDepth::new(w, h),
            NearDepth::new(w, h),
            NearDepth::new(w, h),
        );
        for (runs, is_ghost) in [(done, false), (ghost, true)] {
            for samples in runs {
                for pair in samples.windows(2) {
                    let (a, b) = (&pair[0], &pair[1]);
                    let corners = [
                        self.at_depth(a, self.ground(a.lat, a.lon)),
                        self.at_depth(b, self.ground(b.lat, b.lon)),
                        self.at_depth(b, self.lift(b)),
                        self.at_depth(a, self.lift(a)),
                    ];
                    if let [Some(a0), Some(b0), Some(b1), Some(a1)] = corners {
                        curtains.fill_quad([a0, b0, b1, a1]);
                        if is_ghost {
                            ghosts.fill_stroke(a1, b1, width * 0.6);
                        } else {
                            tails.fill_stroke(a1, b1, width);
                        }
                    }
                }
            }
        }
        for near in [&mut curtains, &mut ghosts, &mut tails] {
            near.grow();
        }
        let (tail, hidden_tail) = tails.masks(terrain).unzip();
        TrackMasks {
            curtain: curtains.masks(terrain).map(|m| m.0),
            ghost: ghosts.masks(terrain).map(|m| m.0),
            tail,
            hidden_tail,
        }
    }

    /// Append the curtain quads under `samples`, one path per color run.
    fn curtain_quads(
        &self,
        samples: &[Sample],
        color: &dyn Fn(&Sample, &Sample) -> Color,
        quads: &mut Vec<CurtainQuad>,
    ) {
        let camera = self.view.camera;
        for (run, color) in color_runs(samples, color) {
            let mut builder = PathBuilder::new();
            let mut depth = 0.0;
            for pair in run.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                let corners = [
                    camera.project(a.lat, a.lon, self.ground(a.lat, a.lon)),
                    camera.project(b.lat, b.lon, self.ground(b.lat, b.lon)),
                    camera.project(b.lat, b.lon, self.lift(b)),
                    camera.project(a.lat, a.lon, self.lift(a)),
                ];
                let Some(corners) = corners.into_iter().collect::<Option<Vec<_>>>() else {
                    continue;
                };
                builder.move_to(corners[0].0 as f32, corners[0].1 as f32);
                for &(x, y) in &corners[1..] {
                    builder.line_to(x as f32, y as f32);
                }
                builder.close();
                depth += (camera_depth(camera, a) + camera_depth(camera, b)) / 2.0;
            }
            if let Some(path) = builder.finish() {
                quads.push((depth / (run.len() - 1) as f64, path, color));
            }
        }
    }

    /// Fill the ghost and tail curtains back to front.
    fn draw_curtains(
        &self,
        pixmap: &mut Pixmap,
        done: &[Vec<Sample>],
        ghost: &[Vec<Sample>],
        mask: Option<&Mask>,
    ) {
        let mut quads = Vec::new();
        for samples in ghost {
            self.curtain_quads(
                samples,
                &|_, _| Color::from_rgba8(0xff, 0xff, 0xff, 0x14),
                &mut quads,
            );
        }
        for samples in done {
            self.curtain_quads(
                samples,
                &|a, b| with_alpha(self.color_of(a, b), 0.38 * self.recency(b)),
                &mut quads,
            );
        }
        quads.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, path, color) in &quads {
            let mut paint = antialiased();
            paint.set_color(*color);
            pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), mask);
        }
    }

    /// Stroke the ghost line, the faint hidden tail, and the tail line.
    fn draw_lines(
        &self,
        pixmap: &mut Pixmap,
        done: &[Vec<Sample>],
        ghost: &[Vec<Sample>],
        masks: &TrackMasks,
    ) {
        let raised = |s: &Sample| self.raised(s);
        for samples in ghost {
            stroke_polyline(
                pixmap,
                samples,
                raised,
                |_, _| Color::from_rgba8(0xff, 0xff, 0xff, 0x70),
                self.style.tail_width * 0.6,
                masks.ghost.as_ref(),
            );
        }
        // Behind the hills the tail stays as a faint line, so a followed marker
        // keeps its tail and the boundaries never leave a gap.
        if masks.hidden_tail.is_some() {
            for samples in done {
                stroke_polyline(
                    pixmap,
                    samples,
                    raised,
                    |a, b| scale_alpha(self.color_of(a, b), HIDDEN_ALPHA * self.recency(b)),
                    self.style.tail_width * 0.6,
                    masks.hidden_tail.as_ref(),
                );
            }
        }
        for samples in done {
            stroke_polyline(
                pixmap,
                samples,
                raised,
                |a, b| scale_alpha(self.color_of(a, b), self.recency(b)),
                self.style.tail_width,
                masks.tail.as_ref(),
            );
        }
    }

    /// Draw the marker at progress `u` with its drop line and ground shadow,
    /// or only faintly when it is behind a hill.
    fn draw_marker(&self, pixmap: &mut Pixmap, timeline: &Timeline, u: f64) {
        let style = self.style;
        let (lat, lon) = timeline.position_at(u);
        let end = Sample {
            lat,
            lon,
            ele: timeline.elevation_at(u),
            u,
        };
        let (Some(top), Some(foot)) = (
            self.raised(&end),
            self.view.camera.project(lat, lon, self.ground(lat, lon)),
        ) else {
            return;
        };
        // Behind a hill the marker shows faintly, without its shadow and drop
        // line, so the viewer still knows where it is.
        if self.hidden(&end) {
            fill_circle(
                pixmap,
                top,
                style.marker_radius,
                with_alpha(style.marker_color, HIDDEN_ALPHA),
            );
            return;
        }
        fill_circle(
            pixmap,
            foot,
            style.marker_radius * 0.7,
            Color::from_rgba8(0, 0, 0, 0x90),
        );
        let mut drop = PathBuilder::new();
        drop.move_to(foot.0 as f32, foot.1 as f32);
        drop.line_to(top.0 as f32, top.1 as f32);
        if let Some(path) = drop.finish() {
            let mut paint = antialiased();
            paint.set_color(with_alpha(style.marker_color, 0.7));
            let stroke = Stroke {
                width: style.marker_radius / 4.0,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
        fill_circle(pixmap, top, style.marker_radius, style.marker_color);
    }
}

/// Fill an antialiased circle of `radius` around the screen point `center`.
fn fill_circle(pixmap: &mut Pixmap, center: (f64, f64), radius: f32, color: Color) {
    if let Some(circle) = PathBuilder::from_circle(center.0 as f32, center.1 as f32, radius) {
        let mut paint = antialiased();
        paint.set_color(color);
        pixmap.fill_path(
            &circle,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Screen polylines or polygons, as `(x, y)` points.
pub(crate) type ScreenShapes = Vec<Vec<(f32, f32)>>;

/// Screen outline of a raised track and its curtain under `camera`, for
/// keeping labels off them: the top and foot polylines, and one quad per
/// segment between them. Over `terrain`, the track stands on the ground.
pub fn track_outline_3d(
    camera: &Camera,
    points: &[TrackPoint],
    relief: Relief,
    terrain: Option<&Terrain>,
) -> (ScreenShapes, ScreenShapes) {
    let mut top = Vec::new();
    let mut foot = Vec::new();
    for p in points {
        let ground = terrain.map_or(0.0, |t| t.height_at(p.lat, p.lon));
        let lift = camera.meters_to_px(relief.height_m(p.ele), p.lat);
        if let (Some(a), Some(b)) = (
            camera.project(p.lat, p.lon, lift.max(ground)),
            camera.project(p.lat, p.lon, ground),
        ) {
            top.push((a.0 as f32, a.1 as f32));
            foot.push((b.0 as f32, b.1 as f32));
        }
    }
    let areas = (1..top.len())
        .map(|i| vec![foot[i - 1], foot[i], top[i], top[i - 1]])
        .collect();
    (vec![top, foot], areas)
}

/// Screen rows `(horizon, end)` of the fog [`draw_fog`] draws on a viewport
/// `height` pixels tall: opaque at the horizon, clear by `end`. `None` when
/// the camera looks straight down or the fog ends above the viewport.
pub fn fog_band(camera: &Camera, height: u32) -> Option<(f64, f64)> {
    let horizon = camera.horizon_y();
    if !horizon.is_finite() {
        return None;
    }
    let height = f64::from(height);
    let fog_end = horizon + 0.45 * (height / 2.0 - horizon).max(0.0) + 0.08 * height;
    (fog_end > 0.0).then_some((horizon, fog_end))
}

/// Fade distant ground into `color` toward the horizon, which keeps the far
/// edge of the tile footprint from showing.
pub fn draw_fog(pixmap: &mut Pixmap, camera: &Camera, color: Color) {
    let Some((horizon, fog_end)) = fog_band(camera, pixmap.height()) else {
        return;
    };
    let Some(rect) = Rect::from_ltrb(0.0, 0.0, pixmap.width() as f32, fog_end as f32) else {
        return;
    };
    let shader = LinearGradient::new(
        Point::from_xy(0.0, horizon as f32),
        Point::from_xy(0.0, fog_end as f32),
        vec![
            GradientStop::new(0.0, with_alpha(color, 1.0)),
            GradientStop::new(1.0, with_alpha(color, 0.0)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    );
    let Some(shader) = shader else {
        return;
    };
    let paint = Paint {
        shader,
        ..Default::default()
    };
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Tail opacity is quantized to this many steps so neighboring segments share
/// a color and batch into one path.
const FADE_LEVELS: f64 = 16.0;

/// Elevation colors are quantized to this many steps so neighboring segments
/// share a color and batch into one path.
const COLOR_LEVELS: f64 = 32.0;

/// Longest run of segments drawn as one path; curtain runs are depth-sorted
/// as a unit, so shorter runs keep the back-to-front order finer.
const MAX_RUN: usize = 24;

/// Longest gap between track samples over terrain, in world pixels; finer
/// than the terrain mesh, so the line can't cut through a hill between them.
pub(super) const TERRAIN_STEP_PX: f64 = 8.0;

/// Opacity of the track and marker behind a hill.
const HIDDEN_ALPHA: f32 = 0.35;

/// Insert samples so consecutive ones are at most `step` world pixels apart
/// under `camera`, interpolating position, elevation, and progress.
pub(super) fn densify(samples: Vec<Sample>, camera: &Camera, step: f64) -> Vec<Sample> {
    let world = |s: &Sample| {
        (
            lon_to_pixel(s.lon, camera.zoom),
            lat_to_pixel(s.lat, camera.zoom),
        )
    };
    let mut out = Vec::with_capacity(samples.len());
    for pair in samples.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        out.push(*a);
        let ((ax, ay), (bx, by)) = (world(a), world(b));
        let parts = ((bx - ax).hypot(by - ay) / step).ceil();
        if parts.is_nan() || parts <= 1.0 || parts > MAX_SPLIT {
            continue;
        }
        for i in 1..parts as usize {
            let t = i as f64 / parts;
            let mix = |x: f64, y: f64| lerp(x, y, t);
            out.push(Sample {
                lat: mix(a.lat, b.lat),
                lon: mix(a.lon, b.lon),
                ele: a.ele.zip(b.ele).map(|(x, y)| mix(x, y)),
                u: mix(a.u, b.u),
            });
        }
    }
    out.extend(samples.last().copied());
    out
}

/// Most pieces one segment is split into by [`densify`].
const MAX_SPLIT: f64 = 4096.0;

/// Screen distance below which consecutive samples merge in [`draw_track_3d`].
const MIN_STEP_PX: f64 = 1.5;

/// Drop samples that land within [`MIN_STEP_PX`] of the previous kept one on
/// screen, keeping both ends, so dense recordings don't pay for curtain quads
/// and strokes too small to see.
fn thin(samples: Vec<Sample>, project: impl Fn(&Sample) -> Option<(f64, f64)>) -> Vec<Sample> {
    let last = samples.len().saturating_sub(1);
    let mut kept: Vec<Sample> = Vec::with_capacity(samples.len());
    let mut prev: Option<(f64, f64)> = None;
    for (i, sample) in samples.into_iter().enumerate() {
        let at = project(&sample);
        let close =
            matches!((prev, at), (Some(p), Some(q)) if (p.0 - q.0).hypot(p.1 - q.1) < MIN_STEP_PX);
        if close && i != last {
            continue;
        }
        prev = at;
        kept.push(sample);
    }
    kept
}

fn camera_depth(camera: &Camera, s: &Sample) -> f64 {
    camera
        .view(
            lon_to_pixel(s.lon, camera.zoom),
            lat_to_pixel(s.lat, camera.zoom),
            0.0,
        )
        .depth
}

fn stroke_polyline(
    pixmap: &mut Pixmap,
    samples: &[Sample],
    project: impl Fn(&Sample) -> Option<(f64, f64)>,
    color: impl Fn(&Sample, &Sample) -> Color,
    width: f32,
    mask: Option<&Mask>,
) {
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    for (run, color) in color_runs(samples, &color) {
        let mut builder = PathBuilder::new();
        let mut pen_down = false;
        for sample in run {
            match project(sample) {
                Some((x, y)) if pen_down => builder.line_to(x as f32, y as f32),
                Some((x, y)) => {
                    builder.move_to(x as f32, y as f32);
                    pen_down = true;
                }
                None => pen_down = false,
            }
        }
        let Some(path) = builder.finish() else {
            continue;
        };
        let mut paint = antialiased();
        paint.shader = Shader::SolidColor(color);
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask);
    }
}

/// Split `samples` into overlapping runs whose segments share one color, at
/// most [`MAX_RUN`] segments each, so each run is drawn with a single path.
///
/// Consecutive runs share their boundary sample, keeping the line unbroken.
fn color_runs<'a>(
    samples: &'a [Sample],
    color: &dyn Fn(&Sample, &Sample) -> Color,
) -> Vec<(&'a [Sample], Color)> {
    let mut runs = Vec::new();
    let mut start = 0;
    while start + 1 < samples.len() {
        let c = color(&samples[start], &samples[start + 1]);
        let mut end = start + 1;
        while end + 1 < samples.len()
            && end - start < MAX_RUN
            && color(&samples[end], &samples[end + 1]) == c
        {
            end += 1;
        }
        runs.push((&samples[start..=end], c));
        start = end;
    }
    runs
}

fn horizontal_extent_m(timeline: &Timeline) -> f64 {
    let (mut min_lat, mut max_lat, mut min_lon, mut max_lon) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for i in 0..timeline.len() {
        let (lat, lon) = timeline.vertex(i);
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
        min_lon = min_lon.min(lon);
        max_lon = max_lon.max(lon);
    }
    let mid = ((min_lat + max_lat) / 2.0).to_radians();
    let dy = (max_lat - min_lat) * 111_320.0;
    let dx = (max_lon - min_lon) * 111_320.0 * mid.cos();
    dx.hypot(dy)
}

fn antialiased() -> Paint<'static> {
    Paint {
        anti_alias: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn densify_splits_long_segments_and_keeps_the_ends() {
        let cam = Camera::new(37.55, 126.98, 15.0, 320, 240, Lens::default());
        let a = Sample {
            lat: 37.55,
            lon: 126.98,
            ele: Some(10.0),
            u: 0.0,
        };
        let b = Sample {
            lat: 37.551,
            lon: 126.982,
            ele: Some(30.0),
            u: 1.0,
        };
        let out = densify(vec![a, b], &cam, 8.0);
        assert!(out.len() > 2);
        assert_eq!((out[0], out[out.len() - 1]), (a, b));
        let world = |s: &Sample| (lon_to_pixel(s.lon, 15.0), lat_to_pixel(s.lat, 15.0));
        for pair in out.windows(2) {
            let ((ax, ay), (bx, by)) = (world(&pair[0]), world(&pair[1]));
            assert!((bx - ax).hypot(by - ay) <= 8.0 + 1e-6);
            assert!(pair[0].u < pair[1].u);
        }
        assert!(out
            .iter()
            .all(|s| s.ele.is_some_and(|e| (10.0..=30.0).contains(&e))));
    }

    use crate::geo::camera::Lens;
    use crate::geo::mercator::{pixel_to_lat, pixel_to_lon};
    use crate::gpx::model::TrackPoint;

    fn hill() -> Vec<TrackPoint> {
        [
            (37.550, 126.980, 30.0),
            (37.552, 126.985, 240.0),
            (37.555, 126.982, 60.0),
        ]
        .iter()
        .map(|&(lat, lon, ele)| {
            let mut p = TrackPoint::new(lat, lon);
            p.ele = Some(ele);
            p
        })
        .collect()
    }

    #[test]
    fn auto_relief_starts_at_lowest_point() {
        let tl = Timeline::build(&hill()).unwrap();
        let relief = Relief::auto(&tl);
        assert_eq!(relief.base_m, 30.0);
        assert!((1.0..=6.0).contains(&relief.exaggeration));
        assert_eq!(relief.height_m(Some(30.0)), 0.0);
        assert_eq!(relief.height_m(None), 0.0);
    }

    #[test]
    fn draws_track_and_marker() {
        let pts = hill();
        let tl = Timeline::build(&pts).unwrap();
        let relief = Relief::auto(&tl);
        let heights = relief.heights_m(&tl);
        let cam = Camera::fit(&pts, &heights, 400, 300, 20.0, 0.0, Lens::default()).unwrap();
        let mut pixmap = Pixmap::new(400, 300).unwrap();
        pixmap.fill(Color::BLACK);
        let blank = pixmap.clone();
        draw_track_3d(
            &mut pixmap,
            &tl,
            &SceneView {
                camera: &cam,
                relief,
                terrain: None,
                occlusion: None,
            },
            0.5,
            1.0,
            &FrameStyle::default(),
        );
        assert_ne!(pixmap.data(), blank.data());
    }

    #[test]
    fn short_tail_leaves_older_progress_as_ghost() {
        let pts = hill();
        let tl = Timeline::build(&pts).unwrap();
        let relief = Relief::auto(&tl);
        let heights = relief.heights_m(&tl);
        let cam = Camera::fit(&pts, &heights, 400, 300, 20.0, 0.0, Lens::default()).unwrap();
        let style = FrameStyle {
            tail_color: Color::from_rgba8(0xff, 0, 0, 0xff),
            ..FrameStyle::default()
        };
        let red_pixels = |tail_u: f64| {
            let mut pixmap = Pixmap::new(400, 300).unwrap();
            pixmap.fill(Color::BLACK);
            draw_track_3d(
                &mut pixmap,
                &tl,
                &SceneView {
                    camera: &cam,
                    relief,
                    terrain: None,
                    occlusion: None,
                },
                0.95,
                tail_u,
                &style,
            );
            pixmap
                .pixels()
                .iter()
                .filter(|p| p.red() > 0x80 && p.green() < 0x40)
                .count()
        };
        let (full, tail) = (red_pixels(1.0), red_pixels(0.2));
        assert!(tail * 2 < full, "tail {tail} vs full {full}");
        assert!(tail > 0);
    }

    #[test]
    fn marker_survives_a_zero_tail_at_the_end() {
        let pts = hill();
        let tl = Timeline::build(&pts).unwrap();
        let relief = Relief::auto(&tl);
        let heights = relief.heights_m(&tl);
        let cam = Camera::fit(&pts, &heights, 400, 300, 20.0, 0.0, Lens::default()).unwrap();
        let mut pixmap = Pixmap::new(400, 300).unwrap();
        pixmap.fill(Color::BLACK);
        draw_track_3d(
            &mut pixmap,
            &tl,
            &SceneView {
                camera: &cam,
                relief,
                terrain: None,
                occlusion: None,
            },
            1.0,
            0.0,
            &FrameStyle::default(),
        );
        let end = &pts[pts.len() - 1];
        let h = cam.meters_to_px(relief.height_m(end.ele), end.lat);
        let (x, y) = cam.project(end.lat, end.lon, h).unwrap();
        let px = pixmap.pixel(x.round() as u32, y.round() as u32).unwrap();
        assert!(px.red() > 0xc0, "marker missing at ({x}, {y})");
    }

    /// The hill scene's terrain drawn, and the whole track over it along a
    /// west–east line `dy` world pixels south of the summit, `ele` meters up,
    /// with or without hiding; and how many of its samples the terrain's
    /// depth map hides.
    fn hill_track(dy: f64, ele: f64, hide: bool) -> (Pixmap, Pixmap, usize) {
        use crate::terrain::draw_terrain;
        use crate::test_support::hill::{hill_camera, hill_origin, terrain};
        let terrain = terrain();
        let mut cam = hill_camera(75.0);
        cam.distance *= 0.4;
        let (ox, oy) = hill_origin();
        let coordinate = |x: f64, y: f64| (pixel_to_lat(y, cam.zoom), pixel_to_lon(x, cam.zoom));
        let points: Vec<TrackPoint> = (0..=8)
            .map(|i| {
                let (lat, lon) = coordinate(ox + 176.0 + f64::from(i) * 20.0, oy + 256.0 + dy);
                let mut p = TrackPoint::new(lat, lon);
                p.ele = Some(ele);
                p
            })
            .collect();
        let tl = Timeline::build(&points).unwrap();
        let relief = Relief {
            base_m: 0.0,
            exaggeration: 1.0,
        };
        let mut ground = Pixmap::new(320, 240).unwrap();
        ground.fill(Color::BLACK);
        let depth = draw_terrain(&mut ground, &cam, &terrain);
        let hidden_samples = points
            .iter()
            .filter(|p| {
                let h = cam
                    .meters_to_px(ele, p.lat)
                    .max(terrain.height_at(p.lat, p.lon));
                let v = cam.view(
                    lon_to_pixel(p.lon, cam.zoom),
                    lat_to_pixel(p.lat, cam.zoom),
                    h,
                );
                let (sx, sy) = cam.to_screen(v);
                depth.hides(sx, sy, v.depth)
            })
            .count();
        let mut pixmap = ground.clone();
        let style = FrameStyle {
            color_by_elevation: false,
            tail_color: Color::from_rgba8(0xff, 0, 0, 0xff),
            marker_radius: 0.0,
            ..FrameStyle::default()
        };
        draw_track_3d(
            &mut pixmap,
            &tl,
            &SceneView {
                camera: &cam,
                relief,
                terrain: Some(&terrain),
                occlusion: hide.then_some(&depth),
            },
            1.0,
            1.0,
            &style,
        );
        (ground, pixmap, hidden_samples)
    }

    /// Largest red rise and number of pixels the track changed.
    fn track_pixels(ground: &Pixmap, drawn: &Pixmap) -> (u8, usize) {
        ground
            .pixels()
            .iter()
            .zip(drawn.pixels())
            .filter(|(a, b)| a != b)
            .fold((0, 0), |(red, n), (a, b)| {
                (red.max(b.red().saturating_sub(a.red())), n + 1)
            })
    }

    #[test]
    fn behind_a_hill_only_a_faint_line_shows_and_no_curtain() {
        // Behind the summit (every sample out of sight), raised above its
        // ground, so with a curtain.
        let (ground, open, behind) = hill_track(-120.0, 250.0, false);
        let (_, hidden, _) = hill_track(-120.0, 250.0, true);
        assert_eq!(behind, 9, "every sample is behind the hill");
        let (open_red, open_n) = track_pixels(&ground, &open);
        let (hidden_red, hidden_n) = track_pixels(&ground, &hidden);
        assert!(open_red > 0xc0, "drawn plainly without hiding: {open_red}");
        assert!(
            hidden_red < 0x70,
            "only faint behind the hill: {hidden_red}"
        );
        assert!(hidden_n < open_n / 2, "no curtain: {hidden_n} vs {open_n}");
        // In front of the hill nothing is hidden.
        let (_, front, _) = hill_track(150.0, 300.0, false);
        let (_, front_hidden, _) = hill_track(150.0, 300.0, true);
        assert_eq!(front.data(), front_hidden.data());
    }

    #[test]
    fn a_curtain_hangs_behind_the_hill_below_a_track_in_sight() {
        // High enough above the far slope that every sample is in sight (so
        // hiding whole segments would change nothing), while the curtain's
        // foot on the far slope is behind the summit.
        let (ground, open, behind) = hill_track(-120.0, 650.0, false);
        let (_, hidden, _) = hill_track(-120.0, 650.0, true);
        assert_eq!(behind, 0, "every sample is in sight");
        let (open_red, open_n) = track_pixels(&ground, &open);
        let (hidden_red, hidden_n) = track_pixels(&ground, &hidden);
        assert!(
            open_red > 0xc0 && hidden_red > 0xc0,
            "the line stays bright"
        );
        assert!(
            hidden_n * 10 < open_n * 9,
            "the curtain's foot is hidden: {hidden_n} vs {open_n}"
        );
    }

    #[test]
    fn fog_darkens_toward_the_horizon() {
        let cam = Camera::new(
            37.5,
            127.0,
            15.0,
            400,
            300,
            Lens {
                pitch_deg: 70.0,
                ..Lens::default()
            },
        );
        let mut pixmap = Pixmap::new(400, 300).unwrap();
        pixmap.fill(Color::WHITE);
        draw_fog(&mut pixmap, &cam, Color::BLACK);
        let top = pixmap.pixel(200, 1).unwrap().red();
        let bottom = pixmap.pixel(200, 298).unwrap().red();
        assert!(
            top < bottom,
            "top {top} should be darker than bottom {bottom}"
        );
    }

    fn sample(lat: f64, lon: f64, ele: Option<f64>, u: f64) -> Sample {
        Sample { lat, lon, ele, u }
    }

    #[test]
    fn relief_without_climb_is_unscaled() {
        let flat = Timeline::build(&[
            TrackPoint::new(37.55, 126.98),
            TrackPoint::new(37.56, 126.99),
        ])
        .unwrap();
        assert_eq!(
            Relief::auto(&flat),
            Relief {
                base_m: 0.0,
                exaggeration: 1.0
            }
        );
        let level: Vec<TrackPoint> = hill()
            .into_iter()
            .map(|mut p| {
                p.ele = Some(50.0);
                p
            })
            .collect();
        let relief = Relief::auto(&Timeline::build(&level).unwrap());
        assert_eq!((relief.base_m, relief.exaggeration), (50.0, 1.0));
    }

    #[test]
    fn relief_heights_scale_above_the_base_and_never_sink() {
        let relief = Relief {
            base_m: 100.0,
            exaggeration: 4.0,
        };
        assert_eq!(relief.height_m(Some(150.0)), 200.0);
        assert_eq!(relief.height_m(Some(20.0)), 0.0);
        assert_eq!(relief.over_terrain().exaggeration, TERRAIN_MAX_EXAGGERATION);
        let gentle = Relief {
            exaggeration: 1.5,
            ..relief
        };
        assert_eq!(gentle.over_terrain(), gentle);
        let tl = Timeline::build(&hill()).unwrap();
        let heights = Relief::auto(&tl).heights_m(&tl);
        assert_eq!(heights.len(), tl.len());
        assert!(heights.iter().all(|&h| h >= 0.0));
        assert!(heights[1] > heights[0] && heights[1] > heights[2]);
    }

    #[test]
    fn densify_leaves_short_segments_and_drops_unknown_elevation() {
        let cam = Camera::new(37.55, 126.98, 15.0, 320, 240, Lens::default());
        let a = sample(37.55, 126.98, Some(10.0), 0.0);
        let near = sample(37.550_01, 126.98, None, 0.5);
        assert_eq!(densify(vec![a, near], &cam, 8.0), [a, near]);
        assert_eq!(densify(vec![a], &cam, 8.0), [a]);
        assert!(densify(Vec::new(), &cam, 8.0).is_empty());
        let far = sample(37.551, 126.982, None, 1.0);
        let out = densify(vec![a, far], &cam, 8.0);
        assert!(out.len() > 2);
        assert!(out[1..].iter().all(|s| s.ele.is_none()));
    }

    #[test]
    fn densify_does_not_split_a_segment_into_too_many_pieces() {
        let cam = Camera::new(37.55, 126.98, 15.0, 320, 240, Lens::default());
        let (a, b) = (
            sample(37.55, 126.98, None, 0.0),
            sample(-37.55, -50.0, None, 1.0),
        );
        assert_eq!(densify(vec![a, b], &cam, 8.0), [a, b]);
    }

    #[test]
    fn thinning_drops_close_samples_but_keeps_both_ends() {
        let samples: Vec<Sample> = (0..6)
            .map(|i| sample(0.0, 0.0, None, f64::from(i)))
            .collect();
        // Screen x is u; u = 3 can't be projected.
        let kept = thin(samples, |s| {
            (s.u != 3.0).then_some((s.u * if s.u < 2.5 { 0.5 } else { 2.0 }, 0.0))
        });
        let us: Vec<f64> = kept.iter().map(|s| s.u).collect();
        // 1 lands 0.5 px from 0 (dropped); 2 is 1.0 px away (dropped); 3 has
        // no screen position and resets the spacing; 5 is 2 px past 4.
        assert_eq!(us, [0.0, 3.0, 4.0, 5.0]);
        let close: Vec<Sample> = (0..3)
            .map(|i| sample(0.0, 0.0, None, f64::from(i)))
            .collect();
        let ends: Vec<f64> = thin(close, |_| Some((0.0, 0.0)))
            .iter()
            .map(|s| s.u)
            .collect();
        assert_eq!(ends, [0.0, 2.0]);
    }

    #[test]
    fn color_runs_break_at_color_changes_and_length_and_share_their_ends() {
        let samples: Vec<Sample> = (0..60)
            .map(|i| sample(0.0, 0.0, None, f64::from(i)))
            .collect();
        let color = |a: &Sample, _: &Sample| {
            if a.u < 10.0 {
                Color::BLACK
            } else {
                Color::WHITE
            }
        };
        let runs = color_runs(&samples, &color);
        let spans: Vec<(f64, f64)> = runs
            .iter()
            .map(|(run, _)| (run[0].u, run[run.len() - 1].u))
            .collect();
        assert_eq!(
            spans,
            [(0.0, 10.0), (10.0, 34.0), (34.0, 58.0), (58.0, 59.0)]
        );
        assert_eq!(runs[0].1, Color::BLACK);
        assert!(runs[1..].iter().all(|(_, c)| *c == Color::WHITE));
        assert!(color_runs(&samples[..1], &color).is_empty());
    }

    #[test]
    fn no_fog_looking_straight_down() {
        let lens = |pitch_deg| Lens {
            pitch_deg,
            ..Lens::default()
        };
        let down = Camera::new(37.5, 127.0, 15.0, 400, 300, lens(0.0));
        assert_eq!(fog_band(&down, 300), None);
        let tilted = Camera::new(37.5, 127.0, 15.0, 400, 300, lens(70.0));
        let (horizon, end) = fog_band(&tilted, 300).unwrap();
        assert!(horizon < end && end > 0.0);
        let mut pixmap = Pixmap::new(400, 300).unwrap();
        pixmap.fill(Color::WHITE);
        draw_fog(&mut pixmap, &down, Color::BLACK);
        assert!(pixmap.pixels().iter().all(|p| p.red() == 255));
    }

    #[test]
    fn track_outline_pairs_each_raised_point_with_its_foot() {
        let points = hill();
        let tl = Timeline::build(&points).unwrap();
        let relief = Relief::auto(&tl);
        let heights = relief.heights_m(&tl);
        let cam = Camera::fit(&points, &heights, 400, 300, 20.0, 0.0, Lens::default()).unwrap();
        let (lines, areas) = track_outline_3d(&cam, &points, relief, None);
        let (top, foot) = (&lines[0], &lines[1]);
        assert_eq!((top.len(), foot.len(), areas.len()), (3, 3, 2));
        // The lowest point stands on the ground; the summit rises above it.
        assert_eq!(top[0], foot[0]);
        assert!(top[1].1 < foot[1].1);
        assert_eq!(areas[0], vec![foot[0], foot[1], top[1], top[0]]);
    }
}
