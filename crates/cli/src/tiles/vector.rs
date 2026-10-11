use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use gpx_motion_core::tiles::pmtiles::{decompress, Compression, MAX_TILE_BYTES};
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
        if let Some(location) = archive_location(url) {
            let (mut archive, name) = match location {
                ArchiveLocation::File(path) => {
                    (PmTilesReader::open(&path)?, path.display().to_string())
                }
                ArchiveLocation::Url(url) => (PmTilesReader::open_url(&url, http_client()?)?, url),
            };
            if let Some(layers) = archive.layer_names() {
                if !layers.iter().any(|l| l == "transportation") {
                    eprintln!(
                        "warning: {name} has no `transportation` layer; the map styles expect the \
                         OpenMapTiles schema (e.g. an archive built with planetiler)"
                    );
                }
            }
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
                    .and_then(|bytes| Ok(decompress(&bytes, Compression::Unknown, MAX_TILE_BYTES)?))
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

/// The local path when `url` names a PMTiles archive (`.pmtiles`, any case,
/// optionally as a `file://` URL); an error for remote archives, which aren't
/// supported yet; `None` for other sources.
/// Where a PMTiles archive lives.
enum ArchiveLocation {
    File(PathBuf),
    Url(String),
}

/// The archive `url` names, if it ends in `.pmtiles`: an `http(s)` URL, a
/// `file://` URL, or a plain path.
fn archive_location(url: &str) -> Option<ArchiveLocation> {
    if !url.to_ascii_lowercase().ends_with(".pmtiles") {
        return None;
    }
    if let Some(path) = url.strip_prefix("file://") {
        return Some(ArchiveLocation::File(PathBuf::from(path)));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(ArchiveLocation::Url(url.to_string()));
    }
    Some(ArchiveLocation::File(PathBuf::from(url)))
}

fn http_client() -> Result<reqwest::blocking::Client, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .user_agent("gpx-motion/0.0 (+https://github.com/gpx-animator)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

fn resolve_tilejson(url: &str, fallback_zoom: u32) -> Result<(String, u32), Box<dyn Error>> {
    let body = http_client()?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_archives() {
        let file = |url: &str| match archive_location(url) {
            Some(ArchiveLocation::File(path)) => Some(path),
            _ => None,
        };
        assert_eq!(file("a/b.pmtiles"), Some(PathBuf::from("a/b.pmtiles")));
        assert_eq!(file("MAP.PMTILES"), Some(PathBuf::from("MAP.PMTILES")));
        assert_eq!(
            file("file:///x/y.pmtiles"),
            Some(PathBuf::from("/x/y.pmtiles"))
        );
        assert!(matches!(
            archive_location("https://example.com/x.pmtiles"),
            Some(ArchiveLocation::Url(url)) if url == "https://example.com/x.pmtiles"
        ));
        assert!(archive_location("https://tiles.openfreemap.org/planet").is_none());
    }
}
