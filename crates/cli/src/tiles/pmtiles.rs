use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use gpx_motion_core::tiles::pmtiles::{
    decompress, find, parse_directory, tile_id, Entry, Header, Lookup, TileType, HEADER_LEN,
    MAX_DIRECTORY_DEPTH,
};
use gpx_motion_core::TileId;

/// Reads vector tiles from a local PMTiles v3 archive, keeping parsed
/// directories for the run.
pub struct PmTilesReader {
    file: File,
    header: Header,
    root: Vec<Entry>,
    leaves: HashMap<u64, Vec<Entry>>,
}

impl PmTilesReader {
    /// Open an archive of MVT tiles.
    pub fn open(path: &Path) -> Result<Self, Box<dyn Error>> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let mut file = File::open(path).map_err(|e| fail(&e))?;
        let mut head = [0u8; HEADER_LEN];
        file.read_exact(&mut head).map_err(|e| fail(&e))?;
        let header = Header::parse(&head).map_err(|e| fail(&e))?;
        if header.tile_type != TileType::Mvt {
            return Err(format!("{}: tiles are not vector (MVT) tiles", path.display()).into());
        }
        let mut reader = Self {
            file,
            root: Vec::new(),
            header,
            leaves: HashMap::new(),
        };
        let bytes = reader.read(reader.header.root_offset, reader.header.root_length)?;
        reader.root = parse_directory(&decompress(&bytes, reader.header.internal_compression)?)?;
        Ok(reader)
    }

    /// Highest zoom stored in the archive.
    pub fn max_zoom(&self) -> u32 {
        self.header.max_zoom as u32
    }

    /// The (decompressed) MVT bytes of tile `id`, or `None` if the archive
    /// doesn't contain it.
    pub fn tile_bytes(&mut self, id: TileId) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
        let Ok(zoom) = u8::try_from(id.zoom) else {
            return Ok(None);
        };
        let target = tile_id(zoom, id.x, id.y);
        let mut lookup = find(&self.root, target);
        for _ in 0..MAX_DIRECTORY_DEPTH {
            let Lookup::Leaf { offset, length } = lookup else {
                break;
            };
            lookup = find(self.leaf(offset, length)?, target);
        }
        match lookup {
            Lookup::Tile { offset, length } => {
                let bytes = self.read(self.header.tile_data_offset + offset, length as u64)?;
                Ok(Some(decompress(&bytes, self.header.tile_compression)?))
            }
            Lookup::Missing => Ok(None),
            Lookup::Leaf { .. } => Err("PMTiles leaf directories nest too deeply".into()),
        }
    }

    fn leaf(&mut self, offset: u64, length: u32) -> Result<&Vec<Entry>, Box<dyn Error>> {
        if !self.leaves.contains_key(&offset) {
            let bytes = self.read(self.header.leaf_offset + offset, length as u64)?;
            let entries = parse_directory(&decompress(&bytes, self.header.internal_compression)?)?;
            self.leaves.insert(offset, entries);
        }
        Ok(&self.leaves[&offset])
    }

    fn read(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Box<dyn Error>> {
        self.file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; usize::try_from(length)?];
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        let leaf = directory(&[(tile_id(1, 1, 1), a.len() as u64, b.len() as u64, 1)]);
        let root = directory(&[
            (tile_id(1, 0, 0), 0, a.len() as u64, 1),
            (tile_id(1, 1, 1), 0, leaf.len() as u64, 0),
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
        std::fs::remove_file(&path).unwrap();
        assert!(PmTilesReader::open(&path).is_err());
    }
}
