//! Label style and the tuning constants for placement.

use tiny_skia::Color;

use crate::render::TextFont;

/// 3D labels farther than this many times the camera distance are dropped
/// (they would sit in the horizon fog).
pub(super) const LABEL_FAR_FACTOR: f64 = 2.0;

/// Line labels (road and river names) are this much smaller than point
/// labels of the base size.
pub(super) const LINE_SIZE_FACTOR: f32 = 0.85;
/// Gap between repeats of a line label along a long line, in CSS pixels.
pub(super) const LINE_REPEAT_PX: f32 = 280.0;

/// Most labels drawn in one view, so dense cities stay readable. In a
/// sequence of views, labels shown in the view before keep their places
/// first and new ones fill the rest (see
/// [`LabelSources::place_3d_steady`](super::LabelSources::place_3d_steady)).
pub(super) const MAX_LABELS: usize = 60;
/// Same-named labels closer than this many CSS pixels are duplicates (tile
/// buffers repeat features near tile edges).
pub(super) const DUPLICATE_PX: f32 = 200.0;
/// Clear space kept around each label (beyond its halo), in CSS pixels.
pub(super) const PADDING_PX: f32 = 3.0;
/// Labels that would draw smaller than this many CSS pixels are skipped.
pub(super) const MIN_SIZE_PX: f32 = 8.0;
/// Smallest size, as a share of [`MIN_SIZE_PX`], a label already shown keeps
/// fading out at, so one hovering at the limit doesn't blink.
pub(super) const FADING_MIN_SIZE: f32 = 0.75;
/// Size factor at and above which a label is fully opaque; it fades to
/// [`FAR_OPACITY`] as the factor drops to [`FAR_SCALE`] (distant 3D labels).
pub(super) const NEAR_SCALE: f32 = 0.9;
pub(super) const FAR_SCALE: f32 = 0.6;
pub(super) const FAR_OPACITY: f32 = 0.35;

/// How far past the canvas edge (CSS pixels) a label's anchor may go while
/// it fades out, rather than vanishing as it leaves.
pub(super) const EDGE_FADE_PX: f32 = 48.0;

/// How far apart (in 4096ths of a tile) two pieces of a line cut at a tile
/// edge may end and still join: tiles simplify their geometry separately.
pub(super) const STITCH_UNITS: f64 = 4.0;

/// Lowest (CSS-pixel) world zoom naming lines of each rank: highways,
/// primary roads, rivers, secondary roads, minor roads, paths.
pub(super) const LINE_MIN_ZOOM: [f64; 6] = [0.0, 12.0, 12.0, 13.5, 15.0, 16.0];

/// How map labels look and which name they show.
#[derive(Debug, Clone)]
pub struct LabelStyle {
    /// Font with glyphs for the labels' script (e.g. a Korean font); labels
    /// it can't render are skipped.
    pub font: TextFont,
    /// Base text size in CSS pixels; cities draw larger, neighbourhoods
    /// smaller.
    pub size: f32,
    /// Text color.
    pub color: Color,
    /// Outline drawn around the text for legibility over the map.
    pub halo: Color,
    /// Prefer the `name:<language>` property (e.g. `ko`, `en`) over `name`.
    pub language: Option<String>,
    /// Device pixels per CSS pixel.
    pub pixel_ratio: f32,
}

impl LabelStyle {
    /// Smallest base text size ([`LabelStyle::size`]) the front ends accept,
    /// in CSS pixels.
    pub const MIN_SIZE: f32 = 4.0;
    /// Largest base text size ([`LabelStyle::size`]) the front ends accept,
    /// in CSS pixels.
    pub const MAX_SIZE: f32 = 96.0;
    /// The CLI's base text size when none is given, in CSS pixels.
    ///
    /// A pixel larger than [`LabelStyle::BROWSER_DEFAULT_SIZE`]: both are CSS
    /// pixels scaled by the pixel ratio, so the two front ends just picked
    /// different defaults, kept so existing output doesn't change.
    pub const CLI_DEFAULT_SIZE: f32 = 13.0;
    /// The browser demo's base text size when none is given, in CSS pixels;
    /// see [`LabelStyle::CLI_DEFAULT_SIZE`].
    pub const BROWSER_DEFAULT_SIZE: f32 = 12.0;

    /// Light text with a dark halo over a dark map `background`, or the
    /// reverse over a light one.
    pub fn contrasting(
        font: TextFont,
        size: f32,
        background: Color,
        language: Option<String>,
        pixel_ratio: f32,
    ) -> Self {
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
        Self {
            font,
            size,
            color,
            halo,
            language,
            pixel_ratio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrasting_colors_follow_the_background() {
        let font = TextFont::bundled().clone();
        let dark = LabelStyle::contrasting(font.clone(), 12.0, Color::BLACK, None, 1.0);
        assert!(dark.color.red() > 0.5 && dark.halo.red() < 0.5);
        let light = LabelStyle::contrasting(font, 12.0, Color::WHITE, None, 1.0);
        assert!(light.color.red() < 0.5 && light.halo.red() > 0.5);
    }
}
