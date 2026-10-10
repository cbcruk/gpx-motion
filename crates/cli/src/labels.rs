//! Map label setup for the CLI: the label font and colors, and the screen
//! areas labels keep clear of.

use std::path::PathBuf;

use gpx_motion_core::{Keepout, LabelStyle, TextFont};
use tiny_skia::{Color, Rect};

use crate::tiles::MapSource;
use crate::Args;

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

/// Label style for `--labels`: the font, and colors that contrast with the
/// map background.
pub fn label_style(
    args: &Args,
    map: Option<&MapSource>,
) -> Result<Option<LabelStyle>, Box<dyn std::error::Error>> {
    if !args.labels {
        return Ok(None);
    }
    let background = match map {
        Some(MapSource::Vector { style, .. }) => style.background,
        Some(MapSource::Raster(_)) => {
            return Err(
                "--labels needs a vector map (--vector-tiles); raster tiles carry no names".into(),
            )
        }
        None => return Err("--labels needs a vector map (--vector-tiles)".into()),
    };
    let path = match &args.label_font {
        Some(path) => path.clone(),
        None => SYSTEM_LABEL_FONTS
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
            .ok_or("--labels found no system Korean/CJK font; pass --label-font")?,
    };
    let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let font = TextFont::from_bytes(data, 0)
        .ok_or_else(|| format!("{}: not a usable font", path.display()))?;
    let luminance =
        0.2126 * background.red() + 0.7152 * background.green() + 0.0722 * background.blue();
    let (color, halo) = if luminance < 0.5 {
        (
            Color::from_rgba8(0xe6, 0xe6, 0xe6, 0xff),
            Color::from_rgba8(0x10, 0x10, 0x10, 0xb0),
        )
    } else {
        (
            Color::from_rgba8(0x33, 0x33, 0x33, 0xff),
            Color::from_rgba8(0xff, 0xff, 0xff, 0xc0),
        )
    };
    Ok(Some(LabelStyle {
        font,
        size: args.label_size,
        color,
        halo,
        language: args.label_language.clone(),
        pixel_ratio: args.pixel_ratio,
    }))
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
    Keepout {
        rects: rects.to_vec(),
        tracks,
        track_half_width: track_width / 2.0 + 2.0 * style.pixel_ratio,
        areas,
    }
}
