mod color;
mod encode;
mod perspective;
mod terrain;
mod tiles;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use gpx_motion_core::geo::mercator::{Projection, MAX_ZOOM};
use gpx_motion_core::{
    draw_block, draw_overlay, draw_profile, parse, stroke_track, stroke_track_by_elevation, Anchor,
    FrameStyle, Lens, MapStyle, OverlayStyle, ProfileStyle, Relief, Terrain, Timeline, TrackPoint,
    TrackStyle,
};
use tiny_skia::{Color, Pixmap};

use rayon::prelude::*;

use color::parse_hex_color;
use encode::FfmpegEncoder;
use perspective::{relief_for, Ground, HeadingFollow, Perspective, Scene};
use tiles::{MapSource, TileFetcher, VectorTileFetcher};

/// Distinct tail/marker colors cycled across multiple input tracks.
const PALETTE: [(u8, u8, u8); 6] = [
    (0xff, 0x5a, 0x36),
    (0x36, 0x9a, 0xff),
    (0x3a, 0xd1, 0x7a),
    (0xff, 0xd1, 0x36),
    (0xc8, 0x5a, 0xff),
    (0xff, 0x36, 0x9a),
];

/// Render one or more GPX tracks to a PNG (static) or an MP4 animation.
///
/// The output extension selects the mode: `.mp4` animates, anything else writes
/// a single PNG of the full polyline(s).
#[derive(Parser, Debug)]
#[command(name = "gpx-motion", version)]
struct Args {
    /// Input GPX file(s); repeat or pass several to overlay multiple tracks.
    #[arg(short, long, required = true, num_args = 1..)]
    input: Vec<PathBuf>,

    /// Output file (`.mp4` animates, otherwise PNG).
    #[arg(short, long, default_value = "track.png")]
    output: PathBuf,

    #[arg(long, default_value_t = 640)]
    viewport_width: u32,

    #[arg(long, default_value_t = 640)]
    viewport_height: u32,

    /// Frames per second (MP4 only).
    #[arg(long, default_value_t = 30.0)]
    fps: f64,

    /// Total video length in ms. Defaults to the longest track's real duration.
    #[arg(long)]
    total_time: Option<f64>,

    /// Tail length in ms (MP4 only). The tail fades toward its start.
    /// Defaults to 5000 in 2D; the 3D view draws all progress so far unless
    /// this is set.
    #[arg(long)]
    tail_duration: Option<f64>,

    /// TMS/XYZ tile URL template, e.g.
    /// `https://tile.openstreetmap.org/{zoom}/{x}/{y}.png`. Omit for a solid
    /// background. Respect each tile provider's usage policy.
    #[arg(long, conflicts_with = "vector_tiles")]
    tms_url_template: Option<String>,

    /// Vector (MVT, OpenMapTiles schema) tiles rendered locally without labels:
    /// a TileJSON URL such as `https://tiles.openfreemap.org/planet`, or an XYZ
    /// template ending in `{z}/{x}/{y}.pbf`.
    #[arg(long)]
    vector_tiles: Option<String>,

    /// Terrarium elevation tiles (XYZ PNG template) for ground relief in the
    /// 3D view, e.g. the open AWS Terrain Tiles at
    /// `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png`.
    /// The map is draped over the terrain around the route. The default
    /// credit names Mapzen; set `--attribution` for other sources.
    #[arg(long)]
    terrain_tiles: Option<String>,

    /// Highest zoom of an XYZ `--vector-tiles` template (TileJSON supplies its own).
    #[arg(long, default_value_t = 14)]
    vector_max_zoom: u32,

    /// Vector map style: `light` or `dark`.
    #[arg(long, default_value = "light")]
    map_style: String,

    /// Background map opacity, `0.0..=1.0`.
    #[arg(long, default_value_t = 1.0)]
    background_map_visibility: f32,

    /// Draw the full route faintly under the animation.
    #[arg(long)]
    pre_draw_track: bool,

    /// Pre-drawn route color.
    #[arg(long, default_value = "#808080")]
    pre_draw_track_color: String,

    /// Fixed zoom level. When set, the camera follows the first track's marker
    /// instead of fitting the whole route into the viewport. May be
    /// fractional (`0..=22`). Given in CSS pixels: `--pixel-ratio 2` renders
    /// one level deeper so the view matches.
    #[arg(long)]
    zoom: Option<f64>,

    /// Info overlay (lat/lon/speed/time) corner, or `hidden`. One of:
    /// `top left`, `top right`, `bottom left`, `bottom right`, `hidden`.
    #[arg(long, default_value = "bottom left")]
    information_position: String,

    /// Attribution text drawn over the map. Defaults to the OpenStreetMap
    /// credit, plus OpenMapTiles for `--vector-tiles`.
    #[arg(long)]
    attribution: Option<String>,

    /// Attribution corner, or `hidden`.
    #[arg(long, default_value = "bottom right")]
    attribution_position: String,

    /// Color the track by elevation (low blue to high red) instead of a
    /// solid per-track color.
    #[arg(long)]
    color_by_elevation: bool,

    /// Draw a distance–elevation chart of the first track along the bottom
    /// edge, with the current position marked (MP4 only).
    #[arg(long)]
    elevation_profile: bool,

    /// Tilt the camera this many degrees from top-down (`0..=75`) for a 3D
    /// view: the track is raised by its elevation over a flat map, with a
    /// curtain down to the ground. `0` keeps the flat 2D view. The map must
    /// come from `--vector-tiles` (or be omitted).
    #[arg(long, default_value_t = 0.0)]
    pitch: f64,

    /// Compass direction (degrees) at the top of the screen in the 3D view.
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    bearing: f64,

    /// Degrees the 3D camera turns around the route over the video.
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    orbit: f64,

    /// Turn the 3D camera with the first track's direction of travel, so the
    /// route ahead points up the screen. `--bearing` becomes an offset from
    /// the heading (e.g. `180` looks back). Can't be combined with `--orbit`.
    #[arg(long)]
    follow_heading: bool,

    /// Meters of track the heading is averaged over. Defaults to the distance
    /// covered in about two seconds of video (at least 150 m), so fast
    /// playback doesn't spin the camera at every corner. Long routes in
    /// short videos still turn faster than the camera can follow; raise
    /// `--max-turn-rate` or lengthen the video.
    #[arg(long)]
    heading_window: Option<f64>,

    /// Fastest the heading-following camera may turn, in degrees per second
    /// of video (must be positive).
    #[arg(long, default_value_t = 90.0)]
    max_turn_rate: f64,

    /// Vertical field of view of the 3D camera, in degrees.
    #[arg(long, default_value_t = 40.0)]
    fov: f64,

    /// Height multiplier for the 3D view. Defaults to an automatic factor
    /// (1–6×) that keeps the climb readable.
    #[arg(long)]
    elevation_exaggeration: Option<f64>,

    /// Window (meters of track) for smoothing recorded elevations before
    /// coloring, the profile, climb totals, and 3D heights. `0` disables it.
    #[arg(long, default_value_t = 100.0)]
    elevation_smoothing: f64,

    /// Device pixels per CSS pixel. Scales line widths, the marker, text, and
    /// padding so a larger output (e.g. `2` for a 2× viewport) looks the same,
    /// only sharper. Map line widths also follow the map zoom. Raster map
    /// tiles are drawn as fetched, so their labels and roads don't scale.
    /// `0.25..=8`.
    #[arg(long, default_value_t = 1.0)]
    pixel_ratio: f32,

    /// Worker threads for parallel frame rendering. Defaults to all cores.
    #[arg(long)]
    jobs: Option<usize>,

    /// Disable the on-disk tile cache (always fetch over the network).
    #[arg(long)]
    no_tile_cache: bool,

    /// On-disk tile cache directory. Defaults to `$HOME/.cache/gpx-motion/tiles`.
    #[arg(long)]
    tile_cache_dir: Option<PathBuf>,
}

/// Shared, immutable per-frame render parameters (safe to share across threads).
struct FrameParams<'a> {
    tracks: &'a [Track],
    styles: &'a [FrameStyle],
    tail_us: &'a [f64],
    video_ms: f64,
    info_anchor: Option<Anchor>,
    attribution_anchor: Option<Anchor>,
    attribution: &'a str,
    overlay_style: &'a OverlayStyle,
    profile: Option<ProfileStyle>,
}

impl FrameParams<'_> {
    /// Draw the animated tracks and overlays for progress `u` onto a background.
    fn draw(&self, pixmap: &mut Pixmap, proj: &Projection, u: f64) {
        for (i, track) in self.tracks.iter().enumerate() {
            draw_overlay(
                pixmap,
                &track.timeline,
                proj,
                u,
                self.tail_us[i],
                &self.styles[i],
            );
        }
        self.draw_hud(pixmap, u);
    }

    /// Draw the screen-space overlays: elevation profile, info, attribution.
    fn draw_hud(&self, pixmap: &mut Pixmap, u: f64) {
        if let Some(profile) = &self.profile {
            draw_profile(
                pixmap,
                &self.tracks[0].timeline,
                u,
                profile,
                &self.styles[0],
            );
        }
        if let Some(anchor) = self.info_anchor {
            let lines = info_lines(&self.tracks[0], u, self.video_ms);
            draw_block(pixmap, &lines, anchor, self.overlay_style);
        }
        if let Some(anchor) = self.attribution_anchor {
            if !self.attribution.is_empty() {
                let line = self.attribution.to_string();
                draw_block(
                    pixmap,
                    std::slice::from_ref(&line),
                    anchor,
                    self.overlay_style,
                );
            }
        }
    }
}

fn progress(frame: u64, total_frames: u64) -> f64 {
    if total_frames > 1 {
        frame as f64 / (total_frames - 1) as f64
    } else {
        1.0
    }
}

/// Resolve the on-disk tile cache directory, honoring `--no-tile-cache` and
/// `--tile-cache-dir`.
fn tile_cache_dir(args: &Args) -> Option<PathBuf> {
    if args.no_tile_cache {
        return None;
    }
    if let Some(dir) = &args.tile_cache_dir {
        return Some(dir.clone());
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/gpx-motion/tiles"))
}

/// Build the background map source from `--tms-url-template` or `--vector-tiles`.
fn map_source(args: &Args) -> Result<Option<MapSource>, Box<dyn std::error::Error>> {
    if let Some(template) = &args.tms_url_template {
        let fetcher = TileFetcher::new(template.clone(), tile_cache_dir(args))?;
        return Ok(Some(MapSource::Raster(fetcher)));
    }
    let Some(url) = &args.vector_tiles else {
        return Ok(None);
    };
    let style = MapStyle::preset(&args.map_style)
        .ok_or_else(|| format!("unknown map style `{}` (light, dark)", args.map_style))?
        .with_pixel_ratio(args.pixel_ratio);
    let tiles = VectorTileFetcher::new(url, args.vector_max_zoom, tile_cache_dir(args))?;
    Ok(Some(MapSource::Vector { tiles, style }))
}

/// The attribution line: `--attribution` if given, else the credit matching
/// the map source (plus the terrain data, when used).
fn attribution(args: &Args) -> String {
    if let Some(text) = &args.attribution {
        return text.clone();
    }
    let has_map = args.vector_tiles.is_some() || args.tms_url_template.is_some();
    match (args.vector_tiles.is_some(), args.terrain_tiles.is_some()) {
        (true, false) => "© OpenMapTiles © OpenStreetMap contributors".to_string(),
        (false, false) => "© OpenStreetMap contributors".to_string(),
        (true, true) => "© OpenMapTiles © OSM · terrain © Mapzen".to_string(),
        (false, true) if has_map => "© OSM contributors · terrain © Mapzen".to_string(),
        (false, true) => "terrain © Mapzen".to_string(),
    }
}

/// Terrain under the route when `--terrain-tiles` is set.
fn terrain_of(
    args: &Args,
    points: &[TrackPoint],
    relief: &Relief,
    zoom: f64,
    map: Option<&mut MapSource>,
    background: Color,
) -> Result<Option<Terrain>, Box<dyn std::error::Error>> {
    let Some(template) = &args.terrain_tiles else {
        return Ok(None);
    };
    terrain::build(
        template,
        tile_cache_dir(args),
        points,
        relief,
        zoom,
        map,
        terrain::Backdrop {
            color: background,
            map_opacity: args.background_map_visibility,
        },
    )
}

/// `--zoom` in device pixels: one level deeper per doubling of
/// `--pixel-ratio`.
fn fixed_zoom(args: &Args) -> Option<f64> {
    let levels = (args.pixel_ratio as f64).log2();
    args.zoom.map(|z| (z + levels).clamp(0.0, MAX_ZOOM))
}

/// 2D tail length when `--tail-duration` is unset.
const DEFAULT_TAIL_MS: f64 = 5000.0;

/// Distance the heading is averaged over when `--heading-window` is unset.
const MIN_HEADING_WINDOW_M: f64 = 150.0;
/// Seconds of video whose travel the default heading window spans.
const HEADING_WINDOW_S: f64 = 2.0;

/// The 3D camera settings from the command line. `video_s` (absent for a
/// still image) sizes the default heading window to the playback speed.
fn perspective_of(args: &Args, lead: &Track, video_s: Option<f64>) -> Result<Perspective, String> {
    let heading = if args.follow_heading {
        if args.orbit != 0.0 {
            return Err("--follow-heading and --orbit both turn the camera; pick one".into());
        }
        if args.max_turn_rate.is_nan() || args.max_turn_rate <= 0.0 {
            return Err("--max-turn-rate must be positive".into());
        }
        let fps = video_s.map_or(1.0, |_| args.fps);
        let window_m = args.heading_window.unwrap_or_else(|| {
            let per_second = video_s.map_or(0.0, |s| lead.timeline.total_distance_m() / s);
            (per_second * HEADING_WINDOW_S).max(MIN_HEADING_WINDOW_M)
        });
        Some(HeadingFollow {
            window_m,
            max_turn_deg: args.max_turn_rate / fps,
        })
    } else {
        None
    };
    Ok(Perspective {
        lens: Lens {
            pitch_deg: args.pitch,
            bearing_deg: args.bearing,
            fov_deg: args.fov,
        },
        orbit_deg: args.orbit,
        follow_zoom: fixed_zoom(args),
        heading,
    })
}

/// Parse an overlay position; `hidden` (or empty) yields `None`.
fn parse_anchor(s: &str) -> Result<Option<Anchor>, String> {
    let normalized: String = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    match normalized.as_str() {
        "hidden" | "" => Ok(None),
        "top left" => Ok(Some(Anchor::TopLeft)),
        "top right" => Ok(Some(Anchor::TopRight)),
        "bottom left" => Ok(Some(Anchor::BottomLeft)),
        "bottom right" => Ok(Some(Anchor::BottomRight)),
        other => Err(format!("invalid position: {other}")),
    }
}

/// Build the info-overlay lines for the lead track at progress `u`.
fn info_lines(track: &Track, u: f64, video_ms: f64) -> Vec<String> {
    let (lat, lon) = track.timeline.position_at(u);
    let mut lines = vec![format!("lat {lat:.5}"), format!("lon {lon:.5}")];
    if let Some(kmh) = track.timeline.speed_kmh_at(u) {
        lines.push(format!("speed {kmh:.1} km/h"));
    }
    if let (Some(ele), Some(elevation)) =
        (track.timeline.elevation_at(u), track.timeline.elevation())
    {
        let (i, _) = track.timeline.locate(u);
        let (gain, loss) = elevation.gain_loss_at(i);
        lines.push(format!("ele {ele:.0} m"));
        lines.push(format!("climb +{gain:.0} / -{loss:.0} m"));
    }
    let elapsed_ms = u * track.timeline.duration_ms.unwrap_or(video_ms);
    lines.push(format!("time {}", format_elapsed(elapsed_ms)));
    lines
}

fn format_elapsed(ms: f64) -> String {
    let total = (ms / 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

/// A parsed track with its interpolation timeline and palette color.
struct Track {
    points: Vec<TrackPoint>,
    timeline: Timeline,
    color: Color,
}

fn main() -> ExitCode {
    let args = Args::parse();

    if let Some(jobs) = args.jobs {
        if let Err(e) = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs.max(1))
            .build_global()
        {
            eprintln!("error: could not configure {jobs} threads: {e}");
            return ExitCode::FAILURE;
        }
    }

    if args.zoom.is_some_and(|z| !(0.0..=MAX_ZOOM).contains(&z)) {
        eprintln!("error: --zoom must be between 0 and {MAX_ZOOM}");
        return ExitCode::FAILURE;
    }
    if !(0.25..=8.0).contains(&args.pixel_ratio) {
        eprintln!("error: --pixel-ratio must be between 0.25 and 8");
        return ExitCode::FAILURE;
    }

    let result = if is_video(&args.output) {
        run_video(&args)
    } else {
        run_png(&args)
    };

    match result {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
}

fn load_tracks(
    inputs: &[PathBuf],
    elevation_smoothing: f64,
) -> Result<Vec<Track>, Box<dyn std::error::Error>> {
    let mut tracks = Vec::new();
    for (i, path) in inputs.iter().enumerate() {
        let points = parse(&std::fs::read_to_string(path)?)?;
        let mut timeline = Timeline::build(&points)
            .ok_or_else(|| format!("{}: needs at least two distinct points", path.display()))?;
        timeline.set_elevation_smoothing(elevation_smoothing);
        let (r, g, b) = PALETTE[i % PALETTE.len()];
        tracks.push(Track {
            points,
            timeline,
            color: Color::from_rgba8(r, g, b, 0xff),
        });
    }
    Ok(tracks)
}

fn all_points(tracks: &[Track]) -> Vec<TrackPoint> {
    tracks.iter().flat_map(|t| t.points.clone()).collect()
}

fn run_png(args: &Args) -> Result<String, Box<dyn std::error::Error>> {
    let tracks = load_tracks(&args.input, args.elevation_smoothing)?;
    let points = all_points(&tracks);
    let style = TrackStyle {
        width: args.viewport_width,
        height: args.viewport_height,
        ..Default::default()
    }
    .scaled(args.pixel_ratio);
    let base_style = FrameStyle {
        width: style.width,
        height: style.height,
        background: style.background,
        ..Default::default()
    }
    .scaled(args.pixel_ratio);
    let mut map = map_source(args)?;

    if args.follow_heading && args.pitch <= 0.0 {
        return Err("--follow-heading needs the 3D view (--pitch > 0)".into());
    }
    if args.terrain_tiles.is_some() && args.pitch <= 0.0 {
        return Err("--terrain-tiles needs the 3D view (--pitch > 0)".into());
    }
    if args.pitch > 0.0 {
        let perspective = perspective_of(args, &tracks[0], None)?;
        if let Some(map) = &map {
            map.check_3d()?;
        }
        let relief = relief_for(
            &tracks,
            args.elevation_exaggeration,
            args.terrain_tiles.is_some(),
        );
        let mut framing = perspective
            .framing(
                &tracks,
                &relief,
                style.width,
                style.height,
                style.padding,
                0.0,
            )
            .ok_or("could not fit track to viewport")?;
        let bearings = perspective.bearings(&tracks[0], &[0.0]);
        framing.cover(&bearings);
        let terrain = terrain_of(
            args,
            &points,
            &relief,
            framing.zoom(),
            map.as_mut(),
            style.background,
        )?;
        let ground = terrain.as_ref().map(|terrain| Ground { terrain, relief });
        let camera = perspective.cameras(&framing, &tracks[0], &[0.0], &bearings, ground)[0];
        if let Some(map) = map.as_mut() {
            map.prefetch_3d(&camera)?;
        }
        let styles: Vec<FrameStyle> = tracks
            .iter()
            .map(|t| {
                FrameStyle {
                    width: style.width,
                    height: style.height,
                    tail_color: t.color,
                    color_by_elevation: args.color_by_elevation,
                    ..Default::default()
                }
                .scaled(args.pixel_ratio)
            })
            .collect();
        let scene = Scene {
            map: map.as_ref(),
            map_opacity: args.background_map_visibility,
            background: style.background,
            tracks: &tracks,
            styles: &styles,
            tail_us: &[],
            relief,
            terrain: terrain.as_ref(),
        };
        let mut pixmap = scene.render(&camera, 1.0).ok_or("invalid viewport size")?;
        draw_attribution(&mut pixmap, args, map.is_some() || terrain.is_some())?;
        pixmap.save_png(&args.output)?;
        return Ok(format!(
            "Rendered {} track(s) in 3D, {} points -> {}",
            tracks.len(),
            points.len(),
            args.output.display()
        ));
    }

    let mut pixmap = match Projection::fit(&points, style.width, style.height, style.padding) {
        Some(proj) => {
            let mut pixmap = build_base(args, &proj, &base_style, None, &tracks, map.as_mut())?;
            for track in &tracks {
                if args.color_by_elevation {
                    stroke_track_by_elevation(
                        &mut pixmap,
                        &track.timeline,
                        &proj,
                        track.color,
                        style.track_width,
                    );
                } else {
                    stroke_track(
                        &mut pixmap,
                        &track.points,
                        &proj,
                        track.color,
                        style.track_width,
                    );
                }
            }
            pixmap
        }
        None => {
            let mut pixmap =
                Pixmap::new(style.width, style.height).ok_or("invalid viewport size")?;
            pixmap.fill(style.background);
            pixmap
        }
    };
    draw_attribution(&mut pixmap, args, map.is_some())?;
    pixmap.save_png(&args.output)?;

    Ok(format!(
        "Rendered {} track(s), {} points -> {}",
        tracks.len(),
        points.len(),
        args.output.display()
    ))
}

/// Credit the map source on a still image (only when a map was drawn).
fn draw_attribution(
    pixmap: &mut Pixmap,
    args: &Args,
    has_map: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !has_map {
        return Ok(());
    }
    if let Some(anchor) = parse_anchor(&args.attribution_position)? {
        let line = attribution(args);
        if !line.is_empty() {
            draw_block(
                pixmap,
                std::slice::from_ref(&line),
                anchor,
                &OverlayStyle::default().scaled(args.pixel_ratio),
            );
        }
    }
    Ok(())
}

fn run_video(args: &Args) -> Result<String, Box<dyn std::error::Error>> {
    let tracks = load_tracks(&args.input, args.elevation_smoothing)?;
    let points = all_points(&tracks);

    let video_ms = args
        .total_time
        .or_else(|| {
            tracks
                .iter()
                .filter_map(|t| t.timeline.duration_ms)
                .fold(None, |acc, d| Some(f64::max(acc.unwrap_or(0.0), d)))
        })
        .ok_or("no track has timestamps; pass --total-time")?;
    if video_ms <= 0.0 {
        return Err("video duration must be positive".into());
    }

    let total_frames = ((video_ms / 1000.0) * args.fps).round().max(1.0) as u64;
    let pre_draw_color = if args.pre_draw_track {
        Some(parse_hex_color(&args.pre_draw_track_color)?)
    } else {
        None
    };
    let info_anchor = parse_anchor(&args.information_position)?;
    let attribution_anchor = parse_anchor(&args.attribution_position)?;
    let profile = args
        .elevation_profile
        .then(|| ProfileStyle::default().scaled(args.pixel_ratio));
    let reserved_bottom = profile.map_or(0.0, |p| p.reserved_height());
    let overlay_style = OverlayStyle {
        bottom_inset: reserved_bottom,
        ..OverlayStyle::default().scaled(args.pixel_ratio)
    };

    // Per-track styles carry the palette color; tail length is relative to each
    // track's own span (real time when timestamped, else the video duration).
    let styles: Vec<FrameStyle> = tracks
        .iter()
        .map(|t| {
            FrameStyle {
                width: args.viewport_width,
                height: args.viewport_height,
                tail_color: t.color,
                color_by_elevation: args.color_by_elevation,
                ..Default::default()
            }
            .scaled(args.pixel_ratio)
        })
        .collect();
    let tail_us: Vec<f64> = tracks
        .iter()
        .map(|t| {
            let reference_ms = t.timeline.duration_ms.unwrap_or(video_ms);
            match args.tail_duration {
                Some(ms) => (ms / reference_ms).clamp(0.0, 1.0),
                None if args.pitch > 0.0 => 1.0,
                None => (DEFAULT_TAIL_MS / reference_ms).clamp(0.0, 1.0),
            }
        })
        .collect();

    let base_style = styles.first().cloned().unwrap_or_default();
    let mut map = map_source(args)?;
    let attribution = attribution(args);

    let params = FrameParams {
        tracks: &tracks,
        styles: &styles,
        tail_us: &tail_us,
        video_ms,
        info_anchor,
        attribution_anchor,
        attribution: &attribution,
        overlay_style: &overlay_style,
        profile,
    };

    let mut encoder = FfmpegEncoder::start(
        &args.output,
        args.viewport_width,
        args.viewport_height,
        args.fps,
    )?;

    if args.follow_heading && args.pitch <= 0.0 {
        return Err("--follow-heading needs the 3D view (--pitch > 0)".into());
    }
    if args.terrain_tiles.is_some() && args.pitch <= 0.0 {
        return Err("--terrain-tiles needs the 3D view (--pitch > 0)".into());
    }
    if args.pitch > 0.0 {
        let perspective = perspective_of(args, &tracks[0], Some(video_ms / 1000.0))?;
        if let Some(map) = &map {
            map.check_3d()?;
        }
        let relief = relief_for(
            &tracks,
            args.elevation_exaggeration,
            args.terrain_tiles.is_some(),
        );
        let mut framing = perspective
            .framing(
                &tracks,
                &relief,
                args.viewport_width,
                args.viewport_height,
                base_style.padding,
                reserved_bottom as f64,
            )
            .ok_or("could not fit track to viewport")?;
        let us: Vec<f64> = (0..total_frames)
            .map(|f| progress(f, total_frames))
            .collect();
        let bearings = perspective.bearings(&tracks[0], &us);
        framing.cover(&bearings);
        let terrain = terrain_of(
            args,
            &points,
            &relief,
            framing.zoom(),
            map.as_mut(),
            base_style.background,
        )?;
        let ground = terrain.as_ref().map(|terrain| Ground { terrain, relief });
        let cameras = perspective.cameras(&framing, &tracks[0], &us, &bearings, ground);
        if let Some(map) = map.as_mut() {
            for camera in &cameras {
                map.prefetch_3d(camera)?;
            }
        }
        let scene = Scene {
            map: map.as_ref(),
            map_opacity: args.background_map_visibility,
            background: base_style.background,
            tracks: &tracks,
            styles: &styles,
            tail_us: &tail_us,
            relief,
            terrain: terrain.as_ref(),
        };
        render_parallel(&mut encoder, total_frames, args, |frame| {
            let u = progress(frame, total_frames);
            let mut pixmap = scene
                .render(&cameras[frame as usize], u)
                .expect("viewport validated by the encoder");
            params.draw_hud(&mut pixmap, u);
            pixmap
        })?;
        encoder.finish()?;
        return Ok(format!(
            "Encoded {total_frames} frames @ {}fps (3D), {} track(s) -> {} ({:.1}s)",
            args.fps,
            tracks.len(),
            args.output.display(),
            video_ms / 1000.0
        ));
    }

    match fixed_zoom(args) {
        // Static camera: fit the whole route, composite the background once, and
        // render frames in parallel (each is an independent clone + overlay).
        None => {
            let fit_height = args
                .viewport_height
                .saturating_sub(reserved_bottom as u32)
                .max(1);
            let proj =
                Projection::fit(&points, args.viewport_width, fit_height, base_style.padding)
                    .ok_or("could not fit track to viewport")?;
            let base = build_base(
                args,
                &proj,
                &base_style,
                pre_draw_color,
                &tracks,
                map.as_mut(),
            )?;
            render_parallel(&mut encoder, total_frames, args, |frame| {
                let mut pixmap = base.clone();
                params.draw(&mut pixmap, &proj, progress(frame, total_frames));
                pixmap
            })?;
        }
        // Follow camera: recenter on the lead marker every frame, so each frame
        // composites its own (cache-backed) tile background — kept sequential.
        Some(zoom) => {
            for frame in 0..total_frames {
                let u = progress(frame, total_frames);
                let (lat, lon) = tracks[0].timeline.position_at(u);
                let proj =
                    Projection::centered(lat, lon, zoom, args.viewport_width, args.viewport_height);
                let mut pixmap = build_base(
                    args,
                    &proj,
                    &base_style,
                    pre_draw_color,
                    &tracks,
                    map.as_mut(),
                )?;
                params.draw(&mut pixmap, &proj, u);
                encoder.write_frame(pixmap.data())?;
            }
        }
    }
    encoder.finish()?;

    Ok(format!(
        "Encoded {total_frames} frames @ {}fps, {} track(s) -> {} ({:.1}s)",
        args.fps,
        tracks.len(),
        args.output.display(),
        video_ms / 1000.0
    ))
}

/// Render frames in memory-bounded parallel chunks, writing each chunk to the
/// encoder in order.
///
/// Frames are independent, so a chunk renders across all rayon workers; the
/// encoder still receives frames sequentially. Chunk size targets a fixed
/// memory budget for the in-flight pixmaps regardless of viewport size.
fn render_parallel(
    encoder: &mut FfmpegEncoder,
    total_frames: u64,
    args: &Args,
    render: impl Fn(u64) -> Pixmap + Sync,
) -> Result<(), Box<dyn std::error::Error>> {
    const MEMORY_BUDGET: usize = 256 * 1024 * 1024;
    let frame_bytes = (args.viewport_width as usize) * (args.viewport_height as usize) * 4;
    let chunk = (MEMORY_BUDGET / frame_bytes.max(1)).clamp(1, 256) as u64;

    let mut start = 0;
    while start < total_frames {
        let end = (start + chunk).min(total_frames);
        let pixmaps: Vec<Pixmap> = (start..end).into_par_iter().map(&render).collect();
        for pixmap in &pixmaps {
            encoder.write_frame(pixmap.data())?;
        }
        start = end;
    }
    Ok(())
}

/// Build the frame background for a projection: solid fill, optional map
/// (raster or vector), and the optional pre-drawn full route.
fn build_base(
    args: &Args,
    proj: &Projection,
    style: &FrameStyle,
    pre_draw_color: Option<Color>,
    tracks: &[Track],
    map: Option<&mut MapSource>,
) -> Result<Pixmap, Box<dyn std::error::Error>> {
    let mut base = Pixmap::new(style.width, style.height).ok_or("invalid viewport size")?;
    base.fill(style.background);

    if let Some(map) = map {
        map.draw(&mut base, proj, args.background_map_visibility);
    }

    if let Some(color) = pre_draw_color {
        for track in tracks {
            stroke_track(&mut base, &track.points, proj, color, style.tail_width);
        }
    }

    Ok(base)
}
