//! Frame sequencing shared by the flat and 3D renderers: what a run
//! renders (one still or an animation's frames), the inputs every frame
//! shares, and where finished frames go.
//!
//! A PNG is rendered as a one-frame [`Shot::Still`] written to a
//! [`Sink::Png`]; a video's frames stream into ffmpeg through
//! [`Sink::Video`].

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use gpx_motion_core::math::smoothstep;
use gpx_motion_core::{FrameStyle, LabelStyle, TrackPoint};
use rayon::prelude::*;
use tiny_skia::{Pixmap, Rect};

use crate::config::Config;
use crate::encode::FfmpegEncoder;
use crate::hud::Hud;
use crate::track::{all_points, Track};

/// 2D tail length when `--tail-duration` is unset.
const DEFAULT_TAIL_MS: f64 = 5000.0;

/// Bytes of in-flight frames [`Sink::write_parallel`] keeps at once.
const MEMORY_BUDGET: usize = 256 * 1024 * 1024;

/// What a run renders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shot {
    /// One image of the finished route(s): framed from the start, as a
    /// video's first frame would be, but drawn complete.
    Still,
    /// An animation of `frames` frames over `ms` milliseconds.
    Video { ms: f64, frames: u64 },
}

impl Shot {
    /// The shot for the output kind; a video lasts `--total-time`, else as
    /// long as the longest timestamped track.
    pub fn of(config: &Config, tracks: &[Track]) -> Result<Self> {
        if !config.is_video() {
            return Ok(Shot::Still);
        }
        let ms = config
            .total_time
            .or_else(|| {
                tracks
                    .iter()
                    .filter_map(|t| t.timeline.duration_ms)
                    .fold(None, |acc, d| Some(f64::max(acc.unwrap_or(0.0), d)))
            })
            .context("no track has timestamps; pass --total-time")?;
        if ms <= 0.0 {
            bail!("video duration must be positive");
        }
        let frames = ((ms / 1000.0) * config.fps).round().max(1.0) as u64;
        Ok(Shot::Video { ms, frames })
    }

    /// Number of frames rendered.
    pub fn frames(&self) -> u64 {
        match *self {
            Shot::Still => 1,
            Shot::Video { frames, .. } => frames,
        }
    }

    /// Video length in seconds; `None` for a still.
    pub fn seconds(&self) -> Option<f64> {
        match *self {
            Shot::Still => None,
            Shot::Video { ms, .. } => Some(ms / 1000.0),
        }
    }

    /// Route progress `frame` draws, from `0` (start) to `1` (finished).
    pub fn progress(&self, frame: u64) -> f64 {
        let frames = self.frames();
        if frames > 1 {
            frame as f64 / (frames - 1) as f64
        } else {
            1.0
        }
    }

    /// Route progress each frame's camera is placed at, in frame order: a
    /// still's camera stands at the start of the route.
    pub fn camera_us(&self) -> Vec<f64> {
        match self {
            Shot::Still => vec![0.0],
            Shot::Video { .. } => (0..self.frames()).map(|f| self.progress(f)).collect(),
        }
    }
}

/// How far into the follow zoom (`0` = whole route, `1` = `--zoom`) the
/// camera is at progress `u` of a `video_s`-second video: eased in over the
/// first `transition_s` seconds and back out over the last.
pub fn zoom_mix(u: f64, video_s: f64, transition_s: f64) -> f64 {
    let t = u * video_s;
    smoothstep(t.min(video_s - t) / transition_s)
}

/// The inputs every frame of a run shares (safe to share across threads).
pub struct Job<'a> {
    /// The validated options.
    pub config: &'a Config,
    /// One still or a video's frames.
    pub shot: Shot,
    /// The input tracks; the first is the lead.
    pub tracks: Vec<Track>,
    /// Every point of every track.
    pub points: Vec<TrackPoint>,
    /// Per-track styles carrying the palette colors.
    pub styles: Vec<FrameStyle>,
    /// Tail length per track as a fraction of its progress (`1.0` draws
    /// everything behind the marker).
    pub tail_us: Vec<f64>,
    /// Map labels, when `--labels` is set.
    pub labels: Option<LabelStyle>,
    /// The overlays drawn over every frame.
    pub hud: Hud,
}

impl<'a> Job<'a> {
    /// Style the `tracks` for `shot`.
    pub fn new(
        config: &'a Config,
        shot: Shot,
        tracks: Vec<Track>,
        labels: Option<LabelStyle>,
    ) -> Self {
        let styles = tracks
            .iter()
            .map(|t| {
                FrameStyle {
                    width: config.viewport_width,
                    height: config.viewport_height,
                    tail_color: t.color,
                    color_by_elevation: config.color_by_elevation,
                    ..Default::default()
                }
                .scaled(config.pixel_ratio)
            })
            .collect();
        let tail_us = tracks.iter().map(|t| tail_u(config, &shot, t)).collect();
        Self {
            config,
            shot,
            points: all_points(&tracks),
            tracks,
            styles,
            tail_us,
            labels,
            hud: Hud::new(config, &shot),
        }
    }

    /// The track the camera and overlays follow.
    pub fn lead(&self) -> &Track {
        &self.tracks[0]
    }

    /// The lead track's style, which also sizes the frame.
    pub fn lead_style(&self) -> &FrameStyle {
        &self.styles[0]
    }

    /// Screen areas the overlays take on a `canvas`-sized frame, for
    /// keeping map labels clear (see [`Hud::rects`]).
    pub fn label_rects(&self, canvas: (f32, f32)) -> Vec<Rect> {
        self.hud.rects(self.lead(), canvas)
    }

    /// Draw the overlays for progress `u` (see [`Hud::draw`]).
    pub fn draw_hud(&self, pixmap: &mut Pixmap, u: f64, map_shown: bool) {
        self.hud
            .draw(pixmap, self.lead(), self.lead_style(), u, map_shown);
    }

    /// The frame size in device pixels, as overlay geometry takes it.
    pub fn canvas(&self) -> (f32, f32) {
        (
            self.config.viewport_width as f32,
            self.config.viewport_height as f32,
        )
    }
}

/// `track`'s tail length as a fraction of its progress: relative to its own
/// span (real time when timestamped, else the video's), the whole route in
/// 3D or a still.
fn tail_u(config: &Config, shot: &Shot, track: &Track) -> f64 {
    let Shot::Video { ms: video_ms, .. } = *shot else {
        return 1.0;
    };
    let reference_ms = track.timeline.duration_ms.unwrap_or(video_ms);
    match config.tail_duration {
        Some(ms) => (ms / reference_ms).clamp(0.0, 1.0),
        None if config.is_3d() => 1.0,
        None => (DEFAULT_TAIL_MS / reference_ms).clamp(0.0, 1.0),
    }
}

/// Where finished frames go.
pub enum Sink {
    /// The single frame of a [`Shot::Still`], saved as a PNG.
    Png(PathBuf),
    /// Frames piped to ffmpeg in order.
    Video {
        encoder: FfmpegEncoder,
        frame_bytes: usize,
    },
}

impl Sink {
    /// Open the output: start ffmpeg for a video, else remember the PNG path.
    pub fn open(config: &Config) -> Result<Self> {
        if !config.is_video() {
            return Ok(Sink::Png(config.output.clone()));
        }
        let encoder = FfmpegEncoder::start(
            &config.output,
            config.viewport_width,
            config.viewport_height,
            config.fps,
        )?;
        let frame_bytes = (config.viewport_width as usize) * (config.viewport_height as usize) * 4;
        Ok(Sink::Video {
            encoder,
            frame_bytes,
        })
    }

    /// Write the next frame.
    pub fn write(&mut self, pixmap: &Pixmap) -> Result<()> {
        match self {
            Sink::Png(path) => pixmap
                .save_png(&*path)
                .with_context(|| path.display().to_string()),
            Sink::Video { encoder, .. } => Ok(encoder.write_frame(pixmap.data())?),
        }
    }

    /// Render every frame of `shot` with `render` and write them in order.
    ///
    /// Frames are independent, so they render in memory-bounded chunks
    /// across all rayon workers; each chunk is then written sequentially.
    /// The chunk size targets [`MEMORY_BUDGET`] regardless of viewport size.
    pub fn write_parallel(
        &mut self,
        shot: &Shot,
        render: impl Fn(u64) -> Result<Pixmap> + Sync,
    ) -> Result<()> {
        let frame_bytes = match self {
            Sink::Png(_) => usize::MAX,
            Sink::Video { frame_bytes, .. } => *frame_bytes,
        };
        let chunk = (MEMORY_BUDGET / frame_bytes.max(1)).clamp(1, 256) as u64;
        let total = shot.frames();
        let mut start = 0;
        while start < total {
            let end = (start + chunk).min(total);
            let pixmaps: Vec<Pixmap> = (start..end)
                .into_par_iter()
                .map(&render)
                .collect::<Result<_>>()?;
            for pixmap in &pixmaps {
                self.write(pixmap)?;
            }
            start = end;
        }
        Ok(())
    }

    /// Flush the output: wait for ffmpeg to finish the file.
    pub fn finish(self) -> Result<()> {
        match self {
            Sink::Png(_) => Ok(()),
            Sink::Video { encoder, .. } => Ok(encoder.finish()?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_mix_eases_in_and_back_out() {
        assert_eq!(zoom_mix(0.0, 60.0, 5.0), 0.0);
        assert_eq!(zoom_mix(1.0, 60.0, 5.0), 0.0);
        assert_eq!(zoom_mix(0.5, 60.0, 5.0), 1.0);
        let half = zoom_mix(2.5 / 60.0, 60.0, 5.0);
        assert!((half - 0.5).abs() < 1e-9);
        assert!(zoom_mix(0.01, 60.0, 5.0) < zoom_mix(0.02, 60.0, 5.0));
        assert!(zoom_mix(0.99, 60.0, 5.0) < zoom_mix(0.98, 60.0, 5.0));
        assert!(zoom_mix(0.5, 6.0, 5.0) < 1.0, "too short to reach the zoom");
    }

    #[test]
    fn a_still_is_one_finished_frame_framed_from_the_start() {
        assert_eq!(Shot::Still.frames(), 1);
        assert_eq!(Shot::Still.progress(0), 1.0);
        assert_eq!(Shot::Still.camera_us(), vec![0.0]);
        assert_eq!(Shot::Still.seconds(), None);
        let video = Shot::Video {
            ms: 2000.0,
            frames: 5,
        };
        assert_eq!(video.camera_us(), vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(video.seconds(), Some(2.0));
    }
}
