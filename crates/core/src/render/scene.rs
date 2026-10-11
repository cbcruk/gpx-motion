use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, PathBuilder, Pixmap,
    Point, Rect, Shader, SpreadMode, Stroke, Transform,
};

use super::frame::{scale_alpha, FrameStyle};
use super::ramp::elevation_color;
use crate::anim::{Sample, Timeline};
use crate::geo::camera::Camera;
use crate::geo::camera::NEAR;
use crate::geo::mercator::{lat_to_pixel, lon_to_pixel};
use crate::gpx::TrackPoint;
use crate::terrain::{DepthMap, Terrain};

/// Raises track elevations into the 3D scene.
///
/// Heights are measured from the track's lowest point (which sits on the map
/// plane) and multiplied by `exaggeration`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Relief {
    pub base_m: f64,
    pub exaggeration: f64,
}

/// Most automatic exaggeration over terrain: the automatic factor is tuned
/// to the track's climb and would spike the hills around a flat route.
pub const TERRAIN_MAX_EXAGGERATION: f64 = 2.0;

impl Relief {
    /// The same relief with the exaggeration capped at
    /// [`TERRAIN_MAX_EXAGGERATION`], for drawing over terrain.
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
    pub camera: &'a Camera,
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
/// drawn last so they stay legible.
pub fn draw_track_3d(
    pixmap: &mut Pixmap,
    timeline: &Timeline,
    view: &SceneView,
    u: f64,
    tail_u: f64,
    style: &FrameStyle,
) {
    let (camera, relief) = (view.camera, &view.relief);
    let elevation = timeline.elevation().filter(|_| style.color_by_elevation);
    let color_of = |a: &Sample, b: &Sample| match (elevation, a.ele, b.ele) {
        (Some(e), Some(ea), Some(eb)) => {
            let t = e.normalize((ea + eb) / 2.0);
            elevation_color((t * COLOR_LEVELS).round() / COLOR_LEVELS)
        }
        _ => style.tail_color,
    };

    let ground = |lat: f64, lon: f64| view.terrain.map_or(0.0, |t| t.height_at(lat, lon));
    let lift = |s: &Sample| {
        camera
            .meters_to_px(relief.height_m(s.ele), s.lat)
            .max(ground(s.lat, s.lon))
    };
    let raised = |s: &Sample| camera.project(s.lat, s.lon, lift(s));
    let hidden = |s: &Sample| {
        view.occlusion.is_some_and(|depth| {
            let (x, y) = (
                lon_to_pixel(s.lon, camera.zoom),
                lat_to_pixel(s.lat, camera.zoom),
            );
            let point = camera.view(x, y, lift(s));
            if point.depth < NEAR {
                return false;
            }
            let (sx, sy) = camera.to_screen(point);
            depth.hides(sx, sy, point.depth)
        })
    };
    // Over terrain, long segments are split so the line follows the ground
    // instead of cutting through hills, and parts behind hills are set apart.
    let prepare = |samples: Vec<Sample>| -> Runs {
        let samples = match view.terrain {
            Some(_) => densify(samples, camera, TERRAIN_STEP_PX),
            None => samples,
        };
        split_runs(thin(samples, raised), hidden)
    };
    let u0 = if tail_u >= 1.0 {
        0.0
    } else {
        (u - tail_u).max(0.0)
    };
    let (done, done_hidden) = prepare(timeline.window(u0, u));
    let mut ghost = prepare(timeline.window(u, 1.0)).0;
    if tail_u < 1.0 {
        ghost.extend(prepare(timeline.window(0.0, u)).0);
    }
    let span = (u - u0).max(f64::EPSILON);
    let recency = |b: &Sample| {
        if tail_u >= 1.0 {
            1.0
        } else {
            let r = ((b.u - u0) / span).clamp(0.0, 1.0);
            ((r * FADE_LEVELS).ceil() / FADE_LEVELS) as f32
        }
    };

    let mut quads: Vec<(f64, tiny_skia::Path, Color)> = Vec::new();
    let mut push_quads = |samples: &[Sample], color: &dyn Fn(&Sample, &Sample) -> Color| {
        for (run, color) in color_runs(samples, color) {
            let mut builder = PathBuilder::new();
            let mut depth = 0.0;
            for pair in run.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                let corners = [
                    camera.project(a.lat, a.lon, ground(a.lat, a.lon)),
                    camera.project(b.lat, b.lon, ground(b.lat, b.lon)),
                    camera.project(b.lat, b.lon, lift(b)),
                    camera.project(a.lat, a.lon, lift(a)),
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
    };
    for samples in &ghost {
        push_quads(samples, &|_, _| Color::from_rgba8(0xff, 0xff, 0xff, 0x14));
    }
    for samples in &done {
        push_quads(samples, &|a, b| {
            with_alpha(color_of(a, b), 0.38 * recency(b))
        });
    }
    quads.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, path, color) in &quads {
        let mut paint = antialiased();
        paint.set_color(*color);
        pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
    }

    for samples in &ghost {
        stroke_polyline(
            pixmap,
            samples,
            raised,
            |_, _| Color::from_rgba8(0xff, 0xff, 0xff, 0x70),
            style.tail_width * 0.6,
        );
    }
    // Behind the hills the track stays as a faint line, so a followed
    // marker keeps its tail and the boundaries never leave a gap.
    for samples in &done_hidden {
        stroke_polyline(
            pixmap,
            samples,
            raised,
            |a, b| scale_alpha(color_of(a, b), HIDDEN_ALPHA * recency(b)),
            style.tail_width * 0.6,
        );
    }
    for samples in &done {
        stroke_polyline(
            pixmap,
            samples,
            raised,
            |a, b| scale_alpha(color_of(a, b), recency(b)),
            style.tail_width,
        );
    }

    let (lat, lon) = timeline.position_at(u);
    let end = Sample {
        lat,
        lon,
        ele: timeline.elevation_at(u),
        u,
    };
    let (Some(top), Some(foot)) = (raised(&end), camera.project(lat, lon, ground(lat, lon))) else {
        return;
    };
    // Behind a hill the marker shows faintly, without its shadow and drop
    // line, so the viewer still knows where it is.
    if hidden(&end) {
        if let Some(marker) =
            PathBuilder::from_circle(top.0 as f32, top.1 as f32, style.marker_radius)
        {
            let mut paint = antialiased();
            paint.set_color(with_alpha(style.marker_color, HIDDEN_ALPHA));
            pixmap.fill_path(
                &marker,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        return;
    }
    if let Some(shadow) =
        PathBuilder::from_circle(foot.0 as f32, foot.1 as f32, style.marker_radius * 0.7)
    {
        let mut paint = antialiased();
        paint.set_color(Color::from_rgba8(0, 0, 0, 0x90));
        pixmap.fill_path(
            &shadow,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
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
    if let Some(marker) = PathBuilder::from_circle(top.0 as f32, top.1 as f32, style.marker_radius)
    {
        let mut paint = antialiased();
        paint.set_color(style.marker_color);
        pixmap.fill_path(
            &marker,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Screen polylines or polygons, as `(x, y)` points.
pub type ScreenShapes = Vec<Vec<(f32, f32)>>;

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
    let height = height as f64;
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
            let mix = |x: f64, y: f64| x + (y - x) * t;
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

/// Runs of samples whose segments are visible, and runs whose segments are
/// hidden, as split by [`split_runs`].
type Runs = (Vec<Vec<Sample>>, Vec<Vec<Sample>>);

/// Split samples into runs of visible segments (both ends visible) and runs
/// of hidden ones (either end `hidden`); neighbouring runs share their
/// boundary sample, so together they draw the whole line.
fn split_runs(samples: Vec<Sample>, hidden: impl Fn(&Sample) -> bool) -> Runs {
    let flags: Vec<bool> = samples.iter().map(&hidden).collect();
    let (mut visible, mut behind): Runs = (Vec::new(), Vec::new());
    let mut start = 0;
    while start + 1 < samples.len() {
        let covered = |i: usize| flags[i] || flags[i + 1];
        let kind = covered(start);
        let mut end = start + 1;
        while end + 1 < samples.len() && covered(end) == kind {
            end += 1;
        }
        let run = samples[start..=end].to_vec();
        if kind {
            behind.push(run);
        } else {
            visible.push(run);
        }
        start = end;
    }
    (visible, behind)
}

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
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
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

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color::from_rgba(color.red(), color.green(), color.blue(), alpha).unwrap_or(color)
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

    #[test]
    fn runs_split_at_hidden_samples_and_share_their_ends() {
        let s = |u: f64| Sample {
            lat: 0.0,
            lon: 0.0,
            ele: None,
            u,
        };
        let samples: Vec<Sample> = (0..8).map(|i| s(i as f64)).collect();
        let (visible, hidden) = split_runs(samples.clone(), |x| [2.0, 5.0].contains(&x.u));
        let us = |runs: &[Vec<Sample>]| -> Vec<Vec<f64>> {
            runs.iter()
                .map(|r| r.iter().map(|x| x.u).collect())
                .collect()
        };
        assert_eq!(
            us(&visible),
            vec![vec![0.0, 1.0], vec![3.0, 4.0], vec![6.0, 7.0]]
        );
        assert_eq!(us(&hidden), vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]);
        let (all, none) = split_runs(samples, |_| false);
        assert_eq!((all.len(), all[0].len(), none.len()), (1, 8, 0));
    }

    use crate::geo::camera::Lens;
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
}
