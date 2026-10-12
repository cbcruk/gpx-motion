//! The flat (top-down) view: a Web Mercator projection over a composited
//! map background, either fit to the whole route once or following the
//! lead marker frame by frame.

use anyhow::{bail, Context, Result};
use gpx_motion_core::geo::mercator::Projection;
use gpx_motion_core::{draw_labels, draw_overlay, stroke_track, stroke_track_by_elevation};
use tiny_skia::{Pixmap, Rect};

use crate::labels;
use crate::tiles::{LabelContext, MapSource};
use crate::video::{zoom_mix, Job, Shot, Sink};

/// Where the flat camera looks.
enum View {
    /// The whole route, fit once. `None` when it can't be fit: a still then
    /// shows only the background.
    Whole(Option<Projection>),
    /// Centered on the lead marker at `zoom`; with `wide` (the whole-route
    /// fit and `--zoom-transition` seconds), eased out to the whole route at
    /// both ends of the video.
    Follow {
        zoom: f64,
        wide: Option<(Projection, f64)>,
    },
}

/// Render every frame of `job` in the flat view over `map` into `sink`.
pub fn render(job: &Job, mut map: Option<&mut MapSource>, sink: &mut Sink) -> Result<()> {
    let label_rects = job.hud.rects(job.lead(), job.canvas());
    let map_shown = map.is_some();
    match view(job)? {
        // The background is composited once; frames are independent clones
        // with the tracks drawn on, so they render in parallel.
        View::Whole(proj) => {
            let base = match &proj {
                Some(proj) => build_base(job, proj, map.as_deref_mut(), &label_rects)?,
                None => {
                    let style = job.lead_style();
                    let mut pixmap =
                        Pixmap::new(style.width, style.height).context("invalid viewport size")?;
                    pixmap.fill(style.background);
                    pixmap
                }
            };
            sink.write_parallel(&job.shot, |frame| {
                let u = job.shot.progress(frame);
                let mut pixmap = base.clone();
                if let Some(proj) = &proj {
                    draw_tracks(job, &mut pixmap, proj, u);
                }
                job.hud
                    .draw(&mut pixmap, job.lead(), job.lead_style(), u, map_shown);
                Ok(pixmap)
            })
        }
        // Each frame recenters, so it composites its own (cache-backed)
        // background; frames are rendered in order.
        View::Follow { zoom, wide } => {
            let video_s = job.shot.seconds().unwrap_or(0.0);
            for frame in 0..job.shot.frames() {
                let u = job.shot.progress(frame);
                let (lat, lon) = job.lead().timeline.position_at(u);
                let config = job.config;
                let close = Projection::centered(
                    lat,
                    lon,
                    zoom,
                    config.viewport_width,
                    config.viewport_height,
                );
                let proj = match wide {
                    Some((fit, transition_s)) => {
                        fit.zoom_between(&close, zoom_mix(u, video_s, transition_s))
                    }
                    None => close,
                };
                let mut pixmap = build_base(job, &proj, map.as_deref_mut(), &label_rects)?;
                draw_tracks(job, &mut pixmap, &proj, u);
                job.hud
                    .draw(&mut pixmap, job.lead(), job.lead_style(), u, map_shown);
                sink.write(&pixmap)?;
            }
            Ok(())
        }
    }
}

/// The camera for `job`: a still always shows the whole route (`--zoom`
/// only steers a video), fit above the profile panel when there is one.
fn view(job: &Job) -> Result<View> {
    let config = job.config;
    let fit_height = config
        .viewport_height
        .saturating_sub(job.hud.reserved_bottom() as u32)
        .max(1);
    let fit = Projection::fit(
        &job.points,
        config.viewport_width,
        fit_height,
        job.lead_style().padding,
    );
    let unfit = "could not fit track to viewport";
    Ok(match (job.shot, config.follow_zoom()) {
        (Shot::Still, _) => View::Whole(fit),
        (Shot::Video { .. }, None) => View::Whole(Some(fit.context(unfit)?)),
        (Shot::Video { .. }, Some(zoom)) => {
            let wide = match config.zoom_transition {
                Some(transition_s) => Some((fit.context(unfit)?, transition_s)),
                None => None,
            };
            View::Follow { zoom, wide }
        }
    })
}

/// Draw every track at progress `u`: a video's fading tails and markers,
/// or a still's complete routes.
fn draw_tracks(job: &Job, pixmap: &mut Pixmap, proj: &Projection, u: f64) {
    let tracks = job.tracks.iter().zip(&job.styles).zip(&job.tail_us);
    for ((track, style), &tail_u) in tracks {
        match job.shot {
            Shot::Video { .. } => draw_overlay(pixmap, &track.timeline, proj, u, tail_u, style),
            Shot::Still if style.color_by_elevation => stroke_track_by_elevation(
                pixmap,
                &track.timeline,
                proj,
                style.tail_color,
                style.tail_width,
            ),
            Shot::Still => stroke_track(
                pixmap,
                &track.points,
                proj,
                style.tail_color,
                style.tail_width,
            ),
        }
    }
}

/// Build the frame background for a projection: solid fill, optional map
/// (raster or vector) with its labels kept clear of `label_rects`, and the
/// optional pre-drawn full route.
fn build_base(
    job: &Job,
    proj: &Projection,
    map: Option<&mut MapSource>,
    label_rects: &[Rect],
) -> Result<Pixmap> {
    let config = job.config;
    let style = job.lead_style();
    let mut base = Pixmap::new(style.width, style.height).context("invalid viewport size")?;
    base.fill(style.background);

    if let Some(map) = map {
        let fade = config.zoom_transition.is_some();
        map.draw(&mut base, proj, config.background_map_visibility, fade);
        if let Some(reason) = map.failure() {
            bail!("{reason}");
        }
        if let Some(label_style) = &job.labels {
            let screen = job
                .tracks
                .iter()
                .map(|t| proj.project_path(&t.points))
                .collect();
            let keepout = labels::keepout(
                label_rects,
                screen,
                Vec::new(),
                style.tail_width,
                label_style,
            );
            let context = LabelContext {
                canvas: (style.width, style.height),
                style: label_style,
                keepout: &keepout,
                fade,
            };
            let placed = map.labels_2d(proj, &context);
            draw_labels(&mut base, &placed, label_style);
        }
    }

    if let Some(color) = config.pre_draw_color {
        for track in &job.tracks {
            stroke_track(&mut base, &track.points, proj, color, style.tail_width);
        }
    }

    Ok(base)
}
