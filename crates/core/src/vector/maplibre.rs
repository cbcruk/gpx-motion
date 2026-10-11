//! Import of MapLibre (Mapbox GL) style JSON into a [`MapStyle`]: the
//! `background`, `fill`, and `line` layers of an OpenMapTiles-schema style,
//! with their filters, zoom ranges, colors, opacities, widths, and dashes.
//! Everything else is skipped and reported, never silently dropped.

use std::collections::BTreeMap;

use serde_json::Value as Json;
use tiny_skia::Color;

use super::expr::{Curve, Filter};
use super::style::{MapStyle, Paint, Rule, StyleError, REFERENCE_ZOOM};

/// MapLibre zoom at which zoom-dependent colors and opacities are taken:
/// the reference zoom of gpx-motion widths, in MapLibre's 512 px tiles.
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
                background = Some(
                    background_color(layer)
                        .map_err(|e| StyleError(format!("layer `{id}`: {e}")))?,
                );
                continue;
            }
            "background" => continue,
            "fill" | "line" => rule(layer, kind, &mut notes),
            other => {
                notes.skipped_type(other);
                continue;
            }
        };
        match result {
            Ok(rule) => rules.push(rule),
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
    };
    Ok((style, notes.finish()))
}

/// The first background layer's color, made opaque.
fn background_color(layer: &Json) -> Result<Color, String> {
    let color = match layer.pointer("/paint/background-color") {
        Some(c) => color_at(c)?,
        None => Color::BLACK,
    };
    Ok(Color::from_rgba(color.red(), color.green(), color.blue(), 1.0).unwrap_or(Color::BLACK))
}

/// Paint properties drawn as they are.
const FILL_PAINT: &[&str] = &["fill-color", "fill-opacity"];
const LINE_PAINT: &[&str] = &["line-color", "line-opacity", "line-width", "line-dasharray"];
/// Properties whose presence makes the layer unrepresentable.
const UNSUPPORTED: &[&str] = &["fill-pattern", "line-pattern", "line-gradient"];

fn rule(layer: &Json, kind: &str, notes: &mut Notes) -> Result<Rule, String> {
    let source_layer = layer
        .get("source-layer")
        .and_then(Json::as_str)
        .ok_or("no `source-layer`")?;
    let paint = layer.get("paint").and_then(Json::as_object);
    let get = |key: &str| paint.and_then(|p| p.get(key));
    for (key, _) in paint.into_iter().flatten() {
        if UNSUPPORTED.contains(&key.as_str()) {
            return Err(format!("`{key}` isn't supported"));
        }
        let drawn = if kind == "fill" {
            FILL_PAINT
        } else {
            LINE_PAINT
        };
        if !drawn.contains(&key.as_str()) {
            notes.ignored(key);
        }
    }
    let filter = layer.get("filter").map(Filter::parse).transpose()?;
    let zoom = |key: &str| layer.get(key).and_then(Json::as_f64);
    let zooms = match (zoom("minzoom"), zoom("maxzoom")) {
        (None, None) => None,
        (min, max) => Some((
            min.unwrap_or(f64::NEG_INFINITY),
            max.unwrap_or(f64::INFINITY),
        )),
    };
    let opacity = |key: &str| -> Result<f32, String> {
        get(key).map_or(Ok(1.0), |v| {
            Ok(Curve::parse(v)?.at(COLOR_ZOOM).clamp(0.0, 1.0) as f32)
        })
    };
    let color =
        |key: &str| -> Result<Color, String> { get(key).map_or(Ok(Color::BLACK), color_at) };
    let faded = |c: Color, o: f32| {
        Color::from_rgba(c.red(), c.green(), c.blue(), c.alpha() * o).unwrap_or(c)
    };
    let mut rule = Rule {
        layer: source_layer.to_string(),
        classes: Vec::new(),
        paint: Paint::Fill(Color::BLACK),
        filter,
        width_curve: None,
        dash: None,
        zooms,
    };
    if kind == "fill" {
        rule.paint = Paint::Fill(faded(color("fill-color")?, opacity("fill-opacity")?));
        return Ok(rule);
    }
    let width = get("line-width").map_or(Ok(Curve::constant(1.0)), Curve::parse)?;
    rule.width_curve = Some(width);
    rule.paint = Paint::Line {
        color: faded(color("line-color")?, opacity("line-opacity")?),
        width: 1.0,
    };
    rule.dash = get("line-dasharray").map(dash).transpose()?;
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
        .and_then(|a| {
            a.iter()
                .map(|v| v.as_f64().map(|v| v as f32))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|v| !v.is_empty() && v.iter().all(|x| *x >= 0.0) && v.iter().any(|x| *x > 0.0))
        .ok_or_else(|| format!("unsupported `line-dasharray` `{json}`"))
}

/// A color, or a zoom curve of colors taken at [`COLOR_ZOOM`].
fn color_at(json: &Json) -> Result<Color, String> {
    if let Some(text) = json.as_str() {
        return css_color(text).ok_or_else(|| format!("unsupported color `{text}`"));
    }
    let bad = || format!("unsupported color `{json}`");
    let stops: Vec<(f64, &Json)> = if let Some(stops) = json.get("stops") {
        stops
            .as_array()
            .ok_or_else(bad)?
            .iter()
            .map(|s| Some((s.get(0)?.as_f64()?, s.get(1)?)))
            .collect::<Option<_>>()
            .ok_or_else(bad)?
    } else {
        let items = json.as_array().ok_or_else(bad)?;
        let zoomed = items.len() >= 5
            && matches!(items[0].as_str(), Some("interpolate" | "interpolate-hcl"))
            && items[2] == serde_json::json!(["zoom"]);
        if !zoomed {
            return Err(bad());
        }
        items[3..]
            .chunks(2)
            .map(|p| Some((p.first()?.as_f64()?, p.get(1)?)))
            .collect::<Option<_>>()
            .ok_or_else(bad)?
    };
    let i = stops.partition_point(|s| s.0 <= COLOR_ZOOM);
    let at = |k: usize| color_at(stops[k].1);
    match (i, stops.len()) {
        (_, 0) => Err(bad()),
        (0, _) => at(0),
        (i, n) if i == n => at(n - 1),
        (i, _) => {
            let (a, b) = (at(i - 1)?, at(i)?);
            let t = ((COLOR_ZOOM - stops[i - 1].0) / (stops[i].0 - stops[i - 1].0)) as f32;
            let mix = |x: f32, y: f32| x + (y - x) * t;
            Color::from_rgba(
                mix(a.red(), b.red()),
                mix(a.green(), b.green()),
                mix(a.blue(), b.blue()),
                mix(a.alpha(), b.alpha()),
            )
            .ok_or_else(bad)
        }
    }
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
    layers: Vec<String>,
    ignored: BTreeMap<String, usize>,
    other: Vec<String>,
}

impl Notes {
    fn skipped_type(&mut self, kind: &str) {
        *self.types.entry(kind.to_string()).or_default() += 1;
    }

    fn skipped_layer(&mut self, id: &str, reason: &str) {
        self.layers.push(format!("skipped layer `{id}`: {reason}"));
    }

    fn ignored(&mut self, property: &str) {
        *self.ignored.entry(property.to_string()).or_default() += 1;
    }

    fn note(&mut self, text: &str) {
        self.other.push(text.to_string());
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
        out.extend(self.layers);
        if !self.ignored.is_empty() {
            let props: Vec<String> = self
                .ignored
                .iter()
                .map(|(k, n)| format!("{k} ({n})"))
                .collect();
            out.push(format!("ignored paint properties: {}", props.join(", ")));
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
        assert_eq!(style.rules.len(), 2);
        assert_eq!(style.rules[0].layer, "water");
        assert!(matches!(style.rules[0].paint, Paint::Fill(c) if (c.alpha() - 0.5).abs() < 0.01));
        let road = &style.rules[1];
        assert_eq!(road.zooms, Some((12.0, f64::INFINITY)));
        assert_eq!(road.dash.as_deref(), Some(&[2.0, 1.0][..]));
        assert!(
            matches!(road.paint, Paint::Line { color, width } if width == 1.0 && (color.alpha() - 0.5).abs() < 0.01)
        );
        assert!(road.width_curve.is_some() && road.filter.is_some());
        let joined = notes.join("\n");
        assert!(joined.contains("1 symbol"), "{joined}");
        assert!(joined.contains("`pattern`: `fill-pattern`"), "{joined}");
        assert!(joined.contains("fill-outline-color (1)"), "{joined}");
        assert!(!joined.contains("hidden"), "{joined}");
        let exported = style.to_json();
        let round_trip = MapStyle::from_json(&exported.to_string()).unwrap();
        assert_eq!(round_trip.to_json(), exported);
    }
}
