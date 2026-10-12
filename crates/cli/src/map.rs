//! The background map the options ask for: its tile source and style,
//! loaded from a preset, a file, or a URL.

use std::path::Path;

use anyhow::{bail, Context, Result};
use gpx_motion_core::maplibre;
use gpx_motion_core::MapStyle;

use crate::config::{Config, MapSpec};
use crate::http;
use crate::sprites;
use crate::tiles::{MapSource, TileFetcher, VectorTileFetcher};

/// Build the background map source from `--tms-url-template` or
/// `--vector-tiles`, writing `--export-style` on the way.
pub fn map_source(config: &Config) -> Result<Option<MapSource>> {
    let cache = config.tile_cache.clone();
    let Some(spec) = &config.map else {
        return Ok(None);
    };
    let (url, max_zoom, style, export_style) = match spec {
        MapSpec::Raster { template } => {
            let fetcher = TileFetcher::new(template.clone(), cache)?;
            return Ok(Some(MapSource::Raster(fetcher)));
        }
        MapSpec::Vector {
            url,
            max_zoom,
            style,
            export_style,
        } => (url, *max_zoom, style, export_style),
    };
    let style = map_style(style)?;
    if let Some(path) = export_style {
        let json = serde_json::to_string_pretty(&style.to_json())?;
        std::fs::write(path, json + "\n").with_context(|| path.display().to_string())?;
    }
    let style = style.with_pixel_ratio(config.pixel_ratio);
    let tiles = VectorTileFetcher::new(url, max_zoom, cache)?;
    Ok(Some(MapSource::Vector { tiles, style }))
}

/// A preset by name, else a JSON style from that URL or file, printing
/// what an imported MapLibre style left out.
fn map_style(spec: &str) -> Result<MapStyle> {
    if let Some(style) = MapStyle::preset(spec) {
        return Ok(style);
    }
    let text = if spec.starts_with("http://") || spec.starts_with("https://") {
        http::fetch_text(spec).context("loading map style")?
    } else {
        let path = Path::new(spec);
        if !path.exists() {
            let looks_like_path = spec.contains(['/', '\\']) || spec.ends_with(".json");
            if looks_like_path {
                bail!("{spec}: style file not found");
            }
            bail!("unknown map style `{spec}` (light, dark, or a JSON style file or URL)");
        }
        std::fs::read_to_string(path).with_context(|| spec.to_string())?
    };
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .with_context(|| format!("{spec}: invalid JSON"))?;
    let (style, notes) = if maplibre::is_maplibre(&json) {
        // Patterns are drawn in their average color, from the style's sprite.
        let names = maplibre::pattern_names(&json);
        let patterns = if names.is_empty() {
            maplibre::PatternColors::new()
        } else {
            sprites::pattern_colors(&json, &names, http::fetch_bytes)
        };
        maplibre::from_maplibre_with(&json, &patterns)
    } else {
        MapStyle::from_json_with_notes(&text)
    }
    .with_context(|| spec.to_string())?;
    for note in notes {
        eprintln!("warning: map style: {note}");
    }
    Ok(style)
}
