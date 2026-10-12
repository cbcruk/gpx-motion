//! Screen-space overlays drawn over the map and tracks: the elevation
//! profile, the info block, and the attribution.

use gpx_motion_core::{
    block_rect, draw_block, draw_profile, Anchor, FrameStyle, OverlayStyle, ProfileStyle,
};
use tiny_skia::{Pixmap, Rect};

use crate::config::{Config, MapSpec};
use crate::frames::Shot;
use crate::track::Track;

/// Frames sampled to bound how wide the info block gets over a video.
const INFO_SAMPLES: usize = 8;

/// The overlays one run draws on every frame (safe to share across
/// threads).
pub struct Hud {
    profile: Option<ProfileStyle>,
    info: Option<InfoBlock>,
    attribution_anchor: Option<Anchor>,
    attribution: String,
    credit: Credit,
    style: OverlayStyle,
}

/// The lead track's position, speed, climb, and elapsed time.
struct InfoBlock {
    anchor: Anchor,
    /// Video length, the time scale of a track without timestamps.
    video_ms: f64,
}

/// When the attribution is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Credit {
    /// On every video frame.
    Always,
    /// On a still image only over a map or terrain, which it credits.
    OverMap,
}

impl Hud {
    /// The overlays the options ask for. A still image only ever carries
    /// the attribution.
    pub fn new(config: &Config, shot: &Shot) -> Self {
        let video = matches!(shot, Shot::Video { .. });
        let profile = (video && config.elevation_profile)
            .then(|| ProfileStyle::default().scaled(config.pixel_ratio));
        let style = OverlayStyle {
            bottom_inset: profile.map_or(0.0, |p| p.reserved_height()),
            ..OverlayStyle::default().scaled(config.pixel_ratio)
        };
        let (info, credit) = match *shot {
            Shot::Still => (None, Credit::OverMap),
            Shot::Video { ms, .. } => {
                let info = config.info_anchor.map(|anchor| InfoBlock {
                    anchor,
                    video_ms: ms,
                });
                (info, Credit::Always)
            }
        };
        Self {
            profile,
            info,
            attribution_anchor: config.attribution_anchor,
            attribution: attribution(config),
            credit,
            style,
        }
    }

    /// Height the profile panel takes at the bottom of the frame, which the
    /// route framing keeps clear.
    pub fn reserved_bottom(&self) -> f32 {
        self.profile.map_or(0.0, |p| p.reserved_height())
    }

    /// Screen areas the overlays take, for keeping map labels clear: the
    /// profile panel, the info block (the union of its size over sampled
    /// frames, as its numbers grow), and the attribution.
    pub fn rects(&self, lead: &Track, canvas: (f32, f32)) -> Vec<Rect> {
        let mut rects = Vec::new();
        if let Some(profile) = self.profile {
            rects.extend(Rect::from_ltrb(
                0.0,
                canvas.1 - profile.reserved_height(),
                canvas.0,
                canvas.1,
            ));
        }
        if let Some(info) = &self.info {
            let samples = (0..=INFO_SAMPLES).filter_map(|i| {
                let lines = info_lines(lead, i as f64 / INFO_SAMPLES as f64, info.video_ms);
                block_rect(canvas, &lines, info.anchor, &self.style)
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
            let line = [self.attribution.clone()];
            rects.extend(block_rect(canvas, &line, anchor, &self.style));
        }
        rects
    }

    /// Draw the overlays for the `lead` track (styled by `lead_style`) at
    /// progress `u`; `map_shown` tells a still image whether there is a map
    /// or terrain to credit.
    pub fn draw(
        &self,
        pixmap: &mut Pixmap,
        lead: &Track,
        lead_style: &FrameStyle,
        u: f64,
        map_shown: bool,
    ) {
        if let Some(profile) = &self.profile {
            draw_profile(pixmap, &lead.timeline, u, profile, lead_style);
        }
        if let Some(info) = &self.info {
            let lines = info_lines(lead, u, info.video_ms);
            draw_block(pixmap, &lines, info.anchor, &self.style);
        }
        let credited = self.credit == Credit::Always || map_shown;
        if let Some(anchor) = self.attribution_anchor.filter(|_| credited) {
            if !self.attribution.is_empty() {
                draw_block(
                    pixmap,
                    std::slice::from_ref(&self.attribution),
                    anchor,
                    &self.style,
                );
            }
        }
    }
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
