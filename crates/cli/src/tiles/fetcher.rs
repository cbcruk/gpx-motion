use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpx_motion_core::{TileId, TilePlacement};
use reqwest::blocking::Client;
use tiny_skia::{IntSize, Pixmap};

/// Tiles older than this on disk are refetched.
const DISK_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Fetches and decodes map tiles, with an in-memory cache for the run and an
/// optional on-disk cache (24h TTL) shared across runs.
pub struct TileFetcher {
    client: Client,
    template: String,
    extension: String,
    cache: HashMap<TileId, Pixmap>,
    disk: Option<PathBuf>,
}

impl TileFetcher {
    pub fn new(template: String, disk: Option<PathBuf>) -> Result<Self, Box<dyn Error>> {
        let client = Client::builder()
            .user_agent("gpx-motion/0.0 (+https://github.com/gpx-animator)")
            .timeout(Duration::from_secs(30))
            .build()?;
        let extension = template
            .rsplit('.')
            .next()
            .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("tile")
            .to_string();
        Ok(Self {
            client,
            template,
            extension,
            cache: HashMap::new(),
            disk,
        })
    }

    /// Fetch the given placements, returning those that decoded successfully.
    ///
    /// Individual tile failures are logged and skipped so a few missing tiles
    /// don't abort the whole render.
    pub fn fetch_all(&mut self, placements: &[TilePlacement]) -> Vec<(TilePlacement, Pixmap)> {
        let mut out = Vec::with_capacity(placements.len());
        for &placement in placements {
            match self.tile(placement.id) {
                Ok(pixmap) => out.push((placement, pixmap)),
                Err(e) => eprintln!(
                    "warning: tile {}/{}/{} skipped: {e}",
                    placement.id.zoom, placement.id.x, placement.id.y
                ),
            }
        }
        out
    }

    fn tile(&mut self, id: TileId) -> Result<Pixmap, Box<dyn Error>> {
        if let Some(cached) = self.cache.get(&id) {
            return Ok(cached.clone());
        }
        let bytes = self.tile_bytes(id)?;
        let pixmap = decode(&bytes)?;
        self.cache.insert(id, pixmap.clone());
        Ok(pixmap)
    }

    /// Raw tile bytes from a fresh disk cache entry, otherwise from HTTP (which
    /// then populates the disk cache).
    pub fn tile_bytes(&self, id: TileId) -> Result<Vec<u8>, Box<dyn Error>> {
        let path = self.disk_path(id);
        if let Some(path) = &path {
            if let Some(bytes) = read_if_fresh(path) {
                return Ok(bytes);
            }
        }

        let url = self
            .template
            .replace("{zoom}", &id.zoom.to_string())
            .replace("{z}", &id.zoom.to_string())
            .replace("{x}", &id.x.to_string())
            .replace("{y}", &id.y.to_string());
        let bytes = self
            .client
            .get(&url)
            .send()?
            .error_for_status()?
            .bytes()?
            .to_vec();

        if let Some(path) = &path {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, &bytes);
        }
        Ok(bytes)
    }

    fn disk_path(&self, id: TileId) -> Option<PathBuf> {
        let root = self.disk.as_ref()?;
        Some(root.join(format!("{}/{}/{}.{}", id.zoom, id.x, id.y, self.extension)))
    }
}

fn read_if_fresh(path: &std::path::Path) -> Option<Vec<u8>> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age <= DISK_TTL {
        fs::read(path).ok()
    } else {
        None
    }
}

fn decode(bytes: &[u8]) -> Result<Pixmap, Box<dyn Error>> {
    let rgba = image::load_from_memory(bytes)?.to_rgba8();
    let size = IntSize::from_wh(rgba.width(), rgba.height()).ok_or("tile has zero dimension")?;
    Pixmap::from_vec(rgba.into_raw(), size).ok_or_else(|| "tile buffer size mismatch".into())
}
