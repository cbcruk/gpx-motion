//! The tilted 3D view: the camera settings from the options, framing and
//! per-frame cameras, terrain, and label placement ahead of rendering.

use anyhow::{Context, Result};
use gpx_motion_core::{LabelMemory, Lens, PlacedLabel, Relief, Terrain};
use tiny_skia::Rect;

use crate::config::Config;
use crate::frames::{zoom_mix, Job, Shot, Sink};
use crate::perspective::{
    relief_for, CameraPath, Framing, Ground, HeadingFollow, Perspective, Scene,
};
use crate::terrain;
use crate::tiles::MapSource;
use crate::track::Track;

/// Seconds a map label takes to fade in when it first appears in a video.
const LABEL_FADE_S: f64 = 0.4;

/// Distance the heading is averaged over when `--heading-window` is unset.
const MIN_HEADING_WINDOW_M: f64 = 150.0;
/// Seconds of video whose travel the default heading window spans.
const HEADING_WINDOW_S: f64 = 2.0;
/// Seconds over which a follow camera eases into looking down more steeply
/// to keep the marker in sight over hills (and out again).
const SIGHT_EASE_S: f64 = 1.0;

/// Render every frame of `job` in the 3D view over `map` into `sink`.
///
/// Cameras and labels depend on the frame before, so they are worked out
/// in order first; frames then render in parallel.
pub fn render(job: &Job, mut map: Option<&mut MapSource>, sink: &mut Sink) -> Result<()> {
    let config = job.config;
    let lead = job.lead();
    let perspective = perspective_of(config, lead, job.shot.seconds());
    let relief = relief_for(
        &job.tracks,
        config.elevation_exaggeration,
        config.terrain_tiles.is_some(),
    );
    let us = job.shot.camera_us();
    let bearings = perspective.bearings(lead, &us);
    let framing = fit_framing(job, &perspective, &relief, &bearings)?;
    // With --zoom-transition, the ends of the video show the whole route
    // from this wider framing.
    let wide = Perspective {
        follow_zoom: None,
        ..perspective
    };
    let wide_framing = match config.zoom_transition {
        Some(transition_s) => Some((fit_framing(job, &wide, &relief, &bearings)?, transition_s)),
        None => None,
    };
    let terrain = terrain_of(
        job,
        &relief,
        framing.zoom(),
        wide_framing.as_ref().map(|(f, _)| f.zoom()),
        map.as_deref_mut(),
    )?;
    let ground = terrain.as_ref().map(|terrain| Ground { terrain, relief });
    let path = CameraPath {
        framing: &framing,
        lead,
        us: &us,
        bearings: &bearings,
    };
    let mut cameras = perspective.cameras(path, ground);
    if let Some((wide_framing, transition_s)) = &wide_framing {
        let video_s = job.shot.seconds().unwrap_or(0.0);
        let mixes: Vec<f64> = us
            .iter()
            .map(|&u| zoom_mix(u, video_s, *transition_s))
            .collect();
        let wide_path = CameraPath {
            framing: wide_framing,
            ..path
        };
        cameras = wide.zoom_into(wide_path, &cameras, &mixes, ground);
    }
    let fade = config.zoom_transition.is_some();
    if let Some(map) = map.as_deref_mut() {
        for camera in &cameras {
            map.prefetch_3d(camera, fade)?;
        }
    }

    let label_rects: Vec<Rect> = job.label_rects(job.canvas());
    let scene = Scene {
        map: map.as_deref(),
        map_opacity: config.background_map_visibility,
        background: job.lead_style().background,
        tracks: &job.tracks,
        styles: &job.styles,
        tail_us: &job.tail_us,
        relief,
        terrain: terrain.as_ref(),
        labels: job.labels.as_ref(),
        label_rects: &label_rects,
        label_cache: Default::default(),
        fade,
    };
    let fade_frames = match job.shot {
        Shot::Still => 1,
        Shot::Video { .. } => (LABEL_FADE_S * config.fps).round() as u32,
    };
    let mut memory = LabelMemory::new(fade_frames);
    let labels: Vec<Vec<PlacedLabel>> = cameras
        .iter()
        .map(|camera| scene.place_labels(camera, &mut memory))
        .collect();
    let map_shown = scene.map.is_some() || terrain.is_some();
    sink.write_parallel(&job.shot, |frame| {
        let u = job.shot.progress(frame);
        let i = frame as usize;
        let mut pixmap = scene
            .render(&cameras[i], u, &labels[i])
            .context("invalid viewport size")?;
        job.draw_hud(&mut pixmap, u, map_shown);
        Ok(pixmap)
    })
}

/// `perspective`'s starting framing of the tracks above the profile panel,
/// backed off to keep the route in view at every one of `bearings`.
fn fit_framing(
    job: &Job,
    perspective: &Perspective,
    relief: &Relief,
    bearings: &[f64],
) -> Result<Framing> {
    let style = job.lead_style();
    let mut framing = perspective
        .framing(
            &job.tracks,
            relief,
            style.width,
            style.height,
            style.padding,
            job.hud.reserved_bottom() as f64,
        )
        .context("could not fit track to viewport")?;
    framing.cover(bearings);
    Ok(framing)
}

/// The 3D camera settings from the command line. `video_s` (absent for a
/// still image) sizes the default heading window to the playback speed.
fn perspective_of(config: &Config, lead: &Track, video_s: Option<f64>) -> Perspective {
    let heading = if config.follow_heading {
        let fps = video_s.map_or(1.0, |_| config.fps);
        let window_m = config.heading_window.unwrap_or_else(|| {
            let per_second = video_s.map_or(0.0, |s| lead.timeline.total_distance_m() / s);
            (per_second * HEADING_WINDOW_S).max(MIN_HEADING_WINDOW_M)
        });
        Some(HeadingFollow {
            window_m,
            max_turn_deg: config.max_turn_rate / fps,
        })
    } else {
        None
    };
    Perspective {
        lens: Lens {
            pitch_deg: config.pitch,
            bearing_deg: config.bearing,
            fov_deg: config.fov,
        },
        orbit_deg: config.orbit,
        follow_zoom: config.follow_zoom(),
        heading,
        sight_frames: video_s.map_or(0, |_| (SIGHT_EASE_S * config.fps).round() as usize),
    }
}

/// Terrain under the route at the cameras' `zoom` (with surroundings for
/// a zoom out to `wide_zoom`) when `--terrain-tiles` is set, textured with
/// `map`.
fn terrain_of(
    job: &Job,
    relief: &Relief,
    zoom: f64,
    wide_zoom: Option<f64>,
    map: Option<&mut MapSource>,
) -> Result<Option<Terrain>> {
    let config = job.config;
    let Some(template) = &config.terrain_tiles else {
        return Ok(None);
    };
    let sources = terrain::Sources {
        template,
        cache: config.tile_cache.clone(),
        map,
        backdrop: terrain::Backdrop {
            color: job.lead_style().background,
            map_opacity: config.background_map_visibility,
        },
    };
    terrain::build(sources, &job.points, relief, zoom, wide_zoom)
}
