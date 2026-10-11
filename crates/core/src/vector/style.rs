//! Label-free map styles for the OpenMapTiles vector schema.

use tiny_skia::Color;

use super::expr::{Curve, Filter};
use super::mvt::{Feature, Layer};

/// How a matched feature is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    /// Fill polygon features.
    Fill(Color),
    /// Stroke line-string features at a fixed pixel width.
    Line { color: Color, width: f32 },
}

/// Draws the features of one source layer, optionally narrowed by `class`
/// and a MapLibre filter.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub layer: String,
    /// Accepted `class` property values; empty matches every feature.
    pub classes: Vec<String>,
    pub paint: Paint,
    /// A MapLibre filter features must also pass.
    pub filter: Option<Filter>,
    /// Line width in CSS pixels by MapLibre zoom, replacing the line paint's
    /// width (which is then `1`) and its zoom scaling.
    pub width_curve: Option<Curve>,
    /// Dash pattern in multiples of the line width (dash, gap, ...).
    pub dash: Option<Vec<f32>>,
    /// MapLibre zooms `[min, max)` at which the rule draws.
    pub zooms: Option<(f64, f64)>,
}

impl Rule {
    fn new(layer: &str, classes: &[&str], paint: Paint) -> Self {
        Self {
            layer: layer.to_string(),
            classes: classes.iter().map(|c| c.to_string()).collect(),
            paint,
            filter: None,
            width_curve: None,
            dash: None,
            zooms: None,
        }
    }

    /// Whether a feature with this `class` value passes the filter.
    pub fn accepts(&self, class: Option<&str>) -> bool {
        self.classes.is_empty() || class.is_some_and(|c| self.classes.iter().any(|k| k == c))
    }

    /// Whether `feature` of `layer` passes the class list and the filter.
    pub fn matches(&self, layer: &Layer, feature: &Feature) -> bool {
        let class = layer.property(feature, "class").and_then(|v| v.as_str());
        self.accepts(class)
            && self
                .filter
                .as_ref()
                .is_none_or(|f| f.accepts(layer, feature))
    }

    /// Whether the rule draws at MapLibre zoom `zoom`.
    pub fn draws_at(&self, zoom: f64) -> bool {
        self.zooms
            .is_none_or(|(min, max)| zoom >= min && zoom < max)
    }
}

/// A background color plus rules drawn in order (later rules paint on top).
///
/// Line widths are in CSS pixels at [`REFERENCE_ZOOM`]; see
/// [`MapStyle::line_scale`] for how they scale with zoom and pixel ratio.
#[derive(Debug, Clone, PartialEq)]
pub struct MapStyle {
    pub background: Color,
    pub rules: Vec<Rule>,
    /// Device pixels per CSS pixel (e.g. `2` on a retina screen or for a
    /// high-resolution export).
    pub pixel_ratio: f32,
}

/// Zoom at which style line widths apply unscaled.
pub const REFERENCE_ZOOM: f64 = 14.0;
/// Range of the zoom-based line width factor.
const ZOOM_WIDTH_FACTOR: (f64, f64) = (0.35, 3.0);

/// Road classes from minor to major, each a separate width tier.
const MINOR_ROADS: &[&str] = &["minor", "service"];
const MID_ROADS: &[&str] = &["secondary", "tertiary"];
const MAJOR_ROADS: &[&str] = &["primary", "trunk"];
const PATHS: &[&str] = &["path", "track"];

/// Colors that vary between presets; geometry and layer order are shared.
struct Palette {
    background: Color,
    landcover: Color,
    park: Color,
    water: Color,
    building: Color,
    rail: Color,
    path: Color,
    casing: Option<Color>,
    minor: Color,
    mid: Color,
    major: Color,
    motorway: Color,
}

impl MapStyle {
    /// A light, OSM-like basemap.
    pub fn light() -> Self {
        Self::from_palette(Palette {
            background: rgb(0xf2, 0xef, 0xe9),
            landcover: rgb(0xdd, 0xe8, 0xcf),
            park: rgb(0xd4, 0xe6, 0xc4),
            water: rgb(0xaa, 0xd3, 0xdf),
            building: rgb(0xdf, 0xd9, 0xd1),
            rail: rgb(0xb8, 0xb3, 0xad),
            path: rgb(0xcf, 0xc5, 0xba),
            casing: Some(rgb(0xd0, 0xc8, 0xbe)),
            minor: rgb(0xff, 0xff, 0xff),
            mid: rgb(0xff, 0xff, 0xff),
            major: rgb(0xfc, 0xe0, 0xb4),
            motorway: rgb(0xf4, 0xb6, 0x8a),
        })
    }

    /// A muted dark basemap that keeps a bright track in front.
    pub fn dark() -> Self {
        Self::from_palette(Palette {
            background: rgb(0x1e, 0x1e, 0x1e),
            landcover: rgb(0x22, 0x28, 0x22),
            park: rgb(0x23, 0x2c, 0x24),
            water: rgb(0x1b, 0x2a, 0x38),
            building: rgb(0x2a, 0x2a, 0x2c),
            rail: rgb(0x3a, 0x3a, 0x3c),
            path: rgb(0x30, 0x30, 0x32),
            casing: None,
            minor: rgb(0x36, 0x36, 0x38),
            mid: rgb(0x42, 0x42, 0x45),
            major: rgb(0x4e, 0x4e, 0x52),
            motorway: rgb(0x5c, 0x5c, 0x60),
        })
    }

    /// The same style for a canvas with `ratio` device pixels per CSS pixel.
    pub fn with_pixel_ratio(mut self, ratio: f32) -> Self {
        self.pixel_ratio = ratio.max(f32::EPSILON);
        self
    }

    /// Multiplier for style line widths at a world zoom: `√2` per zoom level
    /// away from [`REFERENCE_ZOOM`] (clamped), times the pixel ratio.
    ///
    /// The zoom is first taken back to CSS pixels, so a 2× canvas (fitted one
    /// zoom level deeper) draws the same roads twice as wide rather than
    /// three times.
    pub fn line_scale(&self, zoom: f64) -> f32 {
        let css_zoom = zoom - (self.pixel_ratio as f64).log2();
        let factor = 2f64
            .powf((css_zoom - REFERENCE_ZOOM) / 2.0)
            .clamp(ZOOM_WIDTH_FACTOR.0, ZOOM_WIDTH_FACTOR.1);
        self.pixel_ratio * factor as f32
    }

    /// The MapLibre zoom matching world `zoom` on this style's canvas:
    /// MapLibre counts 512 px tiles in CSS pixels, so it is one level below
    /// the 256 px world zoom taken back to CSS pixels.
    pub fn maplibre_zoom(&self, zoom: f64) -> f64 {
        zoom - (self.pixel_ratio as f64).log2() - 1.0
    }

    /// Stroke width in device pixels of a line `rule` at world `zoom`, for a
    /// paint width of `1` (see [`Rule::width_curve`]); `None` for rules
    /// without a curve, whose width scales by [`MapStyle::line_scale`].
    pub fn curve_width(&self, rule: &Rule, zoom: f64) -> Option<f32> {
        let curve = rule.width_curve.as_ref()?;
        Some(curve.at(self.maplibre_zoom(zoom)).max(0.0) as f32 * self.pixel_ratio)
    }

    /// The style as gpx-motion style JSON (see [`MapStyle::from_json`]),
    /// e.g. to inspect or edit what a MapLibre style became.
    pub fn to_json(&self) -> serde_json::Value {
        let rules: Vec<serde_json::Value> = self.rules.iter().map(rule_json).collect();
        serde_json::json!({
            "background": hex(self.background),
            "rules": rules,
        })
    }

    /// A built-in style by name (`light` or `dark`), or a style parsed from
    /// JSON when `spec` starts with `{`; see [`MapStyle::from_json`].
    pub fn resolve(spec: &str) -> Result<Self, StyleError> {
        Self::resolve_with_notes(spec).map(|(style, _)| style)
    }

    /// [`MapStyle::resolve`], also returning what an imported MapLibre style
    /// skipped or approximated (see [`MapStyle::from_json_with_notes`]).
    pub fn resolve_with_notes(spec: &str) -> Result<(Self, Vec<String>), StyleError> {
        if spec.trim_start().starts_with('{') {
            return Self::from_json_with_notes(spec);
        }
        Self::preset(spec).map(|s| (s, Vec::new())).ok_or_else(|| {
            StyleError(format!(
                "unknown map style `{spec}` (light, dark, or a JSON style)"
            ))
        })
    }

    /// [`MapStyle::from_json`], also returning what an imported MapLibre
    /// style skipped or approximated (empty for gpx-motion styles).
    pub fn from_json_with_notes(text: &str) -> Result<(Self, Vec<String>), StyleError> {
        let text = text.trim_start_matches('\u{feff}');
        let json: serde_json::Value =
            serde_json::from_str(text).map_err(|e| StyleError(format!("invalid JSON: {e}")))?;
        if super::maplibre::is_maplibre(&json) {
            return super::maplibre::from_maplibre(&json);
        }
        Self::from_value(&json).map(|s| (s, Vec::new()))
    }

    /// Parse a style from JSON.
    ///
    /// ```json
    /// {
    ///   "extends": "dark",
    ///   "background": "#101418",
    ///   "rules": [
    ///     { "layer": "water", "fill": "#1b2a38" },
    ///     { "layer": "transportation", "classes": ["primary", "trunk"],
    ///       "line": "#e0a060", "width": 3 }
    ///   ]
    /// }
    /// ```
    ///
    /// - `extends` (optional) starts from a preset's background and rules.
    /// - `background` (opaque) is required unless the style extends a preset.
    /// - `rules`, when present, replace all of the preset's rules. Each names
    ///   an OpenMapTiles `layer`, optionally narrows it by `class` with
    ///   `classes`, and either `fill`s its polygons or strokes its lines with
    ///   `line` and `width` (CSS pixels at zoom 14, `0 < width <= 100`; see
    ///   [`MapStyle::line_scale`]).
    ///
    /// Colors are `#rgb`, `#rrggbb`, or `#rrggbbaa`. Rules draw in order, so
    /// later rules paint on top. Unknown keys are errors, so typos surface.
    ///
    /// A line rule's `width` may also be a zoom curve (MapLibre `interpolate`
    /// or `step` on `["zoom"]`, or `{"stops": ..., "base": ...}`) in CSS pixels
    /// by MapLibre zoom, which replaces the default zoom scaling. Rules may
    /// also carry a MapLibre `filter`, a `dash` pattern (multiples of the
    /// width), and `minzoom`/`maxzoom` (MapLibre zooms).
    ///
    /// A MapLibre style document (with `version` and `layers`) is imported
    /// instead: its background, fill, and line layers become rules; see
    /// [`MapStyle::from_json_with_notes`] for what it leaves out.
    pub fn from_json(text: &str) -> Result<Self, StyleError> {
        Self::from_json_with_notes(text).map(|(style, _)| style)
    }

    fn from_value(json: &serde_json::Value) -> Result<Self, StyleError> {
        let object = json
            .as_object()
            .ok_or_else(|| StyleError("a style must be a JSON object".into()))?;
        known_keys(object, &["extends", "background", "rules"])?;
        let base = match object.get("extends") {
            None => None,
            Some(name) => {
                let name = name
                    .as_str()
                    .ok_or_else(|| StyleError("`extends` must be a preset name".into()))?;
                Some(Self::preset(name).ok_or_else(|| {
                    StyleError(format!("`extends`: unknown preset `{name}` (light, dark)"))
                })?)
            }
        };
        let background = object
            .get("background")
            .map(|v| color(v, "background"))
            .transpose()?;
        if background.is_some_and(|c| c.alpha() < 1.0) {
            return Err(StyleError("`background` must be opaque".into()));
        }
        let rules = object
            .get("rules")
            .map(|v| {
                v.as_array()
                    .ok_or_else(|| StyleError("`rules` must be an array".into()))?
                    .iter()
                    .enumerate()
                    .map(|(i, r)| rule(r).map_err(|e| StyleError(format!("rules[{i}]: {}", e.0))))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let mut style = match (base, background) {
            (Some(base), background) => Self {
                background: background.unwrap_or(base.background),
                ..base
            },
            (None, Some(background)) => Self {
                background,
                rules: Vec::new(),
                pixel_ratio: 1.0,
            },
            (None, None) => {
                return Err(StyleError(
                    "`background` is required unless the style `extends` a preset".into(),
                ))
            }
        };
        if let Some(rules) = rules {
            style.rules = rules;
        }
        Ok(style)
    }

    /// Look up a built-in style by name (`light` or `dark`).
    pub fn preset(name: &str) -> Option<Self> {
        match name {
            "light" => Some(Self::light()),
            "dark" => Some(Self::dark()),
            _ => None,
        }
    }

    fn from_palette(p: Palette) -> Self {
        let line = |color, width| Paint::Line { color, width };
        let mut rules = vec![
            Rule::new("landcover", &[], Paint::Fill(p.landcover)),
            Rule::new("park", &[], Paint::Fill(p.park)),
            Rule::new(
                "landuse",
                &["cemetery", "grass", "pitch", "playground", "stadium"],
                Paint::Fill(p.park),
            ),
            Rule::new("water", &[], Paint::Fill(p.water)),
            Rule::new("waterway", &[], line(p.water, 1.2)),
            Rule::new("building", &[], Paint::Fill(p.building)),
            Rule::new("transportation", PATHS, line(p.path, 0.8)),
            Rule::new("transportation", &["rail", "transit"], line(p.rail, 1.0)),
        ];
        if let Some(casing) = p.casing {
            rules.extend([
                Rule::new("transportation", MINOR_ROADS, line(casing, 2.6)),
                Rule::new("transportation", MID_ROADS, line(casing, 3.6)),
                Rule::new("transportation", MAJOR_ROADS, line(casing, 4.4)),
                Rule::new("transportation", &["motorway"], line(casing, 5.0)),
            ]);
        }
        rules.extend([
            Rule::new("transportation", MINOR_ROADS, line(p.minor, 1.6)),
            Rule::new("transportation", MID_ROADS, line(p.mid, 2.4)),
            Rule::new("transportation", MAJOR_ROADS, line(p.major, 3.0)),
            Rule::new("transportation", &["motorway"], line(p.motorway, 3.4)),
        ]);
        Self {
            background: p.background,
            rules,
            pixel_ratio: 1.0,
        }
    }
}

/// Why a style couldn't be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleError(
    /// A message naming the offending field (e.g. `rules[2]: ...`).
    pub String,
);

/// Largest accepted line `width`, in CSS pixels.
const MAX_LINE_WIDTH: f32 = 100.0;

fn known_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    known: &[&str],
) -> Result<(), StyleError> {
    match object.keys().find(|k| !known.contains(&k.as_str())) {
        Some(key) => Err(StyleError(format!(
            "unknown key `{key}` (expected {})",
            known.join(", ")
        ))),
        None => Ok(()),
    }
}

impl std::fmt::Display for StyleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StyleError {}

fn rule(json: &serde_json::Value) -> Result<Rule, StyleError> {
    let object = json
        .as_object()
        .ok_or_else(|| StyleError("a rule must be an object".into()))?;
    known_keys(
        object,
        &[
            "layer", "classes", "fill", "line", "width", "dash", "filter", "minzoom", "maxzoom",
        ],
    )?;
    let layer = object
        .get("layer")
        .and_then(|v| v.as_str())
        .ok_or_else(|| StyleError("`layer` (a string) is required".into()))?;
    let classes: Vec<String> = match object.get("classes") {
        None => Vec::new(),
        Some(v) => v
            .as_array()
            .and_then(|a| a.iter().map(|c| c.as_str().map(str::to_string)).collect())
            .ok_or_else(|| StyleError("`classes` must be an array of strings".into()))?,
    };
    let mut width_curve = None;
    let paint = match (object.get("fill"), object.get("line")) {
        (Some(_), None) if object.contains_key("width") || object.contains_key("dash") => {
            return Err(StyleError(
                "`width` and `dash` only apply to `line` rules".into(),
            ))
        }
        (Some(fill), None) => Paint::Fill(color(fill, "fill")?),
        (None, Some(line)) => {
            let bad = || {
                StyleError(format!(
                    "a `line` rule needs a `width` in (0, {MAX_LINE_WIDTH}], or a zoom curve \
                     of widths in [0, {MAX_LINE_WIDTH}]"
                ))
            };
            let width = object.get("width").ok_or_else(bad)?;
            let width = match width.as_f64() {
                Some(w) => Some(w as f32)
                    .filter(|w| *w > 0.0 && *w <= MAX_LINE_WIDTH)
                    .ok_or_else(bad)?,
                None => {
                    let curve = Curve::parse(width).map_err(|_| bad())?;
                    if !curve
                        .values()
                        .all(|w| (0.0..=MAX_LINE_WIDTH as f64).contains(&w))
                    {
                        return Err(bad());
                    }
                    width_curve = Some(curve);
                    1.0
                }
            };
            Paint::Line {
                color: color(line, "line")?,
                width,
            }
        }
        _ => {
            return Err(StyleError(
                "a rule needs exactly one of `fill` or `line`".into(),
            ))
        }
    };
    let dash = object
        .get("dash")
        .map(|d| {
            d.as_array()
                .and_then(|a| a.iter().map(|v| v.as_f64().map(|v| v as f32)).collect())
                .filter(|v: &Vec<f32>| {
                    !v.is_empty() && v.iter().all(|x| *x >= 0.0 && x.is_finite())
                })
                .filter(|v| v.iter().any(|x| *x > 0.0))
                .ok_or_else(|| StyleError("`dash` must be an array of non-negative numbers".into()))
        })
        .transpose()?;
    let filter = object
        .get("filter")
        .map(|f| Filter::parse(f).map_err(|e| StyleError(format!("`filter`: {e}"))))
        .transpose()?;
    let zoom = |key: &str, default: f64| {
        object.get(key).map_or(Ok(default), |z| {
            z.as_f64()
                .ok_or_else(|| StyleError(format!("`{key}` must be a number")))
        })
    };
    let (min, max) = (
        zoom("minzoom", f64::NEG_INFINITY)?,
        zoom("maxzoom", f64::INFINITY)?,
    );
    let zooms = (min.is_finite() || max.is_finite()).then_some((min, max));
    Ok(Rule {
        layer: layer.to_string(),
        classes,
        paint,
        filter,
        width_curve,
        dash,
        zooms,
    })
}

/// A rule as gpx-motion style JSON.
fn rule_json(rule: &Rule) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    out.insert("layer".into(), rule.layer.clone().into());
    if !rule.classes.is_empty() {
        out.insert("classes".into(), rule.classes.clone().into());
    }
    match rule.paint {
        Paint::Fill(c) => {
            out.insert("fill".into(), hex(c).into());
        }
        Paint::Line { color, width } => {
            out.insert("line".into(), hex(color).into());
            let width = match &rule.width_curve {
                Some(curve) => curve.to_json(),
                None => serde_json::Value::from(width as f64),
            };
            out.insert("width".into(), width);
        }
    }
    if let Some(dash) = &rule.dash {
        out.insert(
            "dash".into(),
            dash.iter().map(|&d| d as f64).collect::<Vec<_>>().into(),
        );
    }
    if let Some(filter) = &rule.filter {
        out.insert("filter".into(), filter.json().clone());
    }
    if let Some((min, max)) = rule.zooms {
        if min.is_finite() {
            out.insert("minzoom".into(), min.into());
        }
        if max.is_finite() {
            out.insert("maxzoom".into(), max.into());
        }
    }
    out.into()
}

/// `#rrggbb`, or `#rrggbbaa` when translucent.
fn hex(c: Color) -> String {
    let c = c.to_color_u8();
    if c.alpha() == 0xff {
        format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            c.red(),
            c.green(),
            c.blue(),
            c.alpha()
        )
    }
}

/// Parse `#rgb`, `#rrggbb`, or `#rrggbbaa`.
fn color(json: &serde_json::Value, field: &str) -> Result<Color, StyleError> {
    let bad = || {
        StyleError(format!(
            "`{field}` must be a #rgb, #rrggbb, or #rrggbbaa color"
        ))
    };
    let hex = json
        .as_str()
        .and_then(|s| s.strip_prefix('#'))
        .ok_or_else(bad)?;
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    let pair = |i: usize| digits[i] * 16 + digits[i + 1];
    match digits.len() {
        3 => Ok(rgb(digits[0] * 17, digits[1] * 17, digits[2] * 17)),
        6 => Ok(rgb(pair(0), pair(2), pair(4))),
        8 => Ok(Color::from_rgba8(pair(0), pair(2), pair(4), pair(6))),
        _ => Err(bad()),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_standalone_style() {
        let style = MapStyle::from_json(
            r##"{
                "background": "#123",
                "rules": [
                    { "layer": "water", "fill": "#00ff0080" },
                    { "layer": "transportation", "classes": ["primary"], "line": "#abcdef", "width": 2.5 }
                ]
            }"##,
        )
        .unwrap();
        assert_eq!(style.background, rgb(0x11, 0x22, 0x33));
        assert_eq!(style.rules.len(), 2);
        assert_eq!(
            style.rules[0].paint,
            Paint::Fill(Color::from_rgba8(0, 0xff, 0, 0x80))
        );
        assert_eq!(style.rules[1].classes, vec!["primary".to_string()]);
        assert_eq!(
            style.rules[1].paint,
            Paint::Line {
                color: rgb(0xab, 0xcd, 0xef),
                width: 2.5
            }
        );
    }

    #[test]
    fn extends_a_preset() {
        let style =
            MapStyle::from_json(r##"{ "extends": "dark", "background": "#000" }"##).unwrap();
        assert_eq!(style.rules, MapStyle::dark().rules);
        assert_eq!(style.background, rgb(0, 0, 0));
        let replaced = MapStyle::from_json(r#"{ "extends": "light", "rules": [] }"#).unwrap();
        assert!(replaced.rules.is_empty());
        assert_eq!(replaced.background, MapStyle::light().background);
    }

    #[test]
    fn resolves_presets_and_json() {
        assert_eq!(MapStyle::resolve("dark").unwrap(), MapStyle::dark());
        assert!(MapStyle::resolve(r##" {"background": "#fff"}"##).is_ok());
        assert!(MapStyle::resolve("neon").is_err());
    }

    #[test]
    fn reports_where_a_style_is_wrong() {
        let err = |json: &str| MapStyle::from_json(json).unwrap_err().0;
        assert!(err("[]").contains("object"));
        assert!(err("{}").contains("background"));
        assert!(err(r#"{"extends": "neon"}"#).contains("neon"));
        assert!(err(r##"{"background": "#12"}"##).contains("background"));
        assert!(
            err(r##"{"background": "#000", "rules": [{"layer": "water"}]}"##)
                .starts_with("rules[0]")
        );
        assert!(
            err(r##"{"background": "#000", "rules": [{"layer": "x", "line": "#fff"}]}"##)
                .contains("width")
        );
        assert!(err("{").contains("invalid JSON"));
        assert!(err(r##"{"background": "#000", "rule": []}"##).contains("unknown key `rule`"));
        assert!(err(
            r##"{"background": "#000", "rules": [{"layer": "x", "class": [], "fill": "#fff"}]}"##
        )
        .contains("unknown key `class`"));
        assert!(err(
            r##"{"background": "#000", "rules": [{"layer": "x", "fill": "#fff", "width": 2}]}"##
        )
        .contains("only apply"));
        assert!(err(
            r##"{"background": "#000", "rules": [{"layer": "x", "line": "#fff", "width": 1e30}]}"##
        )
        .contains("width"));
        assert!(err(
            r##"{"background": "#000", "rules": [{"layer": "x", "line": "#fff", "width": 1e-50}]}"##
        )
        .contains("width"));
        assert!(err(r##"{"background": "#ff000000"}"##).contains("opaque"));
        assert!(MapStyle::from_json("\u{feff}{\"extends\": \"dark\"}").is_ok());
    }

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

    #[test]
    fn presets_resolve_by_name() {
        assert_eq!(MapStyle::preset("dark"), Some(MapStyle::dark()));
        assert!(MapStyle::preset("neon").is_none());
    }
}
