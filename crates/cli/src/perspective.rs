//! Tilted 3D view: camera framing, per-frame cameras, and frame rendering.

use gpx_motion_core::{draw_track_3d, steer, Camera, FrameStyle, Lens, Relief, TrackPoint};
use tiny_skia::{Color, Pixmap};

use crate::tiles::MapSource;
use crate::Track;

/// Camera settings for the 3D view.
#[derive(Debug, Clone, Copy)]
pub struct Perspective {
    pub lens: Lens,
    /// Degrees the bearing turns over the whole animation.
    pub orbit_deg: f64,
    /// Fixed zoom that follows the lead marker instead of framing the route.
    pub follow_zoom: Option<u32>,
    /// Turn the camera with the lead marker's direction of travel instead of
    /// orbiting; `lens.bearing_deg` becomes an offset from the heading.
    pub heading: Option<HeadingFollow>,
}

/// How the camera tracks the direction of travel.
#[derive(Debug, Clone, Copy)]
pub struct HeadingFollow {
    /// Meters of track the heading is averaged over.
    pub window_m: f64,
    /// Largest bearing change between consecutive frames, in degrees.
    pub max_turn_deg: f64,
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
    /// One camera per progress value in `us`: orbiting from the base bearing,
    /// or turning with the lead marker's heading (rate-limited across the
    /// sequence, so pass every frame in order).
    pub fn cameras(&self, framing: &Framing, lead: &Track, us: &[f64]) -> Vec<Camera> {
        let bearings: Vec<f64> = match self.heading {
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
        };
        us.iter()
            .zip(bearings)
            .map(|(&u, bearing)| self.camera_at(framing, lead, u, bearing))
            .collect()
    }

    /// Camera at progress `u` facing `bearing_deg`: re-aimed so the route
    /// stays centered, or in follow mode aimed at the lead marker.
    fn camera_at(&self, framing: &Framing, lead: &Track, u: f64, bearing_deg: f64) -> Camera {
        let camera = framing.base.with_bearing(bearing_deg);
        if self.follow_zoom.is_some() {
            let (lat, lon) = lead.timeline.position_at(u);
            camera.with_target(lat, lon)
        } else {
            camera.aim(
                &framing.points,
                &framing.heights,
                framing.padding,
                framing.bottom_inset,
            )
        }
    }

    /// The starting framing: around every track (far enough to stay in view
    /// across the whole orbit), or at the follow zoom on the start point.
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
        // A heading-following camera may face any direction.
        let turn = if self.heading.is_some() {
            360.0
        } else {
            self.orbit_deg
        };
        let base = camera.widen_for_orbit(&points, &heights, padding, bottom_inset, turn);
        Some(Framing {
            base,
            points,
            heights,
            padding,
            bottom_inset,
        })
    }
}

/// Relief shared by all tracks: automatic exaggeration from the lead track
/// unless overridden, based at the lowest elevation of any track.
pub fn relief_for(tracks: &[Track], exaggeration: Option<f64>) -> Relief {
    let mut relief = tracks
        .first()
        .map(|t| Relief::auto(&t.timeline))
        .unwrap_or(Relief {
            base_m: 0.0,
            exaggeration: 1.0,
        });
    if let Some(min) = tracks
        .iter()
        .filter_map(|t| t.timeline.elevation().map(|e| e.min))
        .reduce(f64::min)
    {
        relief.base_m = min;
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
    pub relief: Relief,
}

impl Scene<'_> {
    /// Render the ground and every track's 3D progress at `u` (no text overlays).
    pub fn render(&self, camera: &Camera, u: f64) -> Option<Pixmap> {
        let style = self.styles.first()?;
        let mut pixmap = Pixmap::new(style.width, style.height)?;
        pixmap.fill(self.background);
        if let Some(map) = self.map {
            map.draw_3d(&mut pixmap, camera, self.map_opacity);
        }
        for (track, style) in self.tracks.iter().zip(self.styles) {
            draw_track_3d(&mut pixmap, &track.timeline, camera, &self.relief, u, style);
        }
        Some(pixmap)
    }
}
