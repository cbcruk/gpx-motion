//! Tilted 3D view: camera framing, per-frame cameras, and frame rendering.

use std::sync::Mutex;

use gpx_motion_core::mercator::{lat_to_pixel, lon_to_pixel};
use gpx_motion_core::{
    draw_buildings, draw_fog, draw_labels, draw_terrain, draw_track_3d, rise_ahead, steer,
    track_outline_3d, Camera, DepthMap, Facet, FrameStyle, LabelMemory, LabelStyle, Lens,
    PlacedLabel, Relief, SceneView, Terrain, TrackPoint, MAX_PITCH_DEG, TERRAIN_CLEARANCE,
};
use tiny_skia::{Color, Pixmap, Rect};

use crate::labels;
use crate::tiles::{LabelCache, LabelContext, MapSource};
use crate::track::Track;

/// Camera settings for the 3D view.
#[derive(Debug, Clone, Copy)]
pub struct Perspective {
    pub lens: Lens,
    /// Degrees the bearing turns over the whole animation.
    pub orbit_deg: f64,
    /// Fixed zoom that follows the lead marker instead of framing the route.
    pub follow_zoom: Option<f64>,
    /// Turn the camera with the lead marker's direction of travel instead of
    /// orbiting; `lens.bearing_deg` becomes an offset from the heading.
    pub heading: Option<HeadingFollow>,
    /// Frames over which a follow camera eases into (and out of) looking
    /// down more steeply to keep the marker in sight over hills.
    pub sight_frames: usize,
}

/// Steps in which a follow camera looks down more steeply until it sees the
/// marker past the hills, in degrees.
const SIGHT_STEP_DEG: f64 = 2.5;
/// Steepest a follow camera looks down for that, in degrees from straight down.
const SIGHT_MIN_PITCH_DEG: f64 = 10.0;

/// How the camera tracks the direction of travel.
#[derive(Debug, Clone, Copy)]
pub struct HeadingFollow {
    /// Meters of track the heading is averaged over.
    pub window_m: f64,
    /// Largest bearing change between consecutive frames, in degrees.
    pub max_turn_deg: f64,
}

/// Terrain the cameras must stay above and aim along, with the relief that
/// raises the track over it.
#[derive(Clone, Copy)]
pub struct Ground<'a> {
    pub terrain: &'a Terrain,
    pub relief: Relief,
}

/// The frames a camera sequence runs through: the framing it starts from,
/// the lead track it follows, and each frame's progress and bearing (from
/// [`Perspective::bearings`]), in frame order.
#[derive(Clone, Copy)]
pub struct CameraPath<'a> {
    /// The starting camera and what it keeps in view.
    pub framing: &'a Framing,
    /// The track follow-mode cameras aim at.
    pub lead: &'a Track,
    /// Each frame's route progress, from `0` to `1`.
    pub us: &'a [f64],
    /// Each frame's camera bearing in degrees.
    pub bearings: &'a [f64],
}

/// The starting camera plus what it keeps in view, for re-aiming per frame.
pub struct Framing {
    base: Camera,
    points: Vec<TrackPoint>,
    heights: Vec<f64>,
    padding: f64,
    bottom_inset: f64,
}

impl Perspective {
    /// Camera bearing for each progress value in `us`: orbiting from the base
    /// bearing, or turning with the lead marker's heading (rate-limited across
    /// the sequence, so pass every frame in order).
    pub fn bearings(&self, lead: &Track, us: &[f64]) -> Vec<f64> {
        match self.heading {
            Some(follow) => {
                let headings: Vec<Option<f64>> = us
                    .iter()
                    .map(|&u| lead.timeline.heading_at(u, follow.window_m))
                    .collect();
                steer(&headings, follow.max_turn_deg)
                    .into_iter()
                    .map(|b| b + self.lens.bearing_deg)
                    .collect()
            }
            None => us
                .iter()
                .map(|&u| self.lens.bearing_deg + self.orbit_deg * u)
                .collect(),
        }
    }

    /// One camera per frame of `path`. Over `ground`, follow-mode cameras
    /// aim at the raised marker, look down more steeply where hills would
    /// hide it (easing in and out over [`Perspective::sight_frames`], so
    /// the path must hold every frame in order), and every camera stays
    /// above the terrain.
    pub fn cameras(&self, path: CameraPath, ground: Option<Ground>) -> Vec<Camera> {
        let CameraPath {
            framing,
            lead,
            us,
            bearings,
        } = path;
        let pitch = self.lens.pitch_deg.clamp(0.0, MAX_PITCH_DEG);
        let tilts = match (self.follow_zoom, ground) {
            (Some(_), Some(g)) => {
                let needs: Vec<f64> = us
                    .iter()
                    .zip(bearings)
                    .map(|(&u, &bearing)| self.sight_tilt(framing, lead, u, bearing, g))
                    .collect();
                rise_ahead(&needs, self.sight_frames)
            }
            _ => vec![0.0; us.len()],
        };
        us.iter()
            .zip(bearings)
            .zip(tilts)
            .map(|((&u, &bearing), tilt)| {
                self.grounded(framing, lead, u, bearing, pitch - tilt, ground)
            })
            .collect()
    }

    /// How many degrees more steeply than its pitch the follow camera at
    /// progress `u` must look down to see the marker past the hills (in
    /// [`SIGHT_STEP_DEG`] steps, at most down to [`SIGHT_MIN_PITCH_DEG`]).
    fn sight_tilt(
        &self,
        framing: &Framing,
        lead: &Track,
        u: f64,
        bearing_deg: f64,
        ground: Ground,
    ) -> f64 {
        let pitch = self.lens.pitch_deg.clamp(0.0, MAX_PITCH_DEG);
        let floor = SIGHT_MIN_PITCH_DEG.min(pitch);
        let mut tilt = 0.0;
        while pitch - tilt > floor {
            let camera = self.grounded(framing, lead, u, bearing_deg, pitch - tilt, Some(ground));
            let (x, y, h) = marker_point(lead, u, &camera, ground);
            if !ground.terrain.hides_point(&camera, x, y, h) {
                return tilt;
            }
            tilt += SIGHT_STEP_DEG;
        }
        pitch - floor
    }

    /// [`Perspective::camera_at`] at `pitch_deg`, kept above the terrain.
    fn grounded(
        &self,
        framing: &Framing,
        lead: &Track,
        u: f64,
        bearing_deg: f64,
        pitch_deg: f64,
        ground: Option<Ground>,
    ) -> Camera {
        let camera = self.camera_at(framing, lead, u, bearing_deg, pitch_deg, ground);
        match ground {
            Some(g) => above(camera, g.terrain),
            None => camera,
        }
    }

    /// Cameras that zoom from this (route-framing) perspective's view along
    /// `path` into `close`, one per frame: each `mixes` entry runs from `0`
    /// (the whole route) to `1` (the matching `close` camera). Over `ground`
    /// they stay above the terrain, in `close`'s world zoom like the terrain
    /// itself.
    pub fn zoom_into(
        &self,
        path: CameraPath,
        close: &[Camera],
        mixes: &[f64],
        ground: Option<Ground>,
    ) -> Vec<Camera> {
        self.cameras(path, None)
            .iter()
            .zip(close)
            .zip(mixes)
            .map(|((wide, close), &mix)| {
                let camera = wide.zoom_between(close, mix);
                match ground {
                    Some(g) => above(camera, g.terrain),
                    None => camera,
                }
            })
            .collect()
    }

    /// Camera at progress `u` facing `bearing_deg` at `pitch_deg`: re-aimed
    /// so the route stays centered, or in follow mode aimed at the lead
    /// marker.
    fn camera_at(
        &self,
        framing: &Framing,
        lead: &Track,
        u: f64,
        bearing_deg: f64,
        pitch_deg: f64,
        ground: Option<Ground>,
    ) -> Camera {
        let camera = framing.base.with_bearing(bearing_deg).with_pitch(pitch_deg);
        if self.follow_zoom.is_none() {
            return camera.aim(
                &framing.points,
                &framing.heights,
                framing.padding,
                framing.bottom_inset,
            );
        }
        let (lat, lon) = lead.timeline.position_at(u);
        match ground {
            Some(g) => {
                let (_, _, h) = marker_point(lead, u, &camera, g);
                camera.with_raised_target(lat, lon, h)
            }
            None => camera.with_target(lat, lon),
        }
    }

    /// The starting framing: around every track, or at the follow zoom on the
    /// start point. Call [`Framing::cover`] before building cameras.
    pub fn framing(
        &self,
        tracks: &[Track],
        relief: &Relief,
        width: u32,
        height: u32,
        padding: f64,
        bottom_inset: f64,
    ) -> Option<Framing> {
        let lead = tracks.first()?;
        if let Some(zoom) = self.follow_zoom {
            let (lat, lon) = lead.timeline.position_at(0.0);
            return Some(Framing {
                base: Camera::new(lat, lon, zoom, width, height, self.lens),
                points: Vec::new(),
                heights: Vec::new(),
                padding,
                bottom_inset,
            });
        }

        let points: Vec<_> = tracks
            .iter()
            .flat_map(|t| t.points.iter().cloned())
            .collect();
        let heights: Vec<f64> = tracks
            .iter()
            .flat_map(|t| relief.heights_m(&t.timeline))
            .collect();
        let camera = Camera::fit(
            &points,
            &heights,
            width,
            height,
            padding,
            bottom_inset,
            self.lens,
        )?;
        Some(Framing {
            base: camera,
            points,
            heights,
            padding,
            bottom_inset,
        })
    }
}

/// `camera` backed off to [`TERRAIN_CLEARANCE`] times the height of the
/// terrain's main area, and of the ground under its eye (surroundings built
/// for a zoom-out may rise higher there).
fn above(camera: Camera, terrain: &Terrain) -> Camera {
    camera
        .with_min_altitude(terrain.core_max_height() * TERRAIN_CLEARANCE)
        .clear_of_terrain(|x, y| terrain.height_at_world(x, y) * TERRAIN_CLEARANCE, 3)
}

/// The lead marker at progress `u` in `camera`'s world pixels, standing on
/// the ground: `x`, `y`, and its height.
fn marker_point(lead: &Track, u: f64, camera: &Camera, ground: Ground) -> (f64, f64, f64) {
    let (lat, lon) = lead.timeline.position_at(u);
    let track = camera.meters_to_px(ground.relief.height_m(lead.timeline.elevation_at(u)), lat);
    (
        lon_to_pixel(lon, camera.zoom),
        lat_to_pixel(lat, camera.zoom),
        track.max(ground.terrain.height_at(lat, lon)),
    )
}

impl Framing {
    /// Zoom level the cameras (and anything drawn with them) use.
    pub fn zoom(&self) -> f64 {
        self.base.zoom
    }

    /// Back the camera off so the route stays in view at every bearing the
    /// cameras will face (the span of `bearings`, at most a full turn).
    /// A follow-zoom framing keeps its fixed distance.
    pub fn cover(&mut self, bearings: &[f64]) {
        if self.points.is_empty() {
            return;
        }
        let lo = bearings.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = bearings.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if !lo.is_finite() || !hi.is_finite() {
            return;
        }
        self.base = self.base.refit_for_orbit(
            lo,
            &self.points,
            &self.heights,
            self.padding,
            self.bottom_inset,
            (hi - lo).min(360.0),
        );
    }
}

/// Relief shared by all tracks: automatic exaggeration from the lead track
/// (capped by [`Relief::over_terrain`] over terrain) unless
/// overridden, based at the lowest elevation of any track.
pub fn relief_for(tracks: &[Track], exaggeration: Option<f64>, terrain: bool) -> Relief {
    let mut relief = tracks.first().map_or(
        Relief {
            base_m: 0.0,
            exaggeration: 1.0,
        },
        |t| Relief::auto(&t.timeline),
    );
    if let Some(min) = tracks
        .iter()
        .filter_map(|t| t.timeline.elevation().map(|e| e.min))
        .reduce(f64::min)
    {
        relief.base_m = min;
    }
    if terrain {
        relief = relief.over_terrain();
    }
    if let Some(x) = exaggeration {
        relief.exaggeration = x.max(0.0);
    }
    relief
}

/// Everything a 3D frame draws besides the camera; shared across threads.
pub struct Scene<'a> {
    pub map: Option<&'a MapSource>,
    pub map_opacity: f32,
    pub background: Color,
    pub tracks: &'a [Track],
    pub styles: &'a [FrameStyle],
    /// Tail length per track as a fraction of progress (`1.0` = everything).
    pub tail_us: &'a [f64],
    pub relief: Relief,
    /// Ground relief drawn over the map, when terrain tiles were given.
    pub terrain: Option<&'a Terrain>,
    /// Map labels, when enabled.
    pub labels: Option<&'a LabelStyle>,
    /// Screen areas labels stay out of (overlays drawn later).
    pub label_rects: &'a [Rect],
    /// Label sources per tile, read once and reused across frames.
    pub label_cache: Mutex<LabelCache>,
    /// Whether the cameras zoom, so the map fades between tile zooms.
    pub fade: bool,
}

impl Scene<'_> {
    /// Map labels for one frame under `camera` (none without labels or a
    /// map), placed after the frame before it as `memory` recalls. Call in
    /// frame order; rendering can then run in parallel.
    pub fn place_labels(&self, camera: &Camera, memory: &mut LabelMemory) -> Vec<PlacedLabel> {
        let (Some(map), Some(style), Some(frame)) = (self.map, self.labels, self.styles.first())
        else {
            return Vec::new();
        };
        let (mut lines, mut areas) = (Vec::new(), Vec::new());
        for track in self.tracks {
            let (outline, curtain) =
                track_outline_3d(camera, &track.points, self.relief, self.terrain);
            lines.extend(outline);
            areas.extend(curtain);
        }
        let keepout = labels::keepout(self.label_rects, lines, areas, frame.tail_width, style);
        let context = LabelContext {
            canvas: (frame.width, frame.height),
            style,
            keepout: &keepout,
            fade: self.fade,
        };
        let mut cache = self
            .label_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.labels_3d(camera, &context, self.terrain, memory, &mut cache)
    }

    /// Raise the buildings of `map`'s style over the ground drawn on
    /// `pixmap` (and its terrain `depth`, which then holds them too).
    fn draw_buildings(
        &self,
        pixmap: &mut Pixmap,
        camera: &Camera,
        map: &MapSource,
        depth: &mut Option<DepthMap>,
    ) {
        let layers = map.buildings(camera, self.fade);
        let ground = |x: f64, y: f64| self.terrain.map_or(0.0, |t| t.height_at_world(x, y));
        let facets: Vec<(Vec<Facet>, f32)> = layers
            .iter()
            .map(|b| (b.facets(camera.zoom, ground), b.opacity * self.map_opacity))
            .filter(|(facets, _)| !facets.is_empty())
            .collect();
        if facets.is_empty() {
            return;
        }
        let layers: Vec<(&[Facet], f32)> = facets.iter().map(|(f, o)| (&f[..], *o)).collect();
        let depth = depth.get_or_insert_with(|| DepthMap::empty(pixmap.width(), pixmap.height()));
        draw_buildings(pixmap, camera, &layers, depth);
    }

    /// Render the ground, `labels` (from [`Scene::place_labels`]), and every
    /// track's 3D progress at `u` (no text overlays).
    pub fn render(&self, camera: &Camera, u: f64, labels: &[PlacedLabel]) -> Option<Pixmap> {
        let style = self.styles.first()?;
        let mut pixmap = Pixmap::new(style.width, style.height)?;
        pixmap.fill(self.background);
        if let Some(map) = self.map {
            map.draw_3d(&mut pixmap, camera, self.map_opacity, self.fade);
        }
        let mut depth = self
            .terrain
            .map(|terrain| draw_terrain(&mut pixmap, camera, terrain));
        if let Some(map) = self.map {
            self.draw_buildings(&mut pixmap, camera, map, &mut depth);
        }
        let fog = self
            .map
            .and_then(MapSource::fog_color)
            .or(self.terrain.map(|_| self.background));
        if let Some(color) = fog {
            draw_fog(&mut pixmap, camera, color);
        }
        if let Some(style) = self.labels {
            draw_labels(&mut pixmap, labels, style);
        }
        let view = SceneView {
            camera,
            relief: self.relief,
            terrain: self.terrain,
            occlusion: depth.as_ref(),
        };
        for (i, (track, style)) in self.tracks.iter().zip(self.styles).enumerate() {
            let tail_u = self.tail_us.get(i).copied().unwrap_or(1.0);
            draw_track_3d(&mut pixmap, &track.timeline, &view, u, tail_u, style);
        }
        Some(pixmap)
    }
}

#[cfg(test)]
mod tests {
    use gpx_motion_core::mercator::{pixel_to_lat, pixel_to_lon};

    use gpx_motion_core::{Dem, TileId, Timeline};

    use super::*;

    /// A DEM tile at zoom 13 with a cone-shaped 400 m hill in the middle.
    fn hill() -> (Dem, (u32, u32)) {
        let (tx, ty) = (6985, 3172);
        let mut rgba = Vec::with_capacity(256 * 256 * 4);
        for y in 0..256 {
            for x in 0..256 {
                let d = ((f64::from(x) - 128.0).powi(2) + (f64::from(y) - 128.0).powi(2)).sqrt();
                let v = (400.0 * (1.0 - d / 128.0)).max(0.0) + 32768.0;
                let r = (v / 256.0).floor();
                let g = (v - r * 256.0).floor();
                rgba.extend_from_slice(&[r as u8, g as u8, 0, 0xff]);
            }
        }
        let mut dem = Dem::new(13);
        dem.add_terrarium(
            TileId {
                zoom: 13,
                x: tx,
                y: ty,
            },
            &rgba,
        );
        (dem, (tx, ty))
    }

    /// Latitude and longitude of a zoom-13 tile pixel.
    fn coordinate(tx: u32, ty: u32, px: f64, py: f64) -> (f64, f64) {
        let (x, y) = (f64::from(tx) * 256.0 + px, f64::from(ty) * 256.0 + py);
        (pixel_to_lat(y, 13.0), pixel_to_lon(x, 13.0))
    }

    #[test]
    fn a_follow_camera_looks_down_past_a_hill_to_keep_the_marker() {
        let (dem, (tx, ty)) = hill();
        let relief = Relief {
            base_m: 0.0,
            exaggeration: 3.0,
        };
        let zoom = 14.0;
        let origin = (f64::from(tx) * 512.0, f64::from(ty) * 512.0);
        let terrain = Terrain::bare(&dem, &relief, zoom, origin, (512, 512));
        let ground = Ground {
            terrain: &terrain,
            relief,
        };
        // A route crossing the hill's north slope from west to east, seen
        // from the south.
        let points: Vec<TrackPoint> = (0..=12)
            .map(|i| {
                let (lat, lon) = coordinate(tx, ty, 68.0 + f64::from(i) * 10.0, 88.0);
                TrackPoint::new(lat, lon)
            })
            .collect();
        let lead = Track {
            timeline: Timeline::build(&points).unwrap(),
            points,
            color: Color::WHITE,
        };
        let perspective = Perspective {
            lens: Lens {
                pitch_deg: 70.0,
                bearing_deg: 0.0,
                fov_deg: 40.0,
            },
            orbit_deg: 0.0,
            follow_zoom: Some(zoom),
            heading: None,
            sight_frames: 4,
        };
        let framing = perspective
            .framing(std::slice::from_ref(&lead), &relief, 320, 240, 0.0, 0.0)
            .unwrap();
        let us: Vec<f64> = (0..=40).map(|i| f64::from(i) / 40.0).collect();
        let bearings = vec![0.0; us.len()];
        let hidden = |cameras: &[Camera]| {
            us.iter()
                .zip(cameras)
                .filter(|(&u, camera)| {
                    let (x, y, h) = marker_point(&lead, u, camera, ground);
                    terrain.hides_point(camera, x, y, h)
                })
                .count()
        };
        let level = Perspective {
            sight_frames: 0,
            ..perspective
        };
        let before: Vec<Camera> = us
            .iter()
            .map(|&u| level.grounded(&framing, &lead, u, 0.0, 70.0, Some(ground)))
            .collect();
        assert!(hidden(&before) > 0, "the hill hides the marker at first");
        let path = CameraPath {
            framing: &framing,
            lead: &lead,
            us: &us,
            bearings: &bearings,
        };
        let cameras = perspective.cameras(path, Some(ground));
        assert_eq!(hidden(&cameras), 0);
        // The camera tilts gradually, not in jumps.
        let pitches: Vec<f64> = cameras
            .iter()
            .map(gpx_motion_core::Camera::pitch_deg)
            .collect();
        assert!(pitches.iter().any(|&p| p < 70.0 - 1e-9));
        let step = pitches
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max);
        assert!(step < 5.0, "largest pitch step {step}");
    }
}
