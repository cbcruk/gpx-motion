//! The style data model: [`MapStyle`], its [`Rule`]s, and how they scale with zoom.

use tiny_skia::Color;

use crate::vector::expr::{Curve, Filter};
use crate::vector::extrusion::Extrusion;
use crate::vector::mvt::{Feature, Layer};
use crate::vector::offset::OffsetCurve;

/// How a matched feature is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Paint {
    /// Fill polygon features.
    Fill(Color),
    /// Stroke line-string features at a fixed pixel width.
    Line { color: Color, width: f32 },
}

/// Draws the features of one source layer, optionally narrowed by `class`
/// and a MapLibre filter.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rule {
    pub layer: String,
    /// Accepted `class` property values; empty matches every feature.
    pub(crate) classes: Vec<String>,
    pub paint: Paint,
    /// A MapLibre filter features must also pass.
    pub filter: Option<Filter>,
    /// Line width in CSS pixels by MapLibre zoom, replacing the line paint's
    /// width (which is then `1`) and its zoom scaling.
    pub(crate) width_curve: Option<Curve>,
    /// Dash pattern in multiples of the line width (dash, gap, ...).
    pub(crate) dash: Option<Vec<f32>>,
    /// MapLibre zooms `[min, max)` at which the rule draws.
    pub zooms: Option<(f64, f64)>,
    /// Opacity (`0..=1`) by MapLibre zoom, applied on top of the paint color.
    pub opacity: Option<Curve>,
    /// Also stroke the outlines of polygon features (MapLibre `line` layers
    /// do); otherwise lines draw only line features.
    pub(crate) outlines: bool,
    /// How line ends look; `None` rounds them.
    pub cap: Option<Cap>,
    /// How line corners look; `None` rounds them.
    pub join: Option<Join>,
    /// Color by MapLibre zoom, replacing the paint's color.
    pub colors: Option<ColorCurve>,
    /// Whether fills are antialiased, by MapLibre zoom (`1` on, `0` off; see
    /// [`Curve::parse_switch`]); `None` antialiases.
    pub(crate) antialias: Option<Curve>,
    /// Offset of the drawn features in CSS pixels along the map's axes, by
    /// MapLibre zoom (MapLibre's `fill-translate`).
    pub(crate) translate: Option<OffsetCurve>,
    /// How far in from a line's edges it fades out, in CSS pixels by
    /// MapLibre zoom (MapLibre's `line-blur`).
    pub(crate) blur: Option<Curve>,
    /// A gap this wide (CSS pixels by MapLibre zoom) down a line's middle,
    /// drawn as two lines either side of it (MapLibre's `line-gap-width`).
    pub(crate) gap: Option<Curve>,
    /// Raise a fill's polygons into prisms in 3D views (MapLibre's
    /// `fill-extrusion`); flat views draw the fill as usual.
    pub extrude: Option<Extrusion>,
}

/// Where a stroked line turns a corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Join {
    Miter,
    Round,
    Bevel,
}

impl Join {
    pub(super) fn name(self) -> &'static str {
        match self {
            Join::Miter => "miter",
            Join::Round => "round",
            Join::Bevel => "bevel",
        }
    }

    /// Parse `miter`, `round`, or `bevel`.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        [Join::Miter, Join::Round, Join::Bevel]
            .into_iter()
            .find(|j| j.name() == name)
    }
}

/// A color that changes with zoom: stops blended in RGB, like MapLibre's
/// `interpolate` on colors (exponentially when `base` isn't 1), or held
/// from one stop to the next, like its `step`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColorCurve {
    /// `(zoom, color)` pairs in increasing zoom order.
    pub(crate) stops: Vec<(f64, Color)>,
    pub(crate) base: f64,
    /// Hold each stop's color until the next (MapLibre `step`) instead of
    /// blending.
    pub(crate) step: bool,
}

impl ColorCurve {
    /// The color at `zoom` (clamped to the first and last stops).
    pub(crate) fn at(&self, zoom: f64) -> Color {
        let i = self.stops.partition_point(|s| s.0 <= zoom);
        if i == 0 {
            return self.stops[0].1;
        }
        let Some(&(z1, b)) = self.stops.get(i).filter(|_| !self.step) else {
            return self.stops[i - 1].1;
        };
        let (z0, a) = self.stops[i - 1];
        let t = if (self.base - 1.0).abs() < 1e-9 {
            (zoom - z0) / (z1 - z0)
        } else {
            (self.base.powf(zoom - z0) - 1.0) / (self.base.powf(z1 - z0) - 1.0)
        } as f32;
        let mix = |x: f32, y: f32| x + (y - x) * t;
        Color::from_rgba(
            mix(a.red(), b.red()),
            mix(a.green(), b.green()),
            mix(a.blue(), b.blue()),
            mix(a.alpha(), b.alpha()),
        )
        .unwrap_or(a)
    }
}

/// The end of a stroked line (or of each dash).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cap {
    Butt,
    Round,
    Square,
}

impl Cap {
    pub(super) fn name(self) -> &'static str {
        match self {
            Cap::Butt => "butt",
            Cap::Round => "round",
            Cap::Square => "square",
        }
    }

    /// Parse `butt`, `round`, or `square`.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        [Cap::Butt, Cap::Round, Cap::Square]
            .into_iter()
            .find(|c| c.name() == name)
    }
}

impl Rule {
    /// A rule drawing `paint` on `layer`'s features of `classes` (all when
    /// empty), with no filter, zoom range, or line extras.
    pub(crate) fn new(layer: &str, classes: &[&str], paint: Paint) -> Self {
        Self {
            layer: layer.to_string(),
            classes: classes
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            paint,
            filter: None,
            width_curve: None,
            dash: None,
            zooms: None,
            opacity: None,
            outlines: false,
            cap: None,
            join: None,
            colors: None,
            antialias: None,
            translate: None,
            blur: None,
            gap: None,
            extrude: None,
        }
    }

    /// Whether a feature with this `class` value passes the filter.
    pub(crate) fn accepts(&self, class: Option<&str>) -> bool {
        self.classes.is_empty() || class.is_some_and(|c| self.classes.iter().any(|k| k == c))
    }

    /// Whether `feature` of `layer` passes the class list and the filter.
    pub(crate) fn matches(&self, layer: &Layer, feature: &Feature) -> bool {
        let class = layer.property(feature, "class").and_then(|v| v.as_str());
        self.accepts(class)
            && self
                .filter
                .as_ref()
                .is_none_or(|f| f.accepts(layer, feature))
    }

    /// Whether the rule draws at MapLibre zoom `zoom`.
    pub(crate) fn draws_at(&self, zoom: f64) -> bool {
        self.zooms
            .is_none_or(|(min, max)| zoom >= min && zoom < max)
    }
}

/// A background color plus rules drawn in order (later rules paint on top).
///
/// Line widths are in CSS pixels at world zoom 14, and scale by `√2` per
/// zoom level away from it (clamped) times the pixel ratio.
#[derive(Debug, Clone, PartialEq)]
pub struct MapStyle {
    /// Canvas fill under every rule.
    pub background: Color,
    pub(crate) rules: Vec<Rule>,
    /// Device pixels per CSS pixel (e.g. `2` on a retina screen or for a
    /// high-resolution export).
    pub pixel_ratio: f32,
    /// Draw from the tile zoom MapLibre would load (one level coarser data,
    /// see [`MapStyle::data_zoom`]), as styles imported from it expect.
    pub(crate) maplibre_tiles: bool,
}

/// Zoom at which style line widths apply unscaled.
pub(crate) const REFERENCE_ZOOM: f64 = 14.0;
/// Range of the zoom-based line width factor.
const ZOOM_WIDTH_FACTOR: (f64, f64) = (0.35, 3.0);

impl MapStyle {
    /// The same style for a canvas with `ratio` device pixels per CSS pixel.
    #[must_use]
    pub fn with_pixel_ratio(mut self, ratio: f32) -> Self {
        self.pixel_ratio = ratio.max(f32::EPSILON);
        self
    }

    /// The style without its extrusion rules, for ground a 3D view raises
    /// the extrusions over (as prisms) instead of drawing them flat.
    #[must_use]
    pub fn without_extrusions(&self) -> Self {
        let mut style = self.clone();
        style.rules.retain(|r| r.extrude.is_none());
        style
    }

    /// Whether any rule raises extrusions in 3D views.
    pub fn extrudes(&self) -> bool {
        self.rules.iter().any(|r| r.extrude.is_some())
    }

    /// Multiplier for style line widths at a world zoom: `√2` per zoom level
    /// away from [`REFERENCE_ZOOM`] (clamped), times the pixel ratio.
    ///
    /// The zoom is first taken back to CSS pixels, so a 2× canvas (fitted one
    /// zoom level deeper) draws the same roads twice as wide rather than
    /// three times.
    pub(crate) fn line_scale(&self, zoom: f64) -> f32 {
        let css_zoom = zoom - f64::from(self.pixel_ratio).log2();
        let factor = 2f64
            .powf((css_zoom - REFERENCE_ZOOM) / 2.0)
            .clamp(ZOOM_WIDTH_FACTOR.0, ZOOM_WIDTH_FACTOR.1);
        self.pixel_ratio * factor as f32
    }

    /// The MapLibre zoom matching world `zoom` on this style's canvas:
    /// MapLibre counts 512 px tiles in CSS pixels, so it is one level below
    /// the 256 px world zoom taken back to CSS pixels.
    pub(crate) fn maplibre_zoom(&self, zoom: f64) -> f64 {
        zoom - f64::from(self.pixel_ratio).log2() - 1.0
    }

    /// The deepest tile zoom to draw world `zoom` from, when the style wants
    /// MapLibre's choice (as styles imported from it do): the whole MapLibre
    /// zoom, since its 512 px tiles hold one level less detail than 256 px
    /// ones at the same scale. `None` keeps the usual choice.
    pub fn data_zoom(&self, zoom: f64) -> Option<u32> {
        self.maplibre_tiles
            .then(|| self.maplibre_zoom(zoom).floor().max(0.0) as u32)
    }

    /// Stroke width in device pixels of a line `rule` at world `zoom`, for a
    /// paint width of `1` (see [`Rule::width_curve`]); `None` for rules
    /// without a curve, whose width scales by [`MapStyle::line_scale`].
    pub(crate) fn curve_width(&self, rule: &Rule, zoom: f64) -> Option<f32> {
        let curve = rule.width_curve.as_ref()?;
        Some(curve.at(self.maplibre_zoom(zoom)).max(0.0) as f32 * self.pixel_ratio)
    }
}

/// Why a style couldn't be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleError(
    /// A message naming the offending field (e.g. `rules[2]: ...`).
    pub String,
);

/// Largest accepted line `width`, in CSS pixels.
pub(crate) const MAX_LINE_WIDTH: f64 = 100.0;

impl std::fmt::Display for StyleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StyleError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_scale_follows_zoom_and_pixel_ratio() {
        let style = MapStyle::light();
        assert_eq!(style.line_scale(REFERENCE_ZOOM), 1.0);
        assert!((style.line_scale(REFERENCE_ZOOM + 2.0) - 2.0).abs() < 1e-6);
        assert_eq!(style.line_scale(2.0), ZOOM_WIDTH_FACTOR.0 as f32);
        let retina = style.with_pixel_ratio(2.0);
        assert!((retina.line_scale(REFERENCE_ZOOM + 1.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn empty_class_filter_accepts_everything() {
        let rule = Rule::new("water", &[], Paint::Fill(Color::BLACK));
        assert!(rule.accepts(None));
        assert!(rule.accepts(Some("lake")));
    }

    #[test]
    fn class_filter_requires_a_listed_class() {
        let rule = Rule::new("transportation", &["motorway"], Paint::Fill(Color::BLACK));
        assert!(rule.accepts(Some("motorway")));
        assert!(!rule.accepts(Some("minor")));
        assert!(!rule.accepts(None));
    }

    fn gray(v: f32) -> Color {
        Color::from_rgba(v, v, v, 1.0).unwrap()
    }

    fn curve(base: f64, step: bool) -> ColorCurve {
        ColorCurve {
            stops: vec![(10.0, gray(0.0)), (12.0, gray(1.0))],
            base,
            step,
        }
    }

    #[test]
    fn color_curves_hold_their_end_stops_outside_them() {
        let linear = curve(1.0, false);
        assert_eq!(linear.at(4.0), gray(0.0));
        assert_eq!(linear.at(10.0), gray(0.0));
        assert_eq!(linear.at(12.0), gray(1.0));
        assert_eq!(linear.at(20.0), gray(1.0));
    }

    #[test]
    fn color_curves_blend_linearly_exponentially_or_in_steps() {
        assert!((curve(1.0, false).at(11.0).red() - 0.5).abs() < 1e-6);
        // Base 2 over two zooms: (2¹ − 1) / (2² − 1) of the way at the middle.
        assert!((curve(2.0, false).at(11.0).red() - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(curve(1.0, true).at(11.9), gray(0.0));
        assert_eq!(curve(1.0, true).at(12.0), gray(1.0));
    }

    #[test]
    fn rules_draw_from_their_min_zoom_up_to_but_not_at_their_max() {
        let mut rule = Rule::new("water", &[], Paint::Fill(Color::BLACK));
        assert!(rule.draws_at(-5.0) && rule.draws_at(30.0));
        rule.zooms = Some((5.0, 10.0));
        assert!(!rule.draws_at(4.99));
        assert!(rule.draws_at(5.0));
        assert!(rule.draws_at(9.99));
        assert!(!rule.draws_at(10.0));
    }

    #[test]
    fn caps_and_joins_parse_their_own_names_only() {
        for cap in [Cap::Butt, Cap::Round, Cap::Square] {
            assert_eq!(Cap::parse(cap.name()), Some(cap));
        }
        for join in [Join::Miter, Join::Round, Join::Bevel] {
            assert_eq!(Join::parse(join.name()), Some(join));
        }
        assert_eq!(Cap::parse("Round"), None);
        assert_eq!(Join::parse("butt"), None);
    }

    #[test]
    fn data_zoom_follows_maplibre_only_when_asked() {
        let style = MapStyle::light();
        assert_eq!(style.data_zoom(14.7), None);
        let imported = MapStyle {
            maplibre_tiles: true,
            ..style
        };
        assert_eq!(imported.data_zoom(14.7), Some(13));
        assert_eq!(
            imported.clone().with_pixel_ratio(2.0).data_zoom(14.7),
            Some(12)
        );
        assert_eq!(imported.data_zoom(0.5), Some(0));
    }

    #[test]
    fn curve_widths_follow_maplibre_zoom_times_the_pixel_ratio() {
        let style = MapStyle::light().with_pixel_ratio(2.0);
        let mut rule = Rule::new(
            "transportation",
            &[],
            Paint::Line {
                color: Color::BLACK,
                width: 1.0,
            },
        );
        assert_eq!(style.curve_width(&rule, 14.0), None);
        rule.width_curve =
            Some(Curve::parse(&serde_json::json!({ "stops": [[10, 2], [14, 6]] })).unwrap());
        // World zoom 14 on a 2× canvas is MapLibre zoom 12: width 4.
        assert_eq!(style.curve_width(&rule, 14.0), Some(8.0));
    }

    #[test]
    fn a_zero_pixel_ratio_is_kept_positive() {
        let style = MapStyle::light().with_pixel_ratio(0.0);
        assert!(style.pixel_ratio > 0.0);
        assert!(style.line_scale(14.0).is_finite());
    }

    #[test]
    fn extrusion_rules_can_be_left_out() {
        let style = MapStyle::from_json(
            r##"{ "background": "#fff", "rules": [
                { "layer": "water", "fill": "#00f" },
                { "layer": "building", "fill": "#888", "extrude": { "height": "render_height" } }
            ] }"##,
        )
        .unwrap();
        assert!(style.extrudes());
        let flat = style.without_extrusions();
        assert!(!flat.extrudes());
        assert_eq!(flat.rules.len(), 1);
        assert_eq!(flat.rules[0].layer, "water");
    }
}
