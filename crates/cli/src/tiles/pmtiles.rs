use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use gpx_motion_core::tiles::pmtiles::{Archive, Step, TileType, HEADER_LEN, PREFIX_LEN};
use gpx_motion_core::TileId;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_RANGE, ETAG, RANGE};
use reqwest::StatusCode;

/// Where an archive's bytes come from.
enum Bytes {
    /// A local file.
    File { file: File, len: u64 },
    /// An HTTP(S) URL read with range requests. The first response's ETag
    /// is kept to notice the archive changing mid-run.
    Http {
        client: Client,
        url: String,
        etag: Option<String>,
    },
}

impl Bytes {
    /// `length` bytes at `offset`, or fewer at the end of the archive when
    /// `partial` (for the opening prefix).
    fn read(&mut self, offset: u64, length: u64, partial: bool) -> Result<Vec<u8>, String> {
        match self {
            Bytes::File { file, len } => {
                let end = offset.checked_add(length);
                let available = len.saturating_sub(offset);
                let length = match end {
                    Some(end) if end <= *len => length,
                    _ if partial => available,
                    _ => return Err("range past the end of the file".to_string()),
                };
                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| e.to_string())?;
                let mut bytes = vec![0; length as usize];
                file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                Ok(bytes)
            }
            Bytes::Http { client, url, etag } => {
                if length == 0 {
                    return Ok(Vec::new());
                }
                let last = offset
                    .checked_add(length - 1)
                    .ok_or("range out of bounds")?;
                let response = client
                    .get(url.as_str())
                    .header(RANGE, format!("bytes={offset}-{last}"))
                    .send()
                    .map_err(|e| e.to_string())?;
                match response.status() {
                    StatusCode::PARTIAL_CONTENT => {}
                    StatusCode::OK => {
                        return Err("the server ignores HTTP range requests; download the \
                                    archive and pass the file instead"
                            .to_string())
                    }
                    StatusCode::RANGE_NOT_SATISFIABLE => {
                        return Err("range past the end of the archive".to_string())
                    }
                    status => return Err(format!("HTTP {status}")),
                }
                let tag = response
                    .headers()
                    .get(ETAG)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                match (etag.as_deref(), tag.as_deref()) {
                    (Some(old), Some(new)) if old != new => {
                        return Err("the remote archive changed while it was being read".to_string())
                    }
                    (None, Some(_)) => *etag = tag.clone(),
                    _ => {}
                }
                if response.headers().get(CONTENT_RANGE).is_none() {
                    return Err("the server sent no Content-Range for a range request".to_string());
                }
                let bytes = response.bytes().map_err(|e| e.to_string())?;
                if bytes.len() as u64 > length || (!partial && (bytes.len() as u64) < length) {
                    return Err("the server sent the wrong number of bytes".to_string());
                }
                Ok(bytes.to_vec())
            }
        }
    }
}

/// Reads vector tiles from a PMTiles v3 archive, a local file or a remote
/// URL (with HTTP range requests), keeping parsed directories for the run.
pub struct PmTilesReader {
    name: String,
    bytes: Bytes,
    archive: Archive,
}

impl PmTilesReader {
    /// Open a local archive of MVT tiles.
    pub fn open(path: &Path) -> Result<Self, Box<dyn Error>> {
        let name = path.display().to_string();
        let fail = |e: &dyn std::fmt::Display| format!("{name}: {e}");
        let file = File::open(path).map_err(|e| fail(&e))?;
        let len = file.metadata().map_err(|e| fail(&e))?.len();
        Self::start(name.clone(), Bytes::File { file, len })
    }

    /// Open a remote archive of MVT tiles over HTTP(S), which must answer
    /// range requests.
    pub fn open_url(url: &str, client: Client) -> Result<Self, Box<dyn Error>> {
        let bytes = Bytes::Http {
            client,
            url: url.to_string(),
            etag: None,
        };
        Self::start(url.to_string(), bytes)
    }

    fn start(name: String, mut bytes: Bytes) -> Result<Self, Box<dyn Error>> {
        let fail = |e: &dyn std::fmt::Display| format!("{name}: {e}");
        let prefix = bytes.read(0, PREFIX_LEN, true).map_err(|e| fail(&e))?;
        if prefix.len() < HEADER_LEN {
            return Err(fail(&"not a PMTiles archive").into());
        }
        let (offset, length) = Archive::root_range(&prefix).map_err(|e| fail(&e))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| fail(&"root out of range"))?;
        let root = if end <= prefix.len() as u64 {
            prefix[offset as usize..end as usize].to_vec()
        } else {
            bytes.read(offset, length, false).map_err(|e| fail(&e))?
        };
        let archive = Archive::new(&prefix, &root).map_err(|e| fail(&e))?;
        if archive.header().tile_type != TileType::Mvt {
            return Err(fail(&"tiles are not Mapbox Vector Tiles (MVT)").into());
        }
        Ok(Self {
            name,
            bytes,
            archive,
        })
    }

    /// Layer names listed in the archive's metadata (`vector_layers`), or
    /// `None` when the metadata is missing or unreadable.
    pub fn layer_names(&mut self) -> Option<Vec<String>> {
        let (offset, length) = self.archive.metadata_range();
        let bytes = self.bytes.read(offset, length, false).ok()?;
        self.archive.layer_names(&bytes)
    }

    /// Highest zoom stored in the archive.
    pub fn max_zoom(&self) -> u32 {
        self.archive.header().max_zoom as u32
    }

    /// The (decompressed) MVT bytes of tile `id`, or `None` if the archive
    /// doesn't contain it.
    pub fn tile_bytes(&mut self, id: TileId) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
        loop {
            match self
                .archive
                .locate(id.zoom, id.x, id.y)
                .map_err(|e| self.error(&e))?
            {
                Step::Missing => return Ok(None),
                Step::Tile { offset, length } => {
                    let bytes = self
                        .bytes
                        .read(offset, length, false)
                        .map_err(|e| self.error(&e))?;
                    let tile = self.archive.tile(&bytes).map_err(|e| self.error(&e))?;
                    return Ok(Some(tile));
                }
                Step::Leaf {
                    offset,
                    length,
                    key,
                } => {
                    let bytes = self
                        .bytes
                        .read(offset, length, false)
                        .map_err(|e| self.error(&e))?;
                    self.archive
                        .add_leaf(key, &bytes)
                        .map_err(|e| self.error(&e))?;
                }
            }
        }
    }

    fn error(&self, e: &dyn std::fmt::Display) -> String {
        format!("{}: {e}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpx_motion_core::tiles::pmtiles::tile_id;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// How the test server answers.
    #[derive(Clone, Copy)]
    enum Server {
        /// Honors ranges with a fixed ETag.
        Ranges,
        /// Ignores ranges and sends the whole file.
        Whole,
        /// Honors ranges but changes its ETag after the first request.
        Changing,
    }

    /// Serve `body` over HTTP on a local port for `requests` requests,
    /// returning the URL.
    fn serve(body: Vec<u8>, mode: Server, requests: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/map.pmtiles", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().take(requests).enumerate() {
                let mut stream = stream.unwrap();
                let mut range = None;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(r) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        let (a, b) = r.trim().split_once('-').unwrap();
                        range = Some((a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()));
                    }
                }
                let etag = match mode {
                    Server::Changing if n > 0 => "\"v2\"",
                    _ => "\"v1\"",
                };
                let (status, part, extra) = match (mode, range) {
                    (Server::Whole, _) | (_, None) => ("200 OK", &body[..], String::new()),
                    (_, Some((a, b))) => {
                        let b = b.min(body.len() - 1);
                        let extra = format!("Content-Range: bytes {a}-{b}/{}\r\n", body.len());
                        ("206 Partial Content", &body[a..=b], extra)
                    }
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nETag: {etag}\r\n{extra}\
                     Connection: close\r\n\r\n",
                    part.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(part).unwrap();
            }
        });
        url
    }

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8 & 0x7f) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    /// A directory with explicit offsets, gzip-compressed.
    fn directory(entries: &[(u64, u64, u64, u64)]) -> Vec<u8> {
        let mut raw = Vec::new();
        varint(&mut raw, entries.len() as u64);
        let mut last = 0;
        for &(id, ..) in entries {
            varint(&mut raw, id - last);
            last = id;
        }
        for &(.., run) in entries {
            varint(&mut raw, run);
        }
        for &(_, _, len, _) in entries {
            varint(&mut raw, len);
        }
        for &(_, off, ..) in entries {
            varint(&mut raw, off + 1);
        }
        gzip(&raw)
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    /// An archive with tiles `1/0/0` and `1/1/1` (in a leaf directory).
    fn archive(path: &Path) {
        let (a, b) = (gzip(b"tile-a"), gzip(b"tile-b"));
        let id = |x, y| tile_id(1, x, y).unwrap();
        let leaf = directory(&[(id(1, 1), a.len() as u64, b.len() as u64, 1)]);
        let root = directory(&[
            (id(0, 0), 0, a.len() as u64, 1),
            (id(1, 1), 0, leaf.len() as u64, 0),
        ]);
        let mut header = vec![0u8; HEADER_LEN];
        header[..7].copy_from_slice(b"PMTiles");
        header[7] = 3;
        let root_offset = HEADER_LEN as u64;
        let leaf_offset = root_offset + root.len() as u64;
        let data_offset = leaf_offset + leaf.len() as u64;
        for (at, v) in [
            (8, root_offset),
            (16, root.len() as u64),
            (40, leaf_offset),
            (48, leaf.len() as u64),
            (56, data_offset),
        ] {
            header[at..at + 8].copy_from_slice(&v.to_le_bytes());
        }
        header[97..102].copy_from_slice(&[2, 2, 1, 0, 1]);
        let mut file = File::create(path).unwrap();
        for part in [header, root, leaf, a, b] {
            file.write_all(&part).unwrap();
        }
    }

    fn archive_bytes() -> Vec<u8> {
        let path = std::env::temp_dir().join(format!(
            "gpx-motion-http-{}-{:?}.pmtiles",
            std::process::id(),
            std::thread::current().id()
        ));
        archive(&path);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        bytes
    }

    #[test]
    fn reads_remote_archives_with_range_requests() {
        let url = serve(archive_bytes(), Server::Ranges, 8);
        let mut reader = PmTilesReader::open_url(&url, Client::new()).unwrap();
        let id = |x, y| TileId { zoom: 1, x, y };
        assert_eq!(
            reader.tile_bytes(id(0, 0)).unwrap().as_deref(),
            Some(&b"tile-a"[..])
        );
        assert_eq!(
            reader.tile_bytes(id(1, 1)).unwrap().as_deref(),
            Some(&b"tile-b"[..])
        );
        assert_eq!(reader.tile_bytes(id(1, 0)).unwrap(), None);
    }

    #[test]
    fn explains_servers_without_ranges_and_changing_archives() {
        let url = serve(archive_bytes(), Server::Whole, 1);
        let err = PmTilesReader::open_url(&url, Client::new()).err().unwrap();
        assert!(
            err.to_string().contains("ignores HTTP range requests"),
            "{err}"
        );

        let url = serve(archive_bytes(), Server::Changing, 4);
        let mut reader = PmTilesReader::open_url(&url, Client::new()).unwrap();
        let err = reader
            .tile_bytes(TileId {
                zoom: 1,
                x: 0,
                y: 0,
            })
            .err()
            .unwrap();
        assert!(err.to_string().contains("changed"), "{err}");
    }

    #[test]
    fn rejects_lengths_past_the_end_of_the_file() {
        let path =
            std::env::temp_dir().join(format!("gpx-motion-bad-{}.pmtiles", std::process::id()));
        archive(&path);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[16..24].copy_from_slice(&(1u64 << 62).to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        let err = PmTilesReader::open(&path).err().unwrap().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(err.contains("past the end"), "{err}");
    }

    #[test]
    fn reads_tiles_through_root_and_leaf_directories() {
        let path = std::env::temp_dir().join(format!("gpx-motion-{}.pmtiles", std::process::id()));
        archive(&path);
        let mut reader = PmTilesReader::open(&path).unwrap();
        let id = |x, y| TileId { zoom: 1, x, y };
        assert_eq!(reader.max_zoom(), 1);
        assert_eq!(
            reader.tile_bytes(id(0, 0)).unwrap().as_deref(),
            Some(&b"tile-a"[..])
        );
        assert_eq!(
            reader.tile_bytes(id(1, 1)).unwrap().as_deref(),
            Some(&b"tile-b"[..])
        );
        assert_eq!(reader.tile_bytes(id(1, 0)).unwrap(), None);
        assert_eq!(reader.layer_names(), None);
        std::fs::remove_file(&path).unwrap();
        assert!(PmTilesReader::open(&path).is_err());
    }
}
