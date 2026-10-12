//! The validated run configuration: every option checked against the
//! others once, before any file or network is touched.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use gpx_motion_core::Anchor;
use tiny_skia::Color;

use crate::cli::Args;

/// What the run writes, chosen by the output extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    /// A single PNG of the full route(s).
    Png,
    /// An MP4 animation (`.mp4`, any case).
    Video,
}

/// Where the background map comes from, as the options name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapSpec {
    /// A TMS/XYZ raster tile URL template.
    Raster { template: String },
    /// Vector tiles (TileJSON URL, XYZ template, or PMTiles archive) drawn
    /// in a style.
    Vector {
        url: String,
        max_zoom: u32,
        style: String,
        export_style: Option<PathBuf>,
    },
}

/// Settings for `--labels`.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelOptions {
    /// `--label-font`; `None` looks for a system Korean/CJK font.
    pub font: Option<PathBuf>,
    /// `--label-language`, the `name:<code>` to prefer.
    pub language: Option<String>,
    /// Text size in CSS pixels.
    pub size: f32,
}

/// The command line after validation: options that only apply together
/// were checked together, and positions and colors are parsed.
#[derive(Debug, Clone)]
pub struct Config {
    pub inputs: Vec<PathBuf>,
    pub output: PathBuf,
    pub kind: OutputKind,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub fps: f64,
    pub total_time: Option<f64>,
    pub tail_duration: Option<f64>,
    pub map: Option<MapSpec>,
    pub terrain_tiles: Option<String>,
    pub background_map_visibility: f32,
    /// The pre-drawn route color, set with `--pre-draw-track`.
    pub pre_draw_color: Option<Color>,
    pub zoom: Option<f64>,
    pub zoom_transition: Option<f64>,
    pub info_anchor: Option<Anchor>,
    pub attribution: Option<String>,
    pub attribution_anchor: Option<Anchor>,
    pub color_by_elevation: bool,
    pub elevation_profile: bool,
    pub pitch: f64,
    pub bearing: f64,
    pub orbit: f64,
    pub follow_heading: bool,
    pub heading_window: Option<f64>,
    pub max_turn_rate: f64,
    pub fov: f64,
    pub elevation_exaggeration: Option<f64>,
    pub elevation_smoothing: f64,
    /// Set with `--labels`.
    pub labels: Option<LabelOptions>,
    pub pixel_ratio: f32,
    pub jobs: Option<usize>,
    /// The on-disk tile cache, unless `--no-tile-cache` (or no `$HOME`).
    pub tile_cache: Option<PathBuf>,
}

impl Config {
    /// Whether the output is an MP4 animation.
    pub fn is_video(&self) -> bool {
        self.kind == OutputKind::Video
    }

    /// Whether the camera is tilted into the 3D view.
    pub fn is_3d(&self) -> bool {
        self.pitch > 0.0
    }
}

impl TryFrom<Args> for Config {
    type Error = anyhow::Error;

    /// Checks the options that depend on each other or on the output kind;
    /// single values were range-checked while parsing.
    fn try_from(args: Args) -> Result<Self> {
        let kind = output_kind(&args.output);
        let video = kind == OutputKind::Video;
        let is_3d = args.pitch > 0.0;

        if args.zoom_transition.is_some() {
            if args.zoom.is_none() {
                bail!("--zoom-transition needs --zoom (the zoom to move into)");
            }
            if !video {
                bail!("--zoom-transition only applies to video output");
            }
        }
        if !video {
            for (flag, set) in [
                ("--elevation-profile", args.elevation_profile),
                ("--pre-draw-track", args.pre_draw_track),
                ("--tail-duration", args.tail_duration.is_some()),
            ] {
                if set {
                    bail!("{flag} only applies to video output");
                }
            }
        }
        if args.follow_heading && !is_3d {
            bail!("--follow-heading needs the 3D view (--pitch > 0)");
        }
        if args.follow_heading && args.orbit != 0.0 {
            bail!("--follow-heading and --orbit both turn the camera; pick one");
        }
        if args.terrain_tiles.is_some() && !is_3d {
            bail!("--terrain-tiles needs the 3D view (--pitch > 0)");
        }
        if args.labels {
            if args.tms_url_template.is_some() {
                bail!("--labels needs a vector map (--vector-tiles); raster tiles carry no names");
            }
            if args.vector_tiles.is_none() {
                bail!("--labels needs a vector map (--vector-tiles)");
            }
        }
        if args.output.is_dir() {
            bail!("{}: is a directory", args.output.display());
        }

        let map = match (args.tms_url_template, args.vector_tiles) {
            (Some(template), _) => Some(MapSpec::Raster { template }),
            (None, Some(url)) => Some(MapSpec::Vector {
                url,
                max_zoom: args.vector_max_zoom,
                style: args.map_style,
                export_style: args.export_style,
            }),
            (None, None) => None,
        };
        let labels = args.labels.then_some(LabelOptions {
            font: args.label_font,
            language: args.label_language,
            size: args.label_size,
        });
        let tile_cache = if args.no_tile_cache {
            None
        } else {
            args.tile_cache_dir.or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".cache/gpx-motion/tiles"))
            })
        };

        Ok(Config {
            inputs: args.input,
            output: args.output,
            kind,
            viewport_width: args.viewport_width,
            viewport_height: args.viewport_height,
            fps: args.fps,
            total_time: args.total_time,
            tail_duration: args.tail_duration,
            map,
            terrain_tiles: args.terrain_tiles,
            background_map_visibility: args.background_map_visibility,
            pre_draw_color: args.pre_draw_track.then_some(args.pre_draw_track_color),
            zoom: args.zoom,
            zoom_transition: args.zoom_transition,
            info_anchor: args.information_position.anchor(),
            attribution: args.attribution,
            attribution_anchor: args.attribution_position.anchor(),
            color_by_elevation: args.color_by_elevation,
            elevation_profile: args.elevation_profile,
            pitch: args.pitch,
            bearing: args.bearing,
            orbit: args.orbit,
            follow_heading: args.follow_heading,
            heading_window: args.heading_window,
            max_turn_rate: args.max_turn_rate,
            fov: args.fov,
            elevation_exaggeration: args.elevation_exaggeration,
            elevation_smoothing: args.elevation_smoothing,
            labels,
            pixel_ratio: args.pixel_ratio,
            jobs: args.jobs,
            tile_cache,
        })
    }
}

/// [`OutputKind::Video`] for a `.mp4` path (any case), else
/// [`OutputKind::Png`].
fn output_kind(path: &Path) -> OutputKind {
    let mp4 = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
    if mp4 {
        OutputKind::Video
    } else {
        OutputKind::Png
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    fn config(extra: &[&str]) -> Result<Config> {
        let base = ["gpx-motion", "-i", "a.gpx"];
        Config::try_from(Args::try_parse_from(base.iter().chain(extra))?)
    }

    fn rejects(extra: &[&str], message: &str) {
        match config(extra) {
            Ok(_) => panic!("{extra:?} was accepted"),
            Err(e) => assert_eq!(e.to_string(), message, "{extra:?}"),
        }
    }

    #[test]
    fn video_only_options_are_rejected_for_png() {
        rejects(
            &["--zoom", "14", "--zoom-transition", "2"],
            "--zoom-transition only applies to video output",
        );
        rejects(
            &["--elevation-profile"],
            "--elevation-profile only applies to video output",
        );
        rejects(
            &["-o", "x.png", "--pre-draw-track"],
            "--pre-draw-track only applies to video output",
        );
        rejects(
            &["--tail-duration", "1000"],
            "--tail-duration only applies to video output",
        );
        let video = config(&[
            "-o",
            "x.MP4",
            "--elevation-profile",
            "--pre-draw-track",
            "--tail-duration",
            "1000",
            "--zoom",
            "14",
            "--zoom-transition",
            "2",
        ])
        .unwrap();
        assert!(video.is_video());
        assert!(video.elevation_profile);
        assert!(video.pre_draw_color.is_some());
    }

    #[test]
    fn zoom_transition_needs_zoom() {
        rejects(
            &["-o", "x.mp4", "--zoom-transition", "2"],
            "--zoom-transition needs --zoom (the zoom to move into)",
        );
    }

    #[test]
    fn camera_options_need_the_3d_view() {
        rejects(
            &["--follow-heading"],
            "--follow-heading needs the 3D view (--pitch > 0)",
        );
        rejects(
            &[
                "--pitch",
                "0",
                "--terrain-tiles",
                "https://t/{z}/{x}/{y}.png",
            ],
            "--terrain-tiles needs the 3D view (--pitch > 0)",
        );
        rejects(
            &["--pitch", "45", "--follow-heading", "--orbit", "90"],
            "--follow-heading and --orbit both turn the camera; pick one",
        );
        let ok = config(&["--pitch", "45", "--follow-heading", "--orbit", "0"]).unwrap();
        assert!(ok.is_3d() && ok.follow_heading);
    }

    #[test]
    fn labels_need_a_vector_map() {
        rejects(
            &["--labels"],
            "--labels needs a vector map (--vector-tiles)",
        );
        rejects(
            &[
                "--labels",
                "--tms-url-template",
                "https://t/{z}/{x}/{y}.png",
            ],
            "--labels needs a vector map (--vector-tiles); raster tiles carry no names",
        );
        let ok = config(&[
            "--labels",
            "--vector-tiles",
            "https://tiles.example/planet",
            "--label-size",
            "20",
            "--label-language",
            "en",
        ])
        .unwrap();
        let labels = ok.labels.unwrap();
        assert_eq!(labels.size, 20.0);
        assert_eq!(labels.language.as_deref(), Some("en"));
    }

    #[test]
    fn output_must_not_be_a_directory() {
        let dir = std::env::temp_dir();
        let dir = dir.to_str().unwrap();
        rejects(&["-o", dir], &format!("{dir}: is a directory"));
    }

    #[test]
    fn options_resolve_into_the_config() {
        let ok = config(&[
            "--vector-tiles",
            "https://tiles.example/planet",
            "--map-style",
            "dark",
            "--information-position",
            "hidden",
            "--attribution-position",
            "Top  Left",
            "--no-tile-cache",
        ])
        .unwrap();
        assert_eq!(ok.kind, OutputKind::Png);
        assert_eq!(ok.info_anchor, None);
        assert_eq!(ok.attribution_anchor, Some(Anchor::TopLeft));
        assert_eq!(ok.tile_cache, None);
        assert!(ok.pre_draw_color.is_none());
        assert_eq!(
            ok.map,
            Some(MapSpec::Vector {
                url: "https://tiles.example/planet".into(),
                max_zoom: 14,
                style: "dark".into(),
                export_style: None,
            })
        );
        let cached = config(&["--tile-cache-dir", "/tmp/tiles"]).unwrap();
        assert_eq!(cached.tile_cache, Some(PathBuf::from("/tmp/tiles")));
    }
}
