use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use gpx_motion_core::{TileId, TilePlacement, VectorTile};

use super::TileFetcher;

/// Fetches and decodes MVT tiles from an XYZ template or a TileJSON endpoint,
/// caching decoded tiles for the run (bytes share the raster disk cache).
pub struct VectorTileFetcher {
    source: TileFetcher,
    /// Highest zoom the source serves; deeper views overzoom these tiles.
    pub max_zoom: u32,
    cache: HashMap<TileId, VectorTile>,
}

impl VectorTileFetcher {
    /// Resolve `url` into a tile source.
    ///
    /// A URL with `{z}`/`{zoom}` placeholders is used as an XYZ template with
    /// `max_zoom`; anything else is fetched as TileJSON, whose first `tiles`
    /// template and `maxzoom` take precedence.
    pub fn new(url: &str, max_zoom: u32, disk: Option<PathBuf>) -> Result<Self, Box<dyn Error>> {
        let (template, max_zoom) = if url.contains("{z}") || url.contains("{zoom}") {
            (url.to_string(), max_zoom)
        } else {
            resolve_tilejson(url, max_zoom)?
        };
        Ok(Self {
            source: TileFetcher::new(template, disk)?,
            max_zoom,
            cache: HashMap::new(),
        })
    }

    /// Fetch and decode the given placements, skipping (and logging) failures.
    pub fn fetch_all(&mut self, placements: &[TilePlacement]) -> Vec<(TilePlacement, &VectorTile)> {
        for placement in placements {
            let id = placement.id;
            if self.cache.contains_key(&id) {
                continue;
            }
            let decoded = self
                .source
                .tile_bytes(id)
                .and_then(|bytes| Ok(VectorTile::decode(&bytes)?));
            match decoded {
                Ok(tile) => {
                    self.cache.insert(id, tile);
                }
                Err(e) => eprintln!("warning: tile {}/{}/{} skipped: {e}", id.zoom, id.x, id.y),
            }
        }
        placements
            .iter()
            .filter_map(|p| self.cache.get(&p.id).map(|tile| (*p, tile)))
            .collect()
    }
}

fn resolve_tilejson(url: &str, fallback_zoom: u32) -> Result<(String, u32), Box<dyn Error>> {
    let body = reqwest::blocking::Client::builder()
        .user_agent("gpx-motion/0.0 (+https://github.com/gpx-animator)")
        .build()?
        .get(url)
        .send()?
        .error_for_status()?
        .bytes()?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    let template = json["tiles"][0]
        .as_str()
        .ok_or("TileJSON has no `tiles` template")?
        .to_string();
    let max_zoom = json["maxzoom"].as_u64().map_or(fallback_zoom, |z| z as u32);
    Ok((template, max_zoom))
}
