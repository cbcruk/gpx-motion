//! Label-free map styles for the OpenMapTiles vector schema.

use tiny_skia::Color;

/// How a matched feature is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    /// Fill polygon features.
    Fill(Color),
    /// Stroke line-string features at a fixed pixel width.
    Line { color: Color, width: f32 },
}

/// Draws the features of one source layer, optionally narrowed by `class`.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub layer: String,
    /// Accepted `class` property values; empty matches every feature.
    pub classes: Vec<String>,
    pub paint: Paint,
}

impl Rule {
    fn new(layer: &str, classes: &[&str], paint: Paint) -> Self {
        Self {
            layer: layer.to_string(),
            classes: classes.iter().map(|c| c.to_string()).collect(),
            paint,
        }
    }

    /// Whether a feature with this `class` value passes the filter.
    pub fn accepts(&self, class: Option<&str>) -> bool {
        self.classes.is_empty() || class.is_some_and(|c| self.classes.iter().any(|k| k == c))
    }
}

/// A background color plus rules drawn in order (later rules paint on top).
#[derive(Debug, Clone, PartialEq)]
pub struct MapStyle {
    pub background: Color,
    pub rules: Vec<Rule>,
}

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
        }
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;

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
