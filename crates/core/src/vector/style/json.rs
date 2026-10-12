//! Parsing and writing gpx-motion style JSON, and resolving style specs.

use tiny_skia::Color;

use crate::color::{parse_hex, to_hex};
use crate::vector::expr::{Curve, Filter};
use crate::vector::extrusion::Extrusion;
use crate::vector::offset::OffsetCurve;

use super::model::{Cap, ColorCurve, Join, MapStyle, Paint, Rule, StyleError, MAX_LINE_WIDTH};

impl ColorCurve {
    fn to_json(&self) -> serde_json::Value {
        if self.step {
            let mut items = vec![
                serde_json::json!("step"),
                serde_json::json!(["zoom"]),
                to_hex(self.stops[0].1).into(),
            ];
            for &(z, c) in &self.stops[1..] {
                items.extend([serde_json::json!(z), to_hex(c).into()]);
            }
            return serde_json::Value::Array(items);
        }
        let stops: Vec<serde_json::Value> = self
            .stops
            .iter()
            .map(|&(z, c)| serde_json::json!([z, to_hex(c)]))
            .collect();
        serde_json::json!({ "base": self.base, "stops": stops })
    }
}

impl MapStyle {
    /// The style as gpx-motion style JSON (see [`MapStyle::from_json`]),
    /// e.g. to inspect or edit what a MapLibre style became.
    pub fn to_json(&self) -> serde_json::Value {
        let rules: Vec<serde_json::Value> = self.rules.iter().map(rule_json).collect();
        let mut out = serde_json::json!({
            "background": to_hex(self.background),
            "rules": rules,
        });
        if self.maplibre_tiles {
            out["tiles"] = "maplibre".into();
        }
        out
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
        if crate::vector::maplibre::is_maplibre(&json) {
            return crate::vector::maplibre::from_maplibre(&json);
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
    /// Colors are `#rgb`, `#rgba`, `#rrggbb`, or `#rrggbbaa`. Rules draw in order, so
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
        known_keys(object, &["extends", "background", "rules", "tiles"])?;
        let maplibre_tiles = match object.get("tiles") {
            None => None,
            Some(v) if v == "maplibre" => Some(true),
            Some(v) if v == "default" => Some(false),
            Some(_) => return Err(StyleError("`tiles` must be `default` or `maplibre`".into())),
        };
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
                maplibre_tiles: false,
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
        if let Some(maplibre_tiles) = maplibre_tiles {
            style.maplibre_tiles = maplibre_tiles;
        }
        Ok(style)
    }
}

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

/// Keys a rule may have.
const RULE_KEYS: &[&str] = &[
    "layer",
    "classes",
    "fill",
    "line",
    "width",
    "dash",
    "filter",
    "minzoom",
    "maxzoom",
    "opacity",
    "outlines",
    "cap",
    "join",
    "colors",
    "antialias",
    "translate",
    "blur",
    "gap",
    "extrude",
];

type JsonObject = serde_json::Map<String, serde_json::Value>;

fn rule(json: &serde_json::Value) -> Result<Rule, StyleError> {
    let object = json
        .as_object()
        .ok_or_else(|| StyleError("a rule must be an object".into()))?;
    known_keys(object, RULE_KEYS)?;
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
    let (paint, width_curve) = rule_paint(object)?;
    let dash = field(object, "dash", dash)?;
    let filter = field(object, "filter", |f| {
        Filter::parse(f).map_err(|e| StyleError(format!("`filter`: {e}")))
    })?;
    let zooms = zoom_range(object)?;
    let opacity = field(object, "opacity", |o| {
        Curve::parse(o)
            .ok()
            .filter(|c| c.values().all(|v| (0.0..=1.0).contains(&v)))
            .ok_or_else(|| StyleError("`opacity` must be a number or zoom curve in [0, 1]".into()))
    })?;
    let outlines = field(object, "outlines", |o| {
        o.as_bool()
            .ok_or_else(|| StyleError("`outlines` must be true or false".into()))
    })?
    .unwrap_or(false);
    let cap = field(object, "cap", |c| {
        c.as_str()
            .and_then(Cap::parse)
            .ok_or_else(|| StyleError("`cap` must be `butt`, `round`, or `square`".into()))
    })?;
    let join = field(object, "join", |j| {
        j.as_str()
            .and_then(Join::parse)
            .ok_or_else(|| StyleError("`join` must be `miter`, `round`, or `bevel`".into()))
    })?;
    let colors = field(object, "colors", |c| {
        crate::vector::maplibre::color_curve(c).map_err(|e| StyleError(format!("`colors`: {e}")))
    })?;
    let antialias = field(object, "antialias", |a| {
        Curve::parse_switch(a).map_err(|e| StyleError(format!("`antialias`: {e}")))
    })?;
    let translate = field(object, "translate", |t| {
        OffsetCurve::parse(t).map_err(|e| StyleError(format!("`translate`: {e}")))
    })?;
    let (blur, gap) = (pixels(object, "blur")?, pixels(object, "gap")?);
    let extrude = field(object, "extrude", |e| {
        Extrusion::parse(e).map_err(|e| StyleError(format!("`extrude`: {e}")))
    })?;
    Ok(Rule {
        layer: layer.to_string(),
        classes,
        paint,
        filter,
        width_curve,
        dash,
        zooms,
        opacity,
        outlines,
        cap,
        join,
        colors,
        antialias,
        translate,
        blur,
        gap,
        extrude,
    })
}

/// The rule's `key` parsed by `parse`, or `None` when absent.
fn field<T>(
    object: &JsonObject,
    key: &str,
    parse: impl FnOnce(&serde_json::Value) -> Result<T, StyleError>,
) -> Result<Option<T>, StyleError> {
    object.get(key).map(parse).transpose()
}

/// A rule's `fill` or `line` paint, with its zoom curve of line widths when
/// `width` is one.
fn rule_paint(object: &JsonObject) -> Result<(Paint, Option<Curve>), StyleError> {
    match (object.get("fill"), object.get("line")) {
        (Some(_), None)
            if ["width", "dash", "outlines", "cap", "join", "blur", "gap"]
                .iter()
                .any(|k| object.contains_key(*k)) =>
        {
            Err(StyleError(
                "`width`, `dash`, `outlines`, `cap`, `join`, `blur`, and `gap` only apply to \
                 `line` rules"
                    .into(),
            ))
        }
        (None, Some(_)) if object.contains_key("antialias") || object.contains_key("extrude") => {
            Err(StyleError(
                "`antialias` and `extrude` only apply to `fill` rules".into(),
            ))
        }
        (Some(fill), None) => Ok((Paint::Fill(color(fill, "fill")?), None)),
        (None, Some(line)) => line_paint(object, line),
        _ => Err(StyleError(
            "a rule needs exactly one of `fill` or `line`".into(),
        )),
    }
}

/// A `line` rule's paint in color `line`, with its `width` zoom curve if it
/// has one.
fn line_paint(
    object: &JsonObject,
    line: &serde_json::Value,
) -> Result<(Paint, Option<Curve>), StyleError> {
    let bad = || {
        StyleError(format!(
            "a `line` rule needs a `width` in (0, {MAX_LINE_WIDTH}], or a zoom curve \
             of widths in [0, {MAX_LINE_WIDTH}]"
        ))
    };
    let mut width_curve = None;
    let width = object.get("width").ok_or_else(bad)?;
    let width = match width.as_f64() {
        Some(w) => Some(w as f32)
            .filter(|w| *w > 0.0 && *w as f64 <= MAX_LINE_WIDTH)
            .ok_or_else(bad)?,
        None => {
            let curve = Curve::parse(width).map_err(|_| bad())?;
            if !curve.values().all(|w| (0.0..=MAX_LINE_WIDTH).contains(&w)) {
                return Err(bad());
            }
            width_curve = Some(curve);
            1.0
        }
    };
    let paint = Paint::Line {
        color: color(line, "line")?,
        width,
    };
    Ok((paint, width_curve))
}

/// A rule's `dash` pattern.
fn dash(d: &serde_json::Value) -> Result<Vec<f32>, StyleError> {
    d.as_array()
        .and_then(|a| a.iter().map(|v| v.as_f64().map(|v| v as f32)).collect())
        .filter(|v: &Vec<f32>| !v.is_empty() && v.iter().all(|x| *x >= 0.0 && x.is_finite()))
        .filter(|v| v.iter().any(|x| *x > 0.0))
        .ok_or_else(|| StyleError("`dash` must be an array of non-negative numbers".into()))
}

/// A rule's `minzoom`..`maxzoom`, or `None` when it has neither.
fn zoom_range(object: &JsonObject) -> Result<Option<(f64, f64)>, StyleError> {
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
    Ok((min.is_finite() || max.is_finite()).then_some((min, max)))
}

/// A rule's `key` in CSS pixels, as a number or zoom curve.
fn pixels(object: &JsonObject, key: &str) -> Result<Option<Curve>, StyleError> {
    field(object, key, |v| {
        Curve::parse(v)
            .ok()
            .filter(|c| c.values().all(|v| (0.0..=MAX_LINE_WIDTH).contains(&v)))
            .ok_or_else(|| {
                StyleError(format!(
                    "`{key}` must be a number or zoom curve in [0, {MAX_LINE_WIDTH}]"
                ))
            })
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
            out.insert("fill".into(), to_hex(c).into());
        }
        Paint::Line { color, width } => {
            out.insert("line".into(), to_hex(color).into());
            let width = match &rule.width_curve {
                Some(curve) => curve.to_json(),
                None => serde_json::Value::from(width as f64),
            };
            out.insert("width".into(), width);
        }
    }
    if let Some(opacity) = &rule.opacity {
        out.insert("opacity".into(), opacity.to_json());
    }
    if rule.outlines {
        out.insert("outlines".into(), true.into());
    }
    if let Some(cap) = rule.cap {
        out.insert("cap".into(), cap.name().into());
    }
    if let Some(join) = rule.join {
        out.insert("join".into(), join.name().into());
    }
    if let Some(colors) = &rule.colors {
        out.insert("colors".into(), colors.to_json());
    }
    if let Some(antialias) = &rule.antialias {
        out.insert("antialias".into(), antialias.switch_json());
    }
    if let Some(translate) = &rule.translate {
        out.insert("translate".into(), translate.to_json());
    }
    if let Some(blur) = &rule.blur {
        out.insert("blur".into(), blur.to_json());
    }
    if let Some(gap) = &rule.gap {
        out.insert("gap".into(), gap.to_json());
    }
    if let Some(extrude) = &rule.extrude {
        out.insert("extrude".into(), extrude.to_json());
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

/// Parse a `#rgb`, `#rgba`, `#rrggbb`, or `#rrggbbaa` field.
fn color(json: &serde_json::Value, field: &str) -> Result<Color, StyleError> {
    json.as_str().and_then(parse_hex).ok_or_else(|| {
        StyleError(format!(
            "`{field}` must be a #rgb, #rgba, #rrggbb, or #rrggbbaa color"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::super::presets::rgb;
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
        assert!(err(
            r##"{"background": "#000", "rules": [{"layer": "x", "line": "#fff", "width": 1, "extrude": {}}]}"##
        )
        .contains("`extrude` only apply"));
        assert!(err(
            r##"{"background": "#000", "rules": [{"layer": "x", "fill": "#fff", "extrude": {"height": "a", "base": -2}}]}"##
        )
        .contains("`extrude`: `base`"));
        assert!(err(r##"{"background": "#ff000000"}"##).contains("opaque"));
        assert!(MapStyle::from_json("\u{feff}{\"extends\": \"dark\"}").is_ok());
    }
}
