//! The command line as clap parses it, with the per-value checks (ranges,
//! colors, overlay positions) that need no other option.

use std::ffi::OsStr;
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::{EnumValueParser, PossibleValue, TypedValueParser};
use clap::{Arg, Command, Parser, ValueEnum};
use gpx_motion_core::geo::mercator::MAX_ZOOM;
use gpx_motion_core::{Anchor, LabelStyle, MAX_PITCH_DEG};
use tiny_skia::Color;

use crate::color::parse_hex_color;

/// The release version: `GPX_MOTION_VERSION` at build time (the release
/// workflow sets it from the tag), else the crate's own.
const VERSION: &str = match option_env!("GPX_MOTION_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Largest `--viewport-width`/`--viewport-height`, in device pixels.
pub const MAX_VIEWPORT: u32 = 16384;

/// Narrowest `--fov`, as the camera clamps it.
pub const MIN_FOV_DEG: f64 = 10.0;

/// Widest `--fov`, as the camera clamps it.
pub const MAX_FOV_DEG: f64 = 120.0;

/// Smallest `--pixel-ratio`.
pub const MIN_PIXEL_RATIO: f32 = 0.25;

/// Largest `--pixel-ratio`.
pub const MAX_PIXEL_RATIO: f32 = 8.0;

/// Render one or more GPX tracks to a PNG (static) or an MP4 animation.
///
/// The output extension selects the mode: `.mp4` animates, anything else writes
/// a single PNG of the full polyline(s).
#[derive(Parser, Debug)]
#[command(name = "gpx-motion", version = VERSION)]
pub struct Args {
    /// Input GPX file(s); repeat or pass several to overlay multiple tracks.
    #[arg(short, long, required = true, num_args = 1..)]
    pub input: Vec<PathBuf>,

    /// Output file (`.mp4` animates, otherwise PNG).
    #[arg(short, long, default_value = "track.png")]
    pub output: PathBuf,

    /// Viewport width in pixels, `1..=16384`.
    #[arg(long, default_value_t = 640, value_parser = clap::value_parser!(u32).range(1..=MAX_VIEWPORT as i64))]
    pub viewport_width: u32,

    /// Viewport height in pixels, `1..=16384`.
    #[arg(long, default_value_t = 640, value_parser = clap::value_parser!(u32).range(1..=MAX_VIEWPORT as i64))]
    pub viewport_height: u32,

    /// Frames per second (MP4 only, positive).
    #[arg(long, default_value_t = 30.0, value_parser = positive)]
    pub fps: f64,

    /// Total video length in ms (positive). Defaults to the longest track's
    /// real duration.
    #[arg(long, value_parser = positive)]
    pub total_time: Option<f64>,

    /// Tail length in ms (MP4 only). The tail fades toward its start.
    /// Defaults to 5000 in 2D; the 3D view draws all progress so far unless
    /// this is set.
    #[arg(long)]
    pub tail_duration: Option<f64>,

    /// TMS/XYZ tile URL template, e.g.
    /// `https://tile.openstreetmap.org/{zoom}/{x}/{y}.png`. Omit for a solid
    /// background. Respect each tile provider's usage policy.
    #[arg(long, conflicts_with = "vector_tiles")]
    pub tms_url_template: Option<String>,

    /// Vector (MVT, OpenMapTiles schema) tiles rendered locally without labels:
    /// a TileJSON URL such as `https://tiles.openfreemap.org/planet`, an XYZ
    /// template ending in `{z}/{x}/{y}.pbf`, or a local `.pmtiles` archive
    /// (PMTiles v3, uncompressed or gzip tiles) for offline renders. The
    /// styles expect the OpenMapTiles schema (e.g. an archive built with
    /// planetiler); Protomaps-schema archives render nearly empty.
    #[arg(long)]
    pub vector_tiles: Option<String>,

    /// Terrarium elevation tiles (XYZ PNG template) for ground relief in the
    /// 3D view, e.g. the open AWS Terrain Tiles at
    /// `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png`.
    /// The map is draped over the terrain around the route. The default
    /// credit names Mapzen; set `--attribution` for other sources.
    #[arg(long)]
    pub terrain_tiles: Option<String>,

    /// Highest zoom of an XYZ `--vector-tiles` template (TileJSON supplies its own).
    #[arg(long, default_value_t = 14)]
    pub vector_max_zoom: u32,

    /// Vector map style: `light`, `dark`, or the path or `http(s)` URL of a
    /// JSON style: gpx-motion's own format (see `styles/`) or a MapLibre
    /// style for OpenMapTiles data, whose fill and line layers are imported
    /// (skipped parts are listed as warnings). Preset names win; pass
    /// `./dark` for a file named `dark`.
    #[arg(long, default_value = "light")]
    pub map_style: String,

    /// Write the resolved `--map-style` as a gpx-motion style JSON file
    /// (e.g. to see or edit what a MapLibre style became), then render as
    /// usual.
    #[arg(long, requires = "vector_tiles")]
    pub export_style: Option<PathBuf>,

    /// Background map opacity, `0.0..=1.0`.
    #[arg(long, default_value_t = 1.0)]
    pub background_map_visibility: f32,

    /// Draw the full route faintly under the animation (MP4 only).
    #[arg(long)]
    pub pre_draw_track: bool,

    /// Pre-drawn route color: `#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA`.
    #[arg(long, default_value = "#808080", value_parser = parse_hex_color)]
    pub pre_draw_track_color: Color,

    /// Fixed zoom level. When set, the camera follows the first track's marker
    /// instead of fitting the whole route into the viewport. May be
    /// fractional (`0..=22`). Given in CSS pixels: `--pixel-ratio 2` renders
    /// one level deeper so the view matches.
    #[arg(long, value_parser = in_range(0.0, MAX_ZOOM))]
    pub zoom: Option<f64>,

    /// Seconds to zoom from the whole route into `--zoom` at the start of a
    /// video, and back out at the end (eased). Needs `--zoom`. A video
    /// shorter than twice this never reaches the full zoom.
    #[arg(long, value_parser = positive_seconds)]
    pub zoom_transition: Option<f64>,

    /// Info overlay (lat/lon/speed/time) corner, or `hidden`.
    #[arg(long, default_value = "bottom left", value_parser = PositionParser)]
    pub information_position: Position,

    /// Attribution text drawn over the map. Defaults to the OpenStreetMap
    /// credit, plus OpenMapTiles for `--vector-tiles`.
    #[arg(long)]
    pub attribution: Option<String>,

    /// Attribution corner, or `hidden`.
    #[arg(long, default_value = "bottom right", value_parser = PositionParser)]
    pub attribution_position: Position,

    /// Color the track by elevation (low blue to high red) instead of a
    /// solid per-track color.
    #[arg(long)]
    pub color_by_elevation: bool,

    /// Draw a distance–elevation chart of the first track along the bottom
    /// edge, with the current position marked (MP4 only).
    #[arg(long)]
    pub elevation_profile: bool,

    /// Tilt the camera this many degrees from top-down (`0..=75`) for a 3D
    /// view: the track is raised by its elevation over a flat map, with a
    /// curtain down to the ground. `0` keeps the flat 2D view. The map must
    /// come from `--vector-tiles` (or be omitted).
    #[arg(long, default_value_t = 0.0, value_parser = in_range(0.0, MAX_PITCH_DEG))]
    pub pitch: f64,

    /// Compass direction (degrees) at the top of the screen in the 3D view.
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    pub bearing: f64,

    /// Degrees the 3D camera turns around the route over the video.
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    pub orbit: f64,

    /// Turn the 3D camera with the first track's direction of travel, so the
    /// route ahead points up the screen. `--bearing` becomes an offset from
    /// the heading (e.g. `180` looks back). Can't be combined with `--orbit`.
    #[arg(long)]
    pub follow_heading: bool,

    /// Meters of track the heading is averaged over. Defaults to the distance
    /// covered in about two seconds of video (at least 150 m), so fast
    /// playback doesn't spin the camera at every corner. Long routes in
    /// short videos still turn faster than the camera can follow; raise
    /// `--max-turn-rate` or lengthen the video.
    #[arg(long)]
    pub heading_window: Option<f64>,

    /// Fastest the heading-following camera may turn, in degrees per second
    /// of video (must be positive).
    #[arg(long, default_value_t = 90.0, value_parser = positive)]
    pub max_turn_rate: f64,

    /// Vertical field of view of the 3D camera, in degrees (`10..=120`).
    #[arg(long, default_value_t = 40.0, value_parser = in_range(MIN_FOV_DEG, MAX_FOV_DEG))]
    pub fov: f64,

    /// Height multiplier for the 3D view. Defaults to an automatic factor
    /// (1–6×) that keeps the climb readable.
    #[arg(long)]
    pub elevation_exaggeration: Option<f64>,

    /// Window (meters of track) for smoothing recorded elevations before
    /// coloring, the profile, climb totals, and 3D heights. `0` disables it.
    #[arg(long, default_value_t = 100.0)]
    pub elevation_smoothing: f64,

    /// Draw place, peak, and water names, and road and river names along
    /// their lines (minor roads only up close), from the vector map (OpenMapTiles
    /// schema), most important first and clear of the track and overlays.
    /// Needs a font for the labels' script: `--label-font`, or a system
    /// Korean/CJK font when one is found.
    #[arg(long)]
    pub labels: bool,

    /// Font file (TrueType/OpenType, or the first face of a `.ttc`) for
    /// `--labels`.
    #[arg(long, requires = "labels")]
    pub label_font: Option<PathBuf>,

    /// Show `name:<code>` (e.g. `ko`, `en`) when a place has one, else its
    /// local name.
    #[arg(long, requires = "labels")]
    pub label_language: Option<String>,

    /// Label text size in CSS pixels (cities draw larger, neighbourhoods
    /// smaller), `4..=96`.
    #[arg(
        long,
        default_value_t = LabelStyle::CLI_DEFAULT_SIZE,
        value_parser = in_range(LabelStyle::MIN_SIZE, LabelStyle::MAX_SIZE),
    )]
    pub label_size: f32,

    /// Device pixels per CSS pixel. Scales line widths, the marker, text, and
    /// padding so a larger output (e.g. `2` for a 2× viewport) looks the same,
    /// only sharper. Map line widths also follow the map zoom. Raster map
    /// tiles are drawn as fetched, so their labels and roads don't scale.
    /// `0.25..=8`.
    #[arg(long, default_value_t = 1.0, value_parser = in_range(MIN_PIXEL_RATIO, MAX_PIXEL_RATIO))]
    pub pixel_ratio: f32,

    /// Worker threads for parallel frame rendering. Defaults to all cores.
    #[arg(long)]
    pub jobs: Option<usize>,

    /// Disable the on-disk tile cache (always fetch over the network).
    #[arg(long)]
    pub no_tile_cache: bool,

    /// On-disk tile cache directory. Defaults to `$HOME/.cache/gpx-motion/tiles`.
    #[arg(long)]
    pub tile_cache_dir: Option<PathBuf>,
}

/// Where an overlay goes on the frame: a corner, or nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Position {
    #[value(name = "top left")]
    TopLeft,
    #[value(name = "top right")]
    TopRight,
    #[value(name = "bottom left")]
    BottomLeft,
    #[value(name = "bottom right")]
    BottomRight,
    Hidden,
}

impl Position {
    /// The corner to draw at, or `None` for [`Position::Hidden`].
    pub fn anchor(self) -> Option<Anchor> {
        match self {
            Position::TopLeft => Some(Anchor::TopLeft),
            Position::TopRight => Some(Anchor::TopRight),
            Position::BottomLeft => Some(Anchor::BottomLeft),
            Position::BottomRight => Some(Anchor::BottomRight),
            Position::Hidden => None,
        }
    }
}

/// Parses a [`Position`] ignoring case and extra whitespace (so
/// `"Bottom  Left"` is `bottom left`); an empty value is `hidden`.
#[derive(Debug, Clone, Copy)]
pub struct PositionParser;

impl TypedValueParser for PositionParser {
    type Value = Position;

    fn parse_ref(
        &self,
        cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Position, clap::Error> {
        let normalized = value
            .to_string_lossy()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let normalized = if normalized.is_empty() {
            "hidden".to_string()
        } else {
            normalized
        };
        EnumValueParser::<Position>::new().parse_ref(cmd, arg, OsStr::new(&normalized))
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(Box::new(
            Position::value_variants()
                .iter()
                .filter_map(ValueEnum::to_possible_value),
        ))
    }
}

/// A value parser accepting numbers in `min..=max`.
pub fn in_range<T>(min: T, max: T) -> impl Fn(&str) -> Result<T, String> + Clone + Send + Sync
where
    T: FromStr + PartialOrd + Display + Copy + Send + Sync + 'static,
    T::Err: Display,
{
    move |s| {
        let value: T = s.parse().map_err(|e: T::Err| e.to_string())?;
        if (min..=max).contains(&value) {
            Ok(value)
        } else {
            Err(format!("must be between {min} and {max}"))
        }
    }
}

/// A value parser accepting finite numbers above zero.
pub fn positive(s: &str) -> Result<f64, String> {
    let value: f64 = s.parse().map_err(|e| format!("{e}"))?;
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err("must be positive".to_string())
    }
}

/// [`positive`], for a span of seconds.
pub fn positive_seconds(s: &str) -> Result<f64, String> {
    positive(s).map_err(|_| "must be a positive number of seconds".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &[&str]) -> Result<Args, clap::Error> {
        let base = ["gpx-motion", "-i", "a.gpx"];
        Args::try_parse_from(base.iter().chain(extra))
    }

    #[test]
    fn positions_accept_every_spelling() {
        let cases = [
            ("top left", Position::TopLeft),
            ("top right", Position::TopRight),
            ("bottom left", Position::BottomLeft),
            ("bottom right", Position::BottomRight),
            ("hidden", Position::Hidden),
            ("Bottom  Left", Position::BottomLeft),
            ("  TOP RIGHT ", Position::TopRight),
            ("", Position::Hidden),
        ];
        for (text, expected) in cases {
            let args = parse(&[
                "--information-position",
                text,
                "--attribution-position",
                text,
            ])
            .unwrap_or_else(|e| panic!("{text:?}: {e}"));
            assert_eq!(args.information_position, expected, "{text:?}");
            assert_eq!(args.attribution_position, expected, "{text:?}");
        }
        let args = parse(&[]).unwrap();
        assert_eq!(args.information_position, Position::BottomLeft);
        assert_eq!(args.attribution_position, Position::BottomRight);
    }

    #[test]
    fn positions_reject_unknown_corners() {
        for flag in ["--information-position", "--attribution-position"] {
            let err = parse(&[flag, "middle"]).unwrap_err().to_string();
            assert!(err.contains("possible values"), "{err}");
        }
    }

    #[test]
    fn numbers_outside_their_range_are_rejected() {
        let cases: &[&[&str]] = &[
            &["--fps", "0"],
            &["--fps", "-1"],
            &["--fps", "inf"],
            &["--fps", "NaN"],
            &["--fov", "5"],
            &["--fov", "121"],
            &["--viewport-width", "0"],
            &["--viewport-height", "16385"],
            &["--pixel-ratio", "0.2"],
            &["--pixel-ratio", "9"],
            &["--label-size", "3"],
            &["--label-size", "97"],
            &["--zoom", "22.5"],
            &["--zoom", "-1"],
            &["--pitch", "76"],
            &["--pitch", "-1"],
            &["--zoom-transition", "0"],
            &["--max-turn-rate", "0"],
            &["--max-turn-rate", "-5"],
            &["--total-time", "0"],
            &["--pre-draw-track-color", "#xyz"],
        ];
        for case in cases {
            assert!(parse(case).is_err(), "{case:?} was accepted");
        }
    }

    #[test]
    fn numbers_at_their_bounds_are_accepted() {
        let cases: &[&[&str]] = &[
            &["--fov", "10", "--pitch", "75", "--zoom", "22"],
            &["--fov", "120", "--pitch", "0", "--zoom", "0"],
            &["--viewport-width", "1", "--viewport-height", "16384"],
            &["--pixel-ratio", "0.25", "--label-size", "4"],
            &["--pixel-ratio", "8", "--label-size", "96"],
            &[
                "--fps",
                "0.5",
                "--max-turn-rate",
                "1",
                "--zoom-transition",
                "2",
            ],
        ];
        for case in cases {
            if let Err(e) = parse(case) {
                panic!("{case:?}: {e}");
            }
        }
    }

    #[test]
    fn range_errors_name_the_bounds() {
        let err = parse(&["--zoom", "30"]).unwrap_err().to_string();
        assert!(err.contains("must be between 0 and 22"), "{err}");
        let err = parse(&["--label-size", "200"]).unwrap_err().to_string();
        assert!(err.contains("must be between 4 and 96"), "{err}");
        let err = parse(&["--zoom-transition", "-"]).unwrap_err().to_string();
        assert!(err.contains("positive number of seconds"), "{err}");
    }
}
