//! A minimal PMTiles v3 writer for test fixtures: one uncompressed root
//! directory and gzip-compressed MVT tiles, no metadata or leaves.

use std::io::Write;

use gpx_motion_core::pmtiles::{tile_id, HEADER_LEN};

use super::mvt::varint;

const COMPRESSION_NONE: u8 = 1;
const COMPRESSION_GZIP: u8 = 2;
const TILE_TYPE_MVT: u8 = 1;

/// One tile at `z/x/y` with its MVT bytes.
pub struct Tile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub mvt: Vec<u8>,
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// The root directory for `(tile id, offset, length)` entries sorted by id,
/// each a run of one tile.
fn directory(entries: &[(u64, u64, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(&mut out, entries.len() as u64);
    let mut last = 0;
    for &(id, ..) in entries {
        varint(&mut out, id - last);
        last = id;
    }
    for _ in entries {
        varint(&mut out, 1);
    }
    for &(_, _, length) in entries {
        varint(&mut out, length);
    }
    for &(_, offset, _) in entries {
        varint(&mut out, offset + 1);
    }
    out
}

/// The bytes of an archive holding `tiles`.
pub fn archive(tiles: &[Tile]) -> Vec<u8> {
    let mut sorted: Vec<(u64, Vec<u8>)> = tiles
        .iter()
        .map(|t| (tile_id(t.z, t.x, t.y).unwrap(), gzip(&t.mvt)))
        .collect();
    sorted.sort_by_key(|(id, _)| *id);

    let mut entries = Vec::new();
    let mut data = Vec::new();
    for (id, bytes) in &sorted {
        entries.push((*id, data.len() as u64, bytes.len() as u64));
        data.extend_from_slice(bytes);
    }
    let root = directory(&entries);

    let root_offset = HEADER_LEN as u64;
    let metadata_offset = root_offset + root.len() as u64;
    let data_offset = metadata_offset;
    let count = sorted.len() as u64;
    let min_zoom = tiles.iter().map(|t| t.z).min().unwrap_or(0);
    let max_zoom = tiles.iter().map(|t| t.z).max().unwrap_or(0);

    let mut header = vec![0u8; HEADER_LEN];
    header[..7].copy_from_slice(b"PMTiles");
    header[7] = 3;
    for (at, value) in [
        (8, root_offset),
        (16, root.len() as u64),
        (24, metadata_offset),
        (32, 0),
        (40, data_offset),
        (48, 0),
        (56, data_offset),
        (64, data.len() as u64),
        (72, count),
        (80, count),
        (88, count),
    ] {
        header[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    header[96] = 1;
    header[97..102].copy_from_slice(&[
        COMPRESSION_NONE,
        COMPRESSION_GZIP,
        TILE_TYPE_MVT,
        min_zoom,
        max_zoom,
    ]);
    for (at, value) in [
        (102, -1_800_000_000_i32),
        (106, -850_000_000),
        (110, 1_800_000_000),
        (114, 850_000_000),
    ] {
        header[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    header[118] = max_zoom;

    [header, root, data].concat()
}
