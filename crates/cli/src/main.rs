mod cli;
mod color;
mod config;
mod encode;
mod http;
mod labels;
mod perspective;
mod sprites;
mod terrain;
mod tiles;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::Parser;
use gpx_motion_core::geo::mercator::{Projection, MAX_ZOOM};
use gpx_motion_core::math::smoothstep;
use gpx_motion_core::vector::maplibre;
use gpx_motion_core::{
    block_rect, draw_block, draw_labels, draw_overlay, draw_profile, parse, stroke_track,
    stroke_track_by_elevation, Anchor, FrameStyle, LabelMemory, LabelStyle, Lens, MapStyle,
    OverlayStyle, PlacedLabel, ProfileStyle, Relief, Terrain, Timeline, TrackPoint, TrackStyle,
};
use tiny_skia::{Color, Pixmap, Rect};

use rayon::prelude::*;

use cli::Args;
use config::{Config, MapSpec};
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

/// Frames sampled to bound how wide the info block gets over a video.
const INFO_SAMPLES: usize = 8;

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
    /// Screen areas the overlays take, for keeping map labels clear: the
    /// profile panel, the info block (the union of its size over sampled
    /// frames, as its numbers grow), and the attribution.
    fn overlay_rects(&self, canvas: (f32, f32)) -> Vec<Rect> {
        let mut rects = Vec::new();
        if let Some(profile) = self.profile {
            rects.extend(Rect::from_ltrb(
                0.0,
                canvas.1 - profile.reserved_height(),
                canvas.0,
                canvas.1,
            ));
        }
        if let (Some(anchor), Some(track)) = (self.info_anchor, self.tracks.first()) {
            let samples = (0..=INFO_SAMPLES).filter_map(|i| {
                let lines = info_lines(track, i as f64 / INFO_SAMPLES as f64, self.video_ms);
                block_rect(canvas, &lines, anchor, self.overlay_style)
            });
            rects.extend(samples.reduce(|a, b| {
                Rect::from_ltrb(
                    a.left().min(b.left()),
                    a.top().min(b.top()),
                    a.right().max(b.right()),
                    a.bottom().max(b.bottom()),
                )
                .unwrap_or(a)
            }));
        }
        if let Some(anchor) = self.attribution_anchor {
            let line = [self.attribution.to_string()];
            rects.extend(block_rect(canvas, &line, anchor, self.overlay_style));
        }
        rects
    }

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

/// How far into the follow zoom (`0` = whole route, `1` = `--zoom`) the
/// camera is at progress `u` of a `video_s`-second video: eased in over the
/// first `transition_s` seconds and back out over the last.
fn zoom_mix(u: f64, video_s: f64, transition_s: f64) -> f64 {
    let t = u * video_s;
    smoothstep(t.min(video_s - t) / transition_s)
}

fn progress(frame: u64, total_frames: u64) -> f64 {
    if total_frames > 1 {
        frame as f64 / (total_frames - 1) as f64
    } else {
        1.0
    }
}

/// Build the background map source from `--tms-url-template` or `--vector-tiles`.
fn map_source(config: &Config) -> Result<Option<MapSource>> {
    let cache = config.tile_cache.clone();
    let Some(spec) = &config.map else {
        return Ok(None);
    };
    let (url, max_zoom, style, export_style) = match spec {
        MapSpec::Raster { template } => {
            let fetcher = TileFetcher::new(template.clone(), cache)?;
            return Ok(Some(MapSource::Raster(fetcher)));
        }
        MapSpec::Vector {
            url,
            max_zoom,
            style,
            export_style,
        } => (url, *max_zoom, style, export_style),
    };
    let style = map_style(style)?;
    if let Some(path) = export_style {
        let json = serde_json::to_string_pretty(&style.to_json())?;
        std::fs::write(path, json + "\n").with_context(|| path.display().to_string())?;
    }
    let style = style.with_pixel_ratio(config.pixel_ratio);
    let tiles = VectorTileFetcher::new(url, max_zoom, cache)?;
    Ok(Some(MapSource::Vector { tiles, style }))
}

/// A preset by name, else a JSON style from that URL or file, printing
/// what an imported MapLibre style left out.
fn map_style(spec: &str) -> Result<MapStyle> {
    if let Some(style) = MapStyle::preset(spec) {
        return Ok(style);
    }
    let text = if spec.starts_with("http://") || spec.starts_with("https://") {
        fetch_text(spec).with_context(|| spec.to_string())?
    } else {
        let path = Path::new(spec);
        if !path.exists() {
            let looks_like_path = spec.contains(['/', '\\']) || spec.ends_with(".json");
            if looks_like_path {
                bail!("{spec}: style file not found");
            }
            bail!("unknown map style `{spec}` (light, dark, or a JSON style file or URL)");
        }
        std::fs::read_to_string(path).with_context(|| spec.to_string())?
    };
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .with_context(|| format!("{spec}: invalid JSON"))?;
    let (style, notes) = if maplibre::is_maplibre(&json) {
        // Patterns are drawn in their average color, from the style's sprite.
        let names = maplibre::pattern_names(&json);
        let patterns = if names.is_empty() {
            maplibre::PatternColors::new()
        } else {
            sprites::pattern_colors(&json, &names, http::fetch_bytes)
        };
        maplibre::from_maplibre_with(&json, &patterns)
    } else {
        MapStyle::from_json_with_notes(&text)
    }
    .with_context(|| spec.to_string())?;
    for note in notes {
        eprintln!("warning: map style: {note}");
    }
    Ok(style)
}

/// GET `url` as text.
fn fetch_text(url: &str) -> Result<String> {
    Ok(String::from_utf8(http::fetch_bytes(url)?)?)
}

/// The attribution line: `--attribution` if given, else the credit matching
/// the map source (plus the terrain data, when used).
fn attribution(config: &Config) -> String {
    if let Some(text) = &config.attribution {
        return text.clone();
    }
    let has_map = config.map.is_some();
    let vector = matches!(config.map, Some(MapSpec::Vector { .. }));
    match (vector, config.terrain_tiles.is_some()) {
        (true, false) => "© OpenMapTiles © OpenStreetMap contributors".to_string(),
        (false, false) => "© OpenStreetMap contributors".to_string(),
        (true, true) => "© OpenMapTiles © OSM · terrain © Mapzen".to_string(),
        (false, true) if has_map => "© OSM contributors · terrain © Mapzen".to_string(),
        (false, true) => "terrain © Mapzen".to_string(),
    }
}

/// Terrain under the route when `--terrain-tiles` is set.
fn terrain_of(
    config: &Config,
    points: &[TrackPoint],
    relief: &Relief,
    zoom: f64,
    wide_zoom: Option<f64>,
    map: Option<&mut MapSource>,
    background: Color,
) -> Result<Option<Terrain>> {
    let Some(template) = &config.terrain_tiles else {
        return Ok(None);
    };
    let sources = terrain::Sources {
        template,
        cache: config.tile_cache.clone(),
        map,
        backdrop: terrain::Backdrop {
            color: background,
            map_opacity: config.background_map_visibility,
        },
    };
    terrain::build(sources, points, relief, zoom, wide_zoom)
}

/// `--zoom` in device pixels: one level deeper per doubling of
/// `--pixel-ratio`.
fn fixed_zoom(config: &Config) -> Option<f64> {
    let levels = (config.pixel_ratio as f64).log2();
    config.zoom.map(|z| (z + levels).clamp(0.0, MAX_ZOOM))
}

/// Seconds a map label takes to fade in when it first appears in a video.
const LABEL_FADE_S: f64 = 0.4;

/// 2D tail length when `--tail-duration` is unset.
const DEFAULT_TAIL_MS: f64 = 5000.0;

/// Distance the heading is averaged over when `--heading-window` is unset.
const MIN_HEADING_WINDOW_M: f64 = 150.0;
/// Seconds of video whose travel the default heading window spans.
const HEADING_WINDOW_S: f64 = 2.0;
/// Seconds over which a follow camera eases into looking down more steeply
/// to keep the marker in sight over hills (and out again).
const SIGHT_EASE_S: f64 = 1.0;

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
        follow_zoom: fixed_zoom(config),
        heading,
        sight_frames: video_s.map_or(0, |_| (SIGHT_EASE_S * config.fps).round() as usize),
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
    match run(Args::parse()) {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Validate the options, then render; returns the summary line.
fn run(args: Args) -> Result<String> {
    let config = Config::try_from(args)?;
    if let Some(jobs) = config.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs.max(1))
            .build_global()
            .with_context(|| format!("could not configure {jobs} threads"))?;
    }
    if config.is_video() {
        run_video(&config)
    } else {
        run_png(&config)
    }
}

fn load_tracks(inputs: &[PathBuf], elevation_smoothing: f64) -> Result<Vec<Track>> {
    let mut tracks = Vec::new();
    for (i, path) in inputs.iter().enumerate() {
        let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        let points = parse(&text).with_context(|| path.display().to_string())?;
        let mut timeline = Timeline::build(&points)
            .with_context(|| format!("{}: needs at least two distinct points", path.display()))?;
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

fn run_png(config: &Config) -> Result<String> {
    let tracks = load_tracks(&config.inputs, config.elevation_smoothing)?;
    let points = all_points(&tracks);
    let style = TrackStyle {
        width: config.viewport_width,
        height: config.viewport_height,
        ..Default::default()
    }
    .scaled(config.pixel_ratio);
    let base_style = FrameStyle {
        width: style.width,
        height: style.height,
        background: style.background,
        ..Default::default()
    }
    .scaled(config.pixel_ratio);
    let mut map = map_source(config)?;
    let label_style =
        labels::label_style(config.labels.as_ref(), config.pixel_ratio, map.as_ref())?;

    if config.is_3d() {
        let perspective = perspective_of(config, &tracks[0], None);
        if let Some(map) = &map {
            map.check_3d()?;
        }
        let relief = relief_for(
            &tracks,
            config.elevation_exaggeration,
            config.terrain_tiles.is_some(),
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
            .context("could not fit track to viewport")?;
        let bearings = perspective.bearings(&tracks[0], &[0.0]);
        framing.cover(&bearings);
        let terrain = terrain_of(
            config,
            &points,
            &relief,
            framing.zoom(),
            None,
            map.as_mut(),
            style.background,
        )?;
        let ground = terrain.as_ref().map(|terrain| Ground { terrain, relief });
        let camera = perspective.cameras(&framing, &tracks[0], &[0.0], &bearings, ground)[0];
        if let Some(map) = map.as_mut() {
            map.prefetch_3d(&camera, false)?;
        }
        let styles: Vec<FrameStyle> = tracks
            .iter()
            .map(|t| {
                FrameStyle {
                    width: style.width,
                    height: style.height,
                    tail_color: t.color,
                    color_by_elevation: config.color_by_elevation,
                    ..Default::default()
                }
                .scaled(config.pixel_ratio)
            })
            .collect();
        let canvas = (style.width as f32, style.height as f32);
        let label_rects = attribution_rect(config, canvas, true);
        let scene = Scene {
            map: map.as_ref(),
            map_opacity: config.background_map_visibility,
            background: style.background,
            tracks: &tracks,
            styles: &styles,
            tail_us: &[],
            relief,
            terrain: terrain.as_ref(),
            labels: label_style.as_ref(),
            label_rects: &label_rects,
            label_cache: Default::default(),
            fade: false,
        };
        let labels = scene.place_labels(&camera, &mut LabelMemory::new(1));
        let mut pixmap = scene
            .render(&camera, 1.0, &labels)
            .context("invalid viewport size")?;
        draw_attribution(&mut pixmap, config, map.is_some() || terrain.is_some());
        pixmap
            .save_png(&config.output)
            .with_context(|| config.output.display().to_string())?;
        return Ok(format!(
            "Rendered {} track(s) in 3D, {} points -> {}",
            tracks.len(),
            points.len(),
            config.output.display()
        ));
    }

    let mut pixmap = match Projection::fit(&points, style.width, style.height, style.padding) {
        Some(proj) => {
            let canvas = (style.width as f32, style.height as f32);
            let rects = attribution_rect(config, canvas, map.is_some());
            let labels = label_style.as_ref().map(|l| (l, rects.as_slice()));
            let mut pixmap = build_base(
                config,
                &proj,
                &base_style,
                None,
                &tracks,
                map.as_mut(),
                labels,
            )?;
            for track in &tracks {
                if config.color_by_elevation {
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
                Pixmap::new(style.width, style.height).context("invalid viewport size")?;
            pixmap.fill(style.background);
            pixmap
        }
    };
    draw_attribution(&mut pixmap, config, map.is_some());
    pixmap
        .save_png(&config.output)
        .with_context(|| config.output.display().to_string())?;

    Ok(format!(
        "Rendered {} track(s), {} points -> {}",
        tracks.len(),
        points.len(),
        config.output.display()
    ))
}

/// Where the still-image attribution goes, for keeping labels clear of it.
fn attribution_rect(config: &Config, canvas: (f32, f32), has_map: bool) -> Vec<Rect> {
    let Some(anchor) = config.attribution_anchor.filter(|_| has_map) else {
        return Vec::new();
    };
    let line = [attribution(config)];
    let style = OverlayStyle::default().scaled(config.pixel_ratio);
    block_rect(canvas, &line, anchor, &style)
        .into_iter()
        .collect()
}

/// Credit the map source on a still image (only when a map was drawn).
fn draw_attribution(pixmap: &mut Pixmap, config: &Config, has_map: bool) {
    if !has_map {
        return;
    }
    if let Some(anchor) = config.attribution_anchor {
        let line = attribution(config);
        if !line.is_empty() {
            draw_block(
                pixmap,
                std::slice::from_ref(&line),
                anchor,
                &OverlayStyle::default().scaled(config.pixel_ratio),
            );
        }
    }
}

fn run_video(config: &Config) -> Result<String> {
    let tracks = load_tracks(&config.inputs, config.elevation_smoothing)?;
    let points = all_points(&tracks);

    let video_ms = config
        .total_time
        .or_else(|| {
            tracks
                .iter()
                .filter_map(|t| t.timeline.duration_ms)
                .fold(None, |acc, d| Some(f64::max(acc.unwrap_or(0.0), d)))
        })
        .context("no track has timestamps; pass --total-time")?;
    if video_ms <= 0.0 {
        bail!("video duration must be positive");
    }

    let total_frames = ((video_ms / 1000.0) * config.fps).round().max(1.0) as u64;
    let pre_draw_color = config.pre_draw_color;
    let info_anchor = config.info_anchor;
    let attribution_anchor = config.attribution_anchor;
    let profile = config
        .elevation_profile
        .then(|| ProfileStyle::default().scaled(config.pixel_ratio));
    let reserved_bottom = profile.map_or(0.0, |p| p.reserved_height());
    let overlay_style = OverlayStyle {
        bottom_inset: reserved_bottom,
        ..OverlayStyle::default().scaled(config.pixel_ratio)
    };

    // Per-track styles carry the palette color; tail length is relative to each
    // track's own span (real time when timestamped, else the video duration).
    let styles: Vec<FrameStyle> = tracks
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
    let tail_us: Vec<f64> = tracks
        .iter()
        .map(|t| {
            let reference_ms = t.timeline.duration_ms.unwrap_or(video_ms);
            match config.tail_duration {
                Some(ms) => (ms / reference_ms).clamp(0.0, 1.0),
                None if config.pitch > 0.0 => 1.0,
                None => (DEFAULT_TAIL_MS / reference_ms).clamp(0.0, 1.0),
            }
        })
        .collect();

    let base_style = styles.first().cloned().unwrap_or_default();
    let mut map = map_source(config)?;
    let attribution = attribution(config);

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
    let label_style =
        labels::label_style(config.labels.as_ref(), config.pixel_ratio, map.as_ref())?;
    let label_rects =
        params.overlay_rects((config.viewport_width as f32, config.viewport_height as f32));

    let mut encoder = FfmpegEncoder::start(
        &config.output,
        config.viewport_width,
        config.viewport_height,
        config.fps,
    )?;

    if config.is_3d() {
        let perspective = perspective_of(config, &tracks[0], Some(video_ms / 1000.0));
        if let Some(map) = &map {
            map.check_3d()?;
        }
        let relief = relief_for(
            &tracks,
            config.elevation_exaggeration,
            config.terrain_tiles.is_some(),
        );
        let mut framing = perspective
            .framing(
                &tracks,
                &relief,
                config.viewport_width,
                config.viewport_height,
                base_style.padding,
                reserved_bottom as f64,
            )
            .context("could not fit track to viewport")?;
        let us: Vec<f64> = (0..total_frames)
            .map(|f| progress(f, total_frames))
            .collect();
        let bearings = perspective.bearings(&tracks[0], &us);
        framing.cover(&bearings);
        // With --zoom-transition, the ends of the video show the whole
        // route from this wider framing.
        let wide = Perspective {
            follow_zoom: None,
            ..perspective
        };
        let wide_framing = match config.zoom_transition {
            Some(transition_s) => {
                let mut wide_framing = wide
                    .framing(
                        &tracks,
                        &relief,
                        config.viewport_width,
                        config.viewport_height,
                        base_style.padding,
                        reserved_bottom as f64,
                    )
                    .context("could not fit track to viewport")?;
                wide_framing.cover(&bearings);
                Some((wide_framing, transition_s))
            }
            None => None,
        };
        let terrain = terrain_of(
            config,
            &points,
            &relief,
            framing.zoom(),
            wide_framing.as_ref().map(|(f, _)| f.zoom()),
            map.as_mut(),
            base_style.background,
        )?;
        let ground = terrain.as_ref().map(|terrain| Ground { terrain, relief });
        let mut cameras = perspective.cameras(&framing, &tracks[0], &us, &bearings, ground);
        if let Some((wide_framing, transition_s)) = wide_framing {
            let mixes: Vec<f64> = us
                .iter()
                .map(|&u| zoom_mix(u, video_ms / 1000.0, transition_s))
                .collect();
            cameras = wide.zoom_into(
                &wide_framing,
                &tracks[0],
                &us,
                &bearings,
                &cameras,
                &mixes,
                ground,
            );
        }
        if let Some(map) = map.as_mut() {
            for camera in &cameras {
                map.prefetch_3d(camera, config.zoom_transition.is_some())?;
            }
        }
        let scene = Scene {
            map: map.as_ref(),
            map_opacity: config.background_map_visibility,
            background: base_style.background,
            tracks: &tracks,
            styles: &styles,
            tail_us: &tail_us,
            relief,
            terrain: terrain.as_ref(),
            labels: label_style.as_ref(),
            label_rects: &label_rects,
            label_cache: Default::default(),
            fade: config.zoom_transition.is_some(),
        };
        // Labels depend on the frame before, so they are placed in order
        // first; frames then render in parallel.
        let fade_frames = (LABEL_FADE_S * config.fps).round() as u32;
        let mut memory = LabelMemory::new(fade_frames);
        let labels: Vec<Vec<PlacedLabel>> = cameras
            .iter()
            .map(|camera| scene.place_labels(camera, &mut memory))
            .collect();
        render_parallel(&mut encoder, total_frames, config, |frame| {
            let u = progress(frame, total_frames);
            let mut pixmap = scene
                .render(&cameras[frame as usize], u, &labels[frame as usize])
                .expect("viewport validated by the encoder");
            params.draw_hud(&mut pixmap, u);
            pixmap
        })?;
        encoder.finish()?;
        return Ok(format!(
            "Encoded {total_frames} frames @ {}fps (3D), {} track(s) -> {} ({:.1}s)",
            config.fps,
            tracks.len(),
            config.output.display(),
            video_ms / 1000.0
        ));
    }

    match fixed_zoom(config) {
        // Static camera: fit the whole route, composite the background once, and
        // render frames in parallel (each is an independent clone + overlay).
        None => {
            let fit_height = config
                .viewport_height
                .saturating_sub(reserved_bottom as u32)
                .max(1);
            let proj = Projection::fit(
                &points,
                config.viewport_width,
                fit_height,
                base_style.padding,
            )
            .context("could not fit track to viewport")?;
            let base = build_base(
                config,
                &proj,
                &base_style,
                pre_draw_color,
                &tracks,
                map.as_mut(),
                label_style.as_ref().map(|l| (l, label_rects.as_slice())),
            )?;
            render_parallel(&mut encoder, total_frames, config, |frame| {
                let mut pixmap = base.clone();
                params.draw(&mut pixmap, &proj, progress(frame, total_frames));
                pixmap
            })?;
        }
        // Follow camera: recenter on the lead marker every frame, so each frame
        // composites its own (cache-backed) tile background — kept sequential.
        // With --zoom-transition, the view eases between the whole-route fit
        // and the follow camera at both ends of the video.
        Some(zoom) => {
            let wide = match config.zoom_transition {
                Some(transition_s) => {
                    let fit_height = config
                        .viewport_height
                        .saturating_sub(reserved_bottom as u32)
                        .max(1);
                    let fit = Projection::fit(
                        &points,
                        config.viewport_width,
                        fit_height,
                        base_style.padding,
                    )
                    .context("could not fit track to viewport")?;
                    Some((fit, transition_s))
                }
                None => None,
            };
            for frame in 0..total_frames {
                let u = progress(frame, total_frames);
                let (lat, lon) = tracks[0].timeline.position_at(u);
                let close = Projection::centered(
                    lat,
                    lon,
                    zoom,
                    config.viewport_width,
                    config.viewport_height,
                );
                let proj = match wide {
                    Some((fit, transition_s)) => {
                        fit.zoom_between(&close, zoom_mix(u, video_ms / 1000.0, transition_s))
                    }
                    None => close,
                };
                let mut pixmap = build_base(
                    config,
                    &proj,
                    &base_style,
                    pre_draw_color,
                    &tracks,
                    map.as_mut(),
                    label_style.as_ref().map(|l| (l, label_rects.as_slice())),
                )?;
                params.draw(&mut pixmap, &proj, u);
                encoder.write_frame(pixmap.data())?;
            }
        }
    }
    encoder.finish()?;

    Ok(format!(
        "Encoded {total_frames} frames @ {}fps, {} track(s) -> {} ({:.1}s)",
        config.fps,
        tracks.len(),
        config.output.display(),
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
    config: &Config,
    render: impl Fn(u64) -> Pixmap + Sync,
) -> Result<()> {
    const MEMORY_BUDGET: usize = 256 * 1024 * 1024;
    let frame_bytes = (config.viewport_width as usize) * (config.viewport_height as usize) * 4;
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
    config: &Config,
    proj: &Projection,
    style: &FrameStyle,
    pre_draw_color: Option<Color>,
    tracks: &[Track],
    map: Option<&mut MapSource>,
    labels: Option<(&LabelStyle, &[Rect])>,
) -> Result<Pixmap> {
    let mut base = Pixmap::new(style.width, style.height).context("invalid viewport size")?;
    base.fill(style.background);

    if let Some(map) = map {
        let fade = config.zoom_transition.is_some();
        map.draw(&mut base, proj, config.background_map_visibility, fade);
        if let Some(reason) = map.failure() {
            bail!("{reason}");
        }
        if let Some((label_style, rects)) = labels {
            let screen = tracks
                .iter()
                .map(|t| proj.project_path(&t.points))
                .collect();
            let keepout = labels::keepout(rects, screen, Vec::new(), style.tail_width, label_style);
            let canvas = (style.width, style.height);
            let placed = map.labels_2d(proj, canvas, label_style, &keepout, fade);
            draw_labels(&mut base, &placed, label_style);
        }
    }

    if let Some(color) = pre_draw_color {
        for track in tracks {
            stroke_track(&mut base, &track.points, proj, color, style.tail_width);
        }
    }

    Ok(base)
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
}
