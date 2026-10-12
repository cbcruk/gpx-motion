//! Map label setup for the CLI: the label font and colors, and the screen
//! areas labels keep clear of.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use gpx_motion_core::{Keepout, LabelStyle, TextFont};
use tiny_skia::Rect;

use crate::config::LabelOptions;
use crate::tiles::MapSource;

/// System fonts tried for `--labels` without `--label-font`, covering Korean
/// on macOS, common Linux distributions, and Windows.
const SYSTEM_LABEL_FONTS: &[&str] = &[
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
    "C:\\Windows\\Fonts\\malgun.ttf",
];

/// Label style for `--labels` (`options`), at `pixel_ratio`: the font,
/// and colors that contrast with the map background.
///
/// The map must be a vector one; [`Config`](crate::config::Config) checks
/// that before any I/O.
pub fn label_style(
    options: Option<&LabelOptions>,
    pixel_ratio: f32,
    map: Option<&MapSource>,
) -> Result<Option<LabelStyle>> {
    let Some(options) = options else {
        return Ok(None);
    };
    let Some(MapSource::Vector { style, .. }) = map else {
        bail!("--labels needs a vector map (--vector-tiles)");
    };
    let path = match &options.font {
        Some(path) => path.clone(),
        None => SYSTEM_LABEL_FONTS
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
            .context("--labels found no system Korean/CJK font; pass --label-font")?,
    };
    let data = std::fs::read(&path).with_context(|| path.display().to_string())?;
    let font = TextFont::from_bytes(data, 0)
        .with_context(|| format!("{}: not a usable font", path.display()))?;
    Ok(Some(LabelStyle::contrasting(
        font,
        options.size,
        style.background,
        options.language.clone(),
        pixel_ratio,
    )))
}

/// What labels avoid: overlay `rects`, the drawn `tracks` (screen
/// polylines, `track_width` wide), and filled `areas` such as 3D curtains.
pub fn keepout(
    rects: &[Rect],
    tracks: Vec<Vec<(f32, f32)>>,
    areas: Vec<Vec<(f32, f32)>>,
    track_width: f32,
    style: &LabelStyle,
) -> Keepout {
    Keepout::around(rects, tracks, areas, track_width, style.pixel_ratio)
}
