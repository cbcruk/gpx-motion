//! Import of MapLibre (Mapbox GL) style JSON into a [`MapStyle`]: the
//! `background`, `fill`, and `line` layers of an OpenMapTiles-schema style,
//! with their filters, zoom ranges, colors, opacities, widths, and dashes.
//! Everything else is skipped and reported, never silently dropped.

use std::collections::BTreeMap;

use serde_json::Value as Json;
use tiny_skia::Color;

use super::expr::{Curve, Filter};
use super::offset::OffsetCurve;
use super::style::{
    Cap, ColorCurve, Join, MapStyle, Paint, Rule, StyleError, MAX_LINE_WIDTH, REFERENCE_ZOOM,
};

/// MapLibre zoom at which a color curve's single value is taken (the
/// background's, and each paint's own color beside its curve): the
/// reference zoom of gpx-motion widths, in MapLibre's 512 px tiles.
const COLOR_ZOOM: f64 = REFERENCE_ZOOM - 1.0;

/// Whether `json` looks like a MapLibre style (a `version` and `layers`).
pub fn is_maplibre(json: &Json) -> bool {
    json.get("version").is_some() && json.get("layers").is_some_and(Json::is_array)
}

/// Convert a MapLibre style, returning the style and what was skipped or
/// approximated (one message per kind).
pub fn from_maplibre(json: &Json) -> Result<(MapStyle, Vec<String>), StyleError> {
    let layers = json
        .get("layers")
        .and_then(Json::as_array)
        .ok_or_else(|| StyleError("a MapLibre style needs a `layers` array".into()))?;
    let mut notes = Notes::default();
    let mut background = None;
    let mut rules = Vec::new();
    for layer in layers {
        let id = layer.get("id").and_then(Json::as_str).unwrap_or("?");
        let kind = layer.get("type").and_then(Json::as_str).unwrap_or("?");
        let hidden = layer
            .pointer("/layout/visibility")
            .and_then(Json::as_str)
            .is_some_and(|v| v == "none");
        if hidden {
            continue;
        }
        let result = match kind {
            "background" if background.is_none() => {
                background = Some(background_color(layer, &mut notes));
                continue;
            }
            "background" => continue,
            "fill" | "line" | "fill-extrusion" => rules_of(layer, kind, &mut notes),
            other => {
                notes.skipped_type(other);
                continue;
            }
        };
        match result {
            Ok(layer_rules) => rules.extend(layer_rules),
            Err(reason) => notes.skipped_layer(id, &reason),
        }
    }
    let background = background.unwrap_or_else(|| {
        notes.note("no background layer; using white");
        Color::WHITE
    });
    let style = MapStyle {
        background,
        rules,
        pixel_ratio: 1.0,
        maplibre_tiles: true,
    };
    Ok((style, notes.finish()))
}

/// The first background layer's color, made opaque (white if unreadable).
fn background_color(layer: &Json, notes: &mut Notes) -> Color {
    if layer.pointer("/paint/background-opacity").is_some() {
        notes.ignored("background-opacity");
    }
    let color = match layer.pointer("/paint/background-color") {
        None => Color::BLACK,
        Some(c) => match color_at(c, COLOR_ZOOM) {
            Ok(color) => color,
            Err(e) => {
                notes.note(&format!("background: {e}; using white"));
                Color::WHITE
            }
        },
    };
    Color::from_rgba(color.red(), color.green(), color.blue(), 1.0).unwrap_or(Color::WHITE)
}

/// Paint properties drawn as they are, per layer type.
const FILL_PAINT: &[&str] = &[
    "fill-color",
    "fill-opacity",
    "fill-outline-color",
    "fill-antialias",
    "fill-translate",
    "fill-translate-anchor",
];
const LINE_PAINT: &[&str] = &[
    "line-color",
    "line-opacity",
    "line-width",
    "line-dasharray",
    "line-blur",
    "line-gap-width",
];
const EXTRUSION_PAINT: &[&str] = &["fill-extrusion-color", "fill-extrusion-opacity"];
/// Properties whose presence makes the layer unrepresentable.
const UNSUPPORTED: &[&str] = &[
    "fill-pattern",
    "line-pattern",
    "line-gradient",
    "fill-extrusion-pattern",
];
/// Layout properties honored (besides `visibility`).
const LAYOUT: &[&str] = &["visibility", "line-cap", "line-join"];
/// Largest dash or gap accepted, in line widths.
const MAX_DASH: f64 = 1000.0;

/// The rules drawing a `fill`, `line`, or `fill-extrusion` layer: one, plus
/// a 1 CSS px outline rule around the polygons of a fill with an explicit
/// `fill-outline-color` (which MapLibre draws only when antialiasing).
fn rules_of(layer: &Json, kind: &str, notes: &mut Notes) -> Result<Vec<Rule>, String> {
    let main = rule(layer, kind, notes)?;
    let antialiased = layer.pointer("/paint/fill-antialias") != Some(&Json::Bool(false));
    let outline = layer
        .pointer("/paint/fill-outline-color")
        .filter(|_| kind == "fill" && antialiased);
    let Some(outline) = outline else {
        return Ok(vec![main]);
    };
    let (color, colors) = paint_color(outline)?;
    // Only the polygons' outlines: a fill layer ignores line features.
    let polygons = serde_json::json!(["==", ["geometry-type"], "Polygon"]);
    let filter = match &main.filter {
        Some(f) => serde_json::json!(["all", polygons, f.json().clone()]),
        None => polygons,
    };
    let edge = Rule {
        filter: Some(Filter::parse(&filter)?),
        paint: Paint::Line { color, width: 1.0 },
        width_curve: Some(Curve::constant(1.0)),
        outlines: true,
        cap: Some(Cap::Butt),
        join: Some(Join::Miter),
        colors,
        ..main.clone()
    };
    Ok(vec![main, edge])
}

/// A color, or a zoom curve of colors with its color at [`COLOR_ZOOM`].
fn paint_color(json: &Json) -> Result<(Color, Option<ColorCurve>), String> {
    if json.is_string() {
        return Ok((color_at(json, COLOR_ZOOM)?, None));
    }
    let curve = color_curve(json)?;
    Ok((curve.at(COLOR_ZOOM), Some(curve)))
}

fn rule(layer: &Json, kind: &str, notes: &mut Notes) -> Result<Rule, String> {
    let source_layer = layer
        .get("source-layer")
        .and_then(Json::as_str)
        .ok_or("no `source-layer`")?;
    let paint = layer.get("paint").and_then(Json::as_object);
    let get = |key: &str| paint.and_then(|p| p.get(key));
    let drawn = match kind {
        "fill" => FILL_PAINT,
        "line" => LINE_PAINT,
        _ => EXTRUSION_PAINT,
    };
    for (key, _) in paint.into_iter().flatten() {
        if UNSUPPORTED.contains(&key.as_str()) {
            return Err(format!("`{key}` isn't supported"));
        }
        // Offsets are taken along the map's axes, as with the default anchor.
        let viewport_anchor =
            key == "fill-translate-anchor" && get(key) == Some(&Json::from("viewport"));
        if !drawn.contains(&key.as_str()) || viewport_anchor {
            notes.ignored(key);
        }
    }
    let layout = layer.get("layout").and_then(Json::as_object);
    for (key, _) in layout.into_iter().flatten() {
        if !LAYOUT.contains(&key.as_str()) {
            notes.ignored(key);
        }
    }
    let filter = match layer.get("filter") {
        None | Some(Json::Null) => None,
        Some(f) => Some(Filter::parse(f)?),
    };
    let zoom = |key: &str| match layer.get(key) {
        None => Ok(None),
        Some(z) => z
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a number")),
    };
    let (min, max) = (zoom("minzoom")?, zoom("maxzoom")?);
    let zooms = (min.is_some() || max.is_some()).then(|| {
        (
            min.unwrap_or(f64::NEG_INFINITY),
            max.unwrap_or(f64::INFINITY),
        )
    });
    // A color that changes with zoom follows its curve; the paint keeps its
    // value at the reference zoom moved into the range where the layer draws.
    let color_zoom = COLOR_ZOOM
        .max(min.unwrap_or(f64::NEG_INFINITY))
        .min(max.map_or(f64::INFINITY, |m| m - 0.5));
    let mut colors = None;
    let mut color = |key: &str| -> Result<Color, String> {
        let Some(value) = get(key) else {
            return Ok(Color::BLACK);
        };
        if value.is_string() {
            return color_at(value, color_zoom);
        }
        let curve = color_curve(value)?;
        let at = curve.at(color_zoom);
        colors = Some(curve);
        Ok(at)
    };
    let opacity = |key: &str| -> Result<Option<Curve>, String> {
        get(key)
            .map(|v| Curve::parse(v).map(|c| c.map(|o| o.clamp(0.0, 1.0))))
            .transpose()
    };
    let mut rule = Rule {
        filter,
        zooms,
        ..Rule::new(source_layer, &[], Paint::Fill(Color::BLACK))
    };
    match kind {
        "fill" => {
            rule.paint = Paint::Fill(color("fill-color")?);
            rule.opacity = opacity("fill-opacity")?;
            let antialias = get("fill-antialias").map(Curve::parse_switch).transpose()?;
            rule.antialias = antialias.filter(|a| a.values().any(|v| v == 0.0));
            let translate = get("fill-translate").map(OffsetCurve::parse).transpose()?;
            rule.translate = translate.filter(|t| !t.is_zero());
        }
        "fill-extrusion" => {
            notes.note_once("3D extrusions (fill-extrusion) are drawn as flat fills");
            rule.paint = Paint::Fill(color("fill-extrusion-color")?);
            rule.opacity = opacity("fill-extrusion-opacity")?;
        }
        _ => {
            let width = get("line-width").map_or(Ok(Curve::constant(1.0)), Curve::parse)?;
            if width.values().any(|w| !w.is_finite()) {
                return Err("`line-width` isn't finite".into());
            }
            rule.width_curve = Some(width.map(|w| w.clamp(0.0, MAX_LINE_WIDTH)));
            rule.paint = Paint::Line {
                color: color("line-color")?,
                width: 1.0,
            };
            rule.opacity = opacity("line-opacity")?;
            let pixels = |key: &str| -> Result<Option<Curve>, String> {
                let curve = get(key).map(Curve::parse).transpose()?;
                Ok(curve
                    .map(|c| c.map(|v| v.clamp(0.0, MAX_LINE_WIDTH)))
                    .filter(|c| c.values().any(|v| v > 0.0)))
            };
            rule.blur = pixels("line-blur")?;
            rule.gap = pixels("line-gap-width")?;
            rule.dash = get("line-dasharray").map(dash).transpose()?;
            rule.outlines = true;
            let cap = layout
                .and_then(|l| l.get("line-cap"))
                .map_or(Some(Cap::Butt), |c| c.as_str().and_then(Cap::parse));
            rule.cap = Some(cap.ok_or("unsupported `line-cap`")?);
            let join = layout
                .and_then(|l| l.get("line-join"))
                .map_or(Some(Join::Miter), |j| j.as_str().and_then(Join::parse));
            rule.join = Some(join.ok_or("unsupported `line-join`")?);
        }
    }
    rule.colors = colors;
    Ok(rule)
}

/// A constant dash array (`[dash, gap, ...]` or `["literal", [...]]`).
fn dash(json: &Json) -> Result<Vec<f32>, String> {
    let values = match json.as_array().map(Vec::as_slice) {
        Some([Json::String(op), inner]) if op == "literal" => inner,
        _ => json,
    };
    values
        .as_array()
        .and_then(|a| a.iter().map(|v| v.as_f64()).collect::<Option<Vec<_>>>())
        .filter(|v| !v.is_empty() && v.iter().all(|x| (0.0..=MAX_DASH).contains(x)))
        .filter(|v| v.iter().any(|x| *x > 0.0))
        .map(|v| v.into_iter().map(|x| x as f32).collect())
        .ok_or_else(|| format!("unsupported `line-dasharray` `{json}`"))
}

/// A color, or a zoom curve of colors taken at `zoom`.
fn color_at(json: &Json, zoom: f64) -> Result<Color, String> {
    if let Some(text) = json.as_str() {
        return css_color(text).ok_or_else(|| format!("unsupported color `{text}`"));
    }
    Ok(color_curve(json)?.at(zoom))
}

/// A zoom curve of colors: legacy `{"stops": [[zoom, color], ...], "base"}`,
/// `["interpolate", <linear | exponential>, ["zoom"], zoom, color, ...]`
/// (also `interpolate-hcl`, blended in RGB here), or
/// `["step", ["zoom"], color, zoom, color, ...]`.
pub(super) fn color_curve(json: &Json) -> Result<ColorCurve, String> {
    let bad = || format!("unsupported color `{json}`");
    if let Some(items) = json
        .as_array()
        .filter(|i| i.first() == Some(&Json::from("step")))
    {
        if items.len() < 3 || items.len() % 2 == 0 || items[1] != serde_json::json!(["zoom"]) {
            return Err(bad());
        }
        let first = css_color(items[2].as_str().ok_or_else(bad)?).ok_or_else(bad)?;
        let mut stops = vec![(f64::NEG_INFINITY, first)];
        for p in items[3..].chunks(2) {
            let z = p[0].as_f64().ok_or_else(bad)?;
            let c = css_color(p[1].as_str().ok_or_else(bad)?).ok_or_else(bad)?;
            stops.push((z, c));
        }
        if stops.windows(2).any(|w| w[0].0 >= w[1].0) {
            return Err(bad());
        }
        return Ok(ColorCurve {
            stops,
            base: 1.0,
            step: true,
        });
    }
    let (stops, base): (Vec<(f64, &Json)>, f64) = if let Some(object) = json.as_object() {
        if object.contains_key("property") || object.get("type").is_some_and(|t| t != "exponential")
        {
            return Err(bad());
        }
        let stops = object
            .get("stops")
            .and_then(Json::as_array)
            .ok_or_else(bad)?
            .iter()
            .map(|s| Some((s.get(0)?.as_f64()?, s.get(1)?)))
            .collect::<Option<_>>()
            .ok_or_else(bad)?;
        (
            stops,
            object.get("base").and_then(Json::as_f64).unwrap_or(1.0),
        )
    } else {
        let items = json.as_array().ok_or_else(bad)?;
        let zoomed = items.len() >= 5
            && matches!(items[0].as_str(), Some("interpolate" | "interpolate-hcl"))
            && items[2] == serde_json::json!(["zoom"]);
        if !zoomed {
            return Err(bad());
        }
        let base = match items[1].as_array().map(Vec::as_slice) {
            Some([Json::String(k), b]) if k == "exponential" => b.as_f64().ok_or_else(bad)?,
            _ => 1.0,
        };
        let stops = items[3..]
            .chunks(2)
            .map(|p| Some((p.first()?.as_f64()?, p.get(1)?)))
            .collect::<Option<_>>()
            .ok_or_else(bad)?;
        (stops, base)
    };
    if stops.is_empty() || stops.windows(2).any(|w| w[0].0 >= w[1].0) {
        return Err(bad());
    }
    let stops = stops
        .into_iter()
        .map(|(z, c)| Some((z, css_color(c.as_str()?)?)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(bad)?;
    Ok(ColorCurve {
        stops,
        base,
        step: false,
    })
}

/// Parse a CSS color: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`,
/// `rgba()`, `hsl()`, `hsla()`, or a common name.
pub fn css_color(text: &str) -> Option<Color> {
    let text = text.trim().to_ascii_lowercase();
    if let Some(hex) = text.strip_prefix('#') {
        let digits: Vec<f32> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as f32))
            .collect::<Option<_>>()?;
        let pair = |i: usize| (digits[i] * 16.0 + digits[i + 1]) / 255.0;
        let single = |i: usize| digits[i] * 17.0 / 255.0;
        return match digits.len() {
            3 => Color::from_rgba(single(0), single(1), single(2), 1.0),
            4 => Color::from_rgba(single(0), single(1), single(2), single(3)),
            6 => Color::from_rgba(pair(0), pair(2), pair(4), 1.0),
            8 => Color::from_rgba(pair(0), pair(2), pair(4), pair(6)),
            _ => None,
        };
    }
    if let Some((name, args)) = text.strip_suffix(')').and_then(|t| t.split_once('(')) {
        let parts: Vec<&str> = args
            .split([',', ' ', '/'])
            .filter(|p| !p.is_empty())
            .collect();
        let number = |p: &str| -> Option<f32> {
            match p.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().ok().map(|v| v / 100.0),
                None => p.parse().ok(),
            }
        };
        let alpha = match parts.get(3) {
            Some(a) => number(a)?.clamp(0.0, 1.0),
            None => 1.0,
        };
        return match (name.trim(), parts.len()) {
            ("rgb" | "rgba", 3 | 4) => {
                let channel = |p: &str| -> Option<f32> {
                    if p.ends_with('%') {
                        number(p)
                    } else {
                        p.parse::<f32>().ok().map(|v| v / 255.0)
                    }
                };
                let (r, g, b) = (channel(parts[0])?, channel(parts[1])?, channel(parts[2])?);
                Color::from_rgba(
                    r.clamp(0.0, 1.0),
                    g.clamp(0.0, 1.0),
                    b.clamp(0.0, 1.0),
                    alpha,
                )
            }
            ("hsl" | "hsla", 3 | 4) => {
                let h = parts[0].trim_end_matches("deg").parse::<f32>().ok()?;
                let (s, l) = (number(parts[1])?, number(parts[2])?);
                let (r, g, b) = hsl(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
                Color::from_rgba(r, g, b, alpha)
            }
            _ => None,
        };
    }
    let (r, g, b, a) = match text.as_str() {
        "transparent" => (0, 0, 0, 0),
        "black" => (0, 0, 0, 255),
        "white" => (255, 255, 255, 255),
        "gray" | "grey" => (128, 128, 128, 255),
        "silver" => (192, 192, 192, 255),
        "red" => (255, 0, 0, 255),
        "green" => (0, 128, 0, 255),
        "blue" => (0, 0, 255, 255),
        "yellow" => (255, 255, 0, 255),
        "orange" => (255, 165, 0, 255),
        "purple" => (128, 0, 128, 255),
        "navy" => (0, 0, 128, 255),
        "teal" => (0, 128, 128, 255),
        "olive" => (128, 128, 0, 255),
        "maroon" => (128, 0, 0, 255),
        "lime" => (0, 255, 0, 255),
        "aqua" | "cyan" => (0, 255, 255, 255),
        "fuchsia" | "magenta" => (255, 0, 255, 255),
        _ => return None,
    };
    Some(Color::from_rgba8(r, g, b, a))
}

/// RGB (`0..=1`) of a hue in degrees and saturation/lightness in `0..=1`.
fn hsl(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r + m, g + m, b + m)
}

/// What the import skipped or ignored, grouped so each kind is one message.
#[derive(Default)]
struct Notes {
    types: BTreeMap<String, usize>,
    /// Skipped layer ids by reason.
    layers: BTreeMap<String, Vec<String>>,
    ignored: BTreeMap<String, usize>,
    other: Vec<String>,
}

impl Notes {
    fn skipped_type(&mut self, kind: &str) {
        *self.types.entry(kind.to_string()).or_default() += 1;
    }

    fn skipped_layer(&mut self, id: &str, reason: &str) {
        self.layers
            .entry(reason.to_string())
            .or_default()
            .push(id.to_string());
    }

    fn ignored(&mut self, property: &str) {
        *self.ignored.entry(property.to_string()).or_default() += 1;
    }

    fn note(&mut self, text: &str) {
        self.other.push(text.to_string());
    }

    fn note_once(&mut self, text: &str) {
        if !self.other.iter().any(|t| t == text) {
            self.note(text);
        }
    }

    fn finish(self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.types.is_empty() {
            let kinds: Vec<String> = self.types.iter().map(|(k, n)| format!("{n} {k}")).collect();
            let labels = if self.types.contains_key("symbol") {
                " (map labels come from --labels instead)"
            } else {
                ""
            };
            out.push(format!("skipped layers: {}{labels}", kinds.join(", ")));
        }
        for (reason, ids) in self.layers {
            let shown: Vec<String> = ids.iter().take(5).map(|id| format!("`{id}`")).collect();
            let more = match ids.len() {
                n if n > 5 => format!(" and {} more", n - 5),
                _ => String::new(),
            };
            let layers = if ids.len() == 1 { "layer" } else { "layers" };
            out.push(format!(
                "skipped {layers} {}{more}: {reason}",
                shown.join(", ")
            ));
        }
        if !self.ignored.is_empty() {
            let props: Vec<String> = self
                .ignored
                .iter()
                .map(|(k, n)| format!("{k} ({n})"))
                .collect();
            out.push(format!("ignored properties: {}", props.join(", ")));
        }
        out.extend(self.other);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_css_colors() {
        let rgba = |c: Color| c.to_color_u8();
        let c = |t| rgba(css_color(t).unwrap());
        assert_eq!(c("#abc"), rgba(Color::from_rgba8(0xaa, 0xbb, 0xcc, 0xff)));
        assert_eq!(c("#11223344").alpha(), 0x44);
        assert_eq!(
            c("rgb(10, 20, 30)"),
            rgba(Color::from_rgba8(10, 20, 30, 255))
        );
        assert_eq!(c("rgba(10,20,30,0.5)").alpha(), 128);
        assert_eq!(
            c("hsl(0, 100%, 50%)"),
            rgba(Color::from_rgba8(255, 0, 0, 255))
        );
        assert_eq!(
            c("hsla(120, 100%, 25%, 1)"),
            rgba(Color::from_rgba8(0, 128, 0, 255))
        );
        assert_eq!(c("White"), rgba(Color::WHITE));
        assert!(css_color("chartreuse-ish").is_none());
    }

    #[test]
    fn converts_fill_and_line_layers() {
        let style = json!({
            "version": 8,
            "sources": {},
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#102030"}},
                {"id": "water", "type": "fill", "source": "t", "source-layer": "water",
                 "paint": {"fill-color": "rgba(0, 0, 255, 0.5)", "fill-outline-color": "#000"}},
                {"id": "roads", "type": "line", "source": "t", "source-layer": "transportation",
                 "minzoom": 12,
                 "filter": ["all", ["==", ["get", "class"], "primary"], ["!=", ["get", "brunnel"], "tunnel"]],
                 "paint": {"line-color": "#fff", "line-opacity": 0.5,
                           "line-width": ["interpolate", ["exponential", 1.2], ["zoom"], 10, 1, 20, 10],
                           "line-dasharray": [2, 1]}},
                {"id": "labels", "type": "symbol", "source": "t", "source-layer": "place"},
                {"id": "pattern", "type": "fill", "source": "t", "source-layer": "park",
                 "paint": {"fill-pattern": "dots"}},
                {"id": "hidden", "type": "line", "source": "t", "source-layer": "x",
                 "layout": {"visibility": "none"}}
            ]
        });
        assert!(is_maplibre(&style));
        let (style, notes) = from_maplibre(&style).unwrap();
        assert_eq!(
            style.background.to_color_u8(),
            Color::from_rgba8(0x10, 0x20, 0x30, 255).to_color_u8()
        );
        assert_eq!(style.rules.len(), 3);
        assert_eq!(style.rules[0].layer, "water");
        assert!(matches!(style.rules[0].paint, Paint::Fill(c) if (c.alpha() - 0.5).abs() < 0.01));
        // The fill's outline color: a 1 px line around its polygons.
        let edge = &style.rules[1];
        assert_eq!(edge.layer, "water");
        assert!(edge.outlines);
        assert!(matches!(edge.paint, Paint::Line { color, .. } if color == Color::BLACK));
        assert_eq!(edge.width_curve.as_ref().map(|w| w.at(15.0)), Some(1.0));
        let filter = edge.filter.as_ref().map(|f| f.json().to_string());
        assert!(
            filter.is_some_and(|f| f.contains("geometry-type")),
            "polygons only"
        );
        let road = &style.rules[2];
        assert_eq!(road.zooms, Some((12.0, f64::INFINITY)));
        assert_eq!(road.dash.as_deref(), Some(&[2.0, 1.0][..]));
        assert_eq!(road.cap, Some(Cap::Butt));
        assert_eq!(road.join, Some(Join::Miter), "MapLibre's default");
        assert_eq!(style.data_zoom(14.6), Some(13), "MapLibre's own tile zoom");
        assert_eq!(
            style.clone().with_pixel_ratio(2.0).data_zoom(14.6),
            Some(12)
        );
        assert_eq!(MapStyle::light().data_zoom(14.6), None);
        assert!(road.outlines);
        assert!(matches!(road.paint, Paint::Line { width, .. } if width == 1.0));
        assert_eq!(road.opacity.as_ref().map(|o| o.at(13.0)), Some(0.5));
        assert!(road.width_curve.is_some() && road.filter.is_some());
        let joined = notes.join("\n");
        assert!(joined.contains("1 symbol"), "{joined}");
        assert!(joined.contains("`pattern`: `fill-pattern`"), "{joined}");
        assert!(!joined.contains("fill-outline-color"), "{joined}");
        assert!(!joined.contains("hidden"), "{joined}");
        let exported = style.to_json();
        let round_trip = MapStyle::from_json(&exported.to_string()).unwrap();
        assert_eq!(round_trip.to_json(), exported);
    }

    #[test]
    fn colors_follow_zoom_and_joins_and_antialias_carry_over() {
        let style = json!({
            "version": 8,
            "sources": {},
            "layers": [
                {"id": "land", "type": "fill", "source": "t", "source-layer": "landuse",
                 "paint": {"fill-antialias": true,
                           "fill-color": ["interpolate", ["linear"], ["zoom"], 10, "#000000", 14, "#ffffff"]}},
                {"id": "road", "type": "line", "source": "t", "source-layer": "transportation",
                 "layout": {"line-join": "round"},
                 "paint": {"line-color": {"base": 2, "stops": [[10, "#000"], [12, "#fff"]]}}}
            ]
        });
        let (style, notes) = from_maplibre(&style).unwrap();
        let gray = |z: f64| style.rules[0].colors.as_ref().map(|c| c.at(z).red());
        assert_eq!(gray(9.0), Some(0.0));
        assert!(
            (gray(12.0).unwrap() - 0.5).abs() < 1e-6,
            "halfway at zoom 12"
        );
        assert_eq!(gray(15.0), Some(1.0));
        let road = &style.rules[1];
        assert_eq!(road.join, Some(Join::Round));
        // Exponential base 2: a third of the way at zoom 11.
        let at11 = road.colors.as_ref().unwrap().at(11.0).red();
        assert!((at11 - 1.0 / 3.0).abs() < 1e-6, "{at11}");
        let joined = notes.join("\n");
        assert!(!joined.contains("antialias"), "{joined}");
        // No antialiasing, no outline (as MapLibre draws it in that pass).
        let plain = json!({"version": 8, "sources": {}, "layers": [
            {"id": "a", "type": "fill", "source": "t", "source-layer": "park",
             "paint": {"fill-antialias": false, "fill-color": "#0f0", "fill-outline-color": "#000"}}]});
        assert_eq!(from_maplibre(&plain).unwrap().0.rules.len(), 1);
        assert!(!joined.contains("one zoom"), "{joined}");
        let exported = style.to_json();
        let round_trip = MapStyle::from_json(&exported.to_string()).unwrap();
        assert_eq!(round_trip.to_json(), exported);
        assert_eq!(round_trip.rules[1].colors, style.rules[1].colors);
    }

    #[test]
    fn antialias_translate_blur_gap_and_step_colors_carry_over() {
        let style = json!({
            "version": 8,
            "sources": {},
            "layers": [
                {"id": "wood", "type": "fill", "source": "t", "source-layer": "landcover",
                 "paint": {"fill-antialias": ["step", ["zoom"], false, 9, true],
                           "fill-translate": ["interpolate", ["linear"], ["zoom"],
                                              14, ["literal", [0, 0]], 16, ["literal", [-2, -2]]],
                           "fill-translate-anchor": "map",
                           "fill-color": ["step", ["zoom"], "#000000", 12, "#ffffff"]}},
                {"id": "border", "type": "line", "source": "t", "source-layer": "boundary",
                 "paint": {"line-blur": ["interpolate", ["linear"], ["zoom"], 0, 0.4, 22, 4],
                           "line-gap-width": 3, "line-color": "#888"}}
            ]
        });
        let (style, notes) = from_maplibre(&style).unwrap();
        let joined = notes.join("\n");
        for key in ["antialias", "translate", "blur", "gap"] {
            assert!(!joined.contains(key), "{key}: {joined}");
        }
        let wood = &style.rules[0];
        let aa = wood.antialias.as_ref().unwrap();
        assert_eq!((aa.at(8.0), aa.at(9.0)), (0.0, 1.0));
        assert_eq!(wood.translate.as_ref().unwrap().at(15.0), (-1.0, -1.0));
        let colors = wood.colors.as_ref().unwrap();
        assert_eq!((colors.at(11.9).red(), colors.at(12.0).red()), (0.0, 1.0));
        let border = &style.rules[1];
        assert!((border.blur.as_ref().unwrap().at(11.0) - 2.2).abs() < 1e-9);
        assert_eq!(border.gap.as_ref().unwrap().at(5.0), 3.0);
        // A viewport anchor isn't followed, and says so.
        let viewport = json!({"version": 8, "sources": {}, "layers": [
            {"id": "a", "type": "fill", "source": "t", "source-layer": "park",
             "paint": {"fill-translate": [1, 1], "fill-translate-anchor": "viewport"}}]});
        assert!(from_maplibre(&viewport)
            .unwrap()
            .1
            .join("\n")
            .contains("fill-translate-anchor"));
        // Defaults stay off the rule.
        let plain = json!({"version": 8, "sources": {}, "layers": [
            {"id": "a", "type": "fill", "source": "t", "source-layer": "park",
             "paint": {"fill-antialias": true, "fill-translate": [0, 0]}}]});
        let rule = &from_maplibre(&plain).unwrap().0.rules[0];
        assert_eq!((&rule.antialias, &rule.translate), (&None, &None));
        let exported = style.to_json();
        let round_trip = MapStyle::from_json(&exported.to_string()).unwrap();
        assert_eq!(round_trip.to_json(), exported);
        assert_eq!(round_trip.rules, style.rules);
    }
}
