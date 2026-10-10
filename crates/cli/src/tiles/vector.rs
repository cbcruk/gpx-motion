use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use gpx_motion_core::tiles::pmtiles::{decompress, Compression};
use gpx_motion_core::{TileId, TilePlacement, VectorTile};

use super::pmtiles::PmTilesReader;
use super::TileFetcher;

/// Where MVT tile bytes come from.
enum Source {
    /// An XYZ template over HTTP, with the shared disk cache.
    Http(TileFetcher),
    /// A local PMTiles archive.
    Archive(PmTilesReader),
}

/// Fetches and decodes MVT tiles from an XYZ template, a TileJSON endpoint, or
/// a local PMTiles archive, caching decoded tiles for the run.
pub struct VectorTileFetcher {
    source: Source,
    /// Highest zoom the source serves; deeper views overzoom these tiles.
    pub max_zoom: u32,
    cache: HashMap<TileId, VectorTile>,
}

impl VectorTileFetcher {
    /// Resolve `url` into a tile source.
    ///
    /// A local path ending in `.pmtiles` is read as a PMTiles archive (its
    /// own max zoom applies). A URL with `{z}`/`{zoom}` placeholders is used as
    /// an XYZ template with `max_zoom`; anything else is fetched as TileJSON,
    /// whose first `tiles` template and `maxzoom` take precedence.
    pub fn new(url: &str, max_zoom: u32, disk: Option<PathBuf>) -> Result<Self, Box<dyn Error>> {
        if is_local_archive(url) {
            let archive = PmTilesReader::open(Path::new(url))?;
            return Ok(Self {
                max_zoom: archive.max_zoom(),
                source: Source::Archive(archive),
                cache: HashMap::new(),
            });
        }
        let (template, max_zoom) = if url.contains("{z}") || url.contains("{zoom}") {
            (url.to_string(), max_zoom)
        } else {
            resolve_tilejson(url, max_zoom)?
        };
        Ok(Self {
            source: Source::Http(TileFetcher::new(template, disk)?),
            max_zoom,
            cache: HashMap::new(),
        })
    }

    /// Fetch and decode the given placements, skipping (and logging) failures.
    pub fn fetch_all(&mut self, placements: &[TilePlacement]) -> Vec<(TilePlacement, &VectorTile)> {
        self.prefetch(placements);
        self.cached(placements)
    }

    /// Download and decode any of `placements` not cached yet.
    pub fn prefetch(&mut self, placements: &[TilePlacement]) {
        for placement in placements {
            let id = placement.id;
            if self.cache.contains_key(&id) {
                continue;
            }
            let bytes = match &mut self.source {
                Source::Http(fetcher) => fetcher
                    .tile_bytes(id)
                    .and_then(|bytes| Ok(decompress(&bytes, Compression::Unknown)?))
                    .map(Some),
                Source::Archive(archive) => archive.tile_bytes(id),
            };
            match bytes.and_then(|b| Ok(b.map(|b| VectorTile::decode(&b)).transpose()?)) {
                Ok(Some(tile)) => {
                    self.cache.insert(id, tile);
                }
                Ok(None) => {}
                Err(e) => eprintln!("warning: tile {}/{}/{} skipped: {e}", id.zoom, id.x, id.y),
            }
        }
    }

    /// The already-decoded tiles among `placements`.
    pub fn cached(&self, placements: &[TilePlacement]) -> Vec<(TilePlacement, &VectorTile)> {
        placements
            .iter()
            .filter_map(|p| self.cache.get(&p.id).map(|tile| (*p, tile)))
            .collect()
    }
}

/// Whether `url` names a local PMTiles file rather than an HTTP source.
fn is_local_archive(url: &str) -> bool {
    url.ends_with(".pmtiles") && !url.starts_with("http://") && !url.starts_with("https://")
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
