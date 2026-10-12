use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint, PathBuilder,
    Pixmap, Point, Rect, Shader, SpreadMode, Stroke, Transform,
};

use super::frame::FrameStyle;
use super::occlusion::{DepthPoint, NearDepth};
use super::ramp::elevation_color;
use crate::anim::{Sample, Timeline};
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
    let at_depth = |s: &Sample, h: f64| -> Option<DepthPoint> {
        let point = camera.view(
            lon_to_pixel(s.lon, camera.zoom),
            lat_to_pixel(s.lat, camera.zoom),
            h,
        );
        (point.depth >= NEAR).then(|| {
            let (x, y) = camera.to_screen(point);
            (x, y, point.depth)
        })
    };
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
    // instead of cutting through hills.
    let prepare = |samples: Vec<Sample>| -> Vec<Sample> {
        let samples = match view.terrain {
            Some(_) => densify(samples, camera, TERRAIN_STEP_PX),
            None => samples,
        };
        thin(samples, raised)
    };
    let u0 = if tail_u >= 1.0 {
        0.0
    } else {
        (u - tail_u).max(0.0)
    };
    let done = vec![prepare(timeline.window(u0, u))];
    let mut ghost = vec![prepare(timeline.window(u, 1.0))];
    if tail_u < 1.0 {
        ghost.push(prepare(timeline.window(0.0, u)));
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

    // Over a terrain depth map each kind of part is drawn through a mask of
    // the pixels where it is in sight (by its own depth, so a near part
    // never lets a far one through), and the tail also faintly where hidden.
    let width = style.tail_width as f64;
    let masks = view.occlusion.map(|terrain| {
        let (w, h) = (pixmap.width(), pixmap.height());
        let (mut curtains, mut ghosts, mut tails) = (
            NearDepth::new(w, h),
            NearDepth::new(w, h),
            NearDepth::new(w, h),
        );
        for (runs, is_ghost) in [(&done, false), (&ghost, true)] {
            for samples in runs {
                for pair in samples.windows(2) {
                    let (a, b) = (&pair[0], &pair[1]);
                    let corners = [
                        at_depth(a, ground(a.lat, a.lon)),
                        at_depth(b, ground(b.lat, b.lon)),
                        at_depth(b, lift(b)),
                        at_depth(a, lift(a)),
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
        let tails = tails.masks(terrain);
        (
            curtains.masks(terrain).map(|m| m.0),
            ghosts.masks(terrain).map(|m| m.0),
            tails,
        )
    });
    let (curtain_mask, ghost_mask, tail_masks) = match &masks {
        Some((c, g, t)) => (c.as_ref(), g.as_ref(), t.as_ref()),
        None => (None, None, None),
    };
    let tail_mask = tail_masks.map(|m| &m.0);
    let hidden_tail_mask = tail_masks.map(|m| &m.1);

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
        pixmap.fill_path(
            path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            curtain_mask,
        );
    }

    for samples in &ghost {
        stroke_polyline(
            pixmap,
            samples,
            raised,
            |_, _| Color::from_rgba8(0xff, 0xff, 0xff, 0x70),
            style.tail_width * 0.6,
            ghost_mask,
        );
    }
    // Behind the hills the tail stays as a faint line, so a followed marker
    // keeps its tail and the boundaries never leave a gap.
    if hidden_tail_mask.is_some() {
        for samples in &done {
            stroke_polyline(
                pixmap,
                samples,
                raised,
                |a, b| scale_alpha(color_of(a, b), HIDDEN_ALPHA * recency(b)),
                style.tail_width * 0.6,
                hidden_tail_mask,
            );
        }
    }
    for samples in &done {
        stroke_polyline(
            pixmap,
            samples,
            raised,
            |a, b| scale_alpha(color_of(a, b), recency(b)),
            style.tail_width,
            tail_mask,
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
        use crate::terrain::mesh::tests::{hill_camera, hill_origin, terrain};
        let terrain = terrain();
        let mut cam = hill_camera(75.0);
        cam.distance *= 0.4;
        let (ox, oy) = hill_origin();
        let coordinate = |x: f64, y: f64| (pixel_to_lat(y, cam.zoom), pixel_to_lon(x, cam.zoom));
        let points: Vec<TrackPoint> = (0..=8)
            .map(|i| {
                let (lat, lon) = coordinate(ox + 176.0 + i as f64 * 20.0, oy + 256.0 + dy);
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
}
