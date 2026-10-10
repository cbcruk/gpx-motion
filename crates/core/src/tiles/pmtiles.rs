//! Read-side pieces of the PMTiles v3 single-file tile archive: header,
//! directories, tile ids, and decompression. Byte ranges are fetched by the
//! caller (a file in the CLI, HTTP ranges in a browser), so this stays I/O
//! free. Spec: <https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md>.

use std::fmt;
use std::io::Read;

/// Size of the fixed header at the start of every archive.
pub const HEADER_LEN: usize = 127;

/// Deepest chain of leaf directories the spec allows below the root.
pub const MAX_DIRECTORY_DEPTH: usize = 3;

/// How directories or tiles are compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Unknown,
    None,
    Gzip,
    Brotli,
    Zstd,
}

impl Compression {
    fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::None,
            2 => Self::Gzip,
            3 => Self::Brotli,
            4 => Self::Zstd,
            _ => Self::Unknown,
        }
    }
}

/// What the archive's tiles contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileType {
    Unknown,
    Mvt,
    Png,
    Jpeg,
    Webp,
    Avif,
}

impl TileType {
    fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::Mvt,
            2 => Self::Png,
            3 => Self::Jpeg,
            4 => Self::Webp,
            5 => Self::Avif,
            _ => Self::Unknown,
        }
    }
}

/// The fixed 127-byte archive header (the fields a reader needs).
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub root_offset: u64,
    pub root_length: u64,
    pub leaf_offset: u64,
    pub tile_data_offset: u64,
    pub internal_compression: Compression,
    pub tile_compression: Compression,
    pub tile_type: TileType,
    pub min_zoom: u8,
    pub max_zoom: u8,
}

impl Header {
    /// Parse the header from the archive's first [`HEADER_LEN`] bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, PmTilesError> {
        if bytes.len() < HEADER_LEN || &bytes[..7] != b"PMTiles" {
            return Err(PmTilesError("not a PMTiles archive"));
        }
        if bytes[7] != 3 {
            return Err(PmTilesError("unsupported PMTiles version (need 3)"));
        }
        let u64_at =
            |i: usize| u64::from_le_bytes(bytes[i..i + 8].try_into().expect("8-byte slice"));
        Ok(Self {
            root_offset: u64_at(8),
            root_length: u64_at(16),
            leaf_offset: u64_at(40),
            tile_data_offset: u64_at(56),
            internal_compression: Compression::from_byte(bytes[97]),
            tile_compression: Compression::from_byte(bytes[98]),
            tile_type: TileType::from_byte(bytes[99]),
            min_zoom: bytes[100],
            max_zoom: bytes[101],
        })
    }
}

/// One directory entry: a run of tiles sharing data, or (run length 0) a
/// pointer to a leaf directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u32,
    pub run_length: u32,
}

/// Where a tile id resolves within a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    /// Tile bytes at `offset` (relative to the tile data section).
    Tile { offset: u64, length: u32 },
    /// Continue in the leaf directory at `offset` (relative to the leaf
    /// directory section).
    Leaf { offset: u64, length: u32 },
    /// The archive has no such tile.
    Missing,
}

/// Parse a (decompressed) directory.
pub fn parse_directory(bytes: &[u8]) -> Result<Vec<Entry>, PmTilesError> {
    let mut reader = Varints { bytes, pos: 0 };
    let count = reader.next()? as usize;
    if count > bytes.len() {
        return Err(PmTilesError("directory entry count exceeds its size"));
    }
    let mut entries = vec![
        Entry {
            tile_id: 0,
            offset: 0,
            length: 0,
            run_length: 0,
        };
        count
    ];
    let mut last_id = 0u64;
    for e in entries.iter_mut() {
        last_id = last_id
            .checked_add(reader.next()?)
            .ok_or(PmTilesError("tile id overflow"))?;
        e.tile_id = last_id;
    }
    for e in entries.iter_mut() {
        e.run_length = u32::try_from(reader.next()?).map_err(|_| PmTilesError("run too long"))?;
    }
    for e in entries.iter_mut() {
        e.length = u32::try_from(reader.next()?).map_err(|_| PmTilesError("entry too long"))?;
    }
    for i in 0..count {
        let value = reader.next()?;
        entries[i].offset = if value == 0 && i > 0 {
            entries[i - 1].offset + entries[i - 1].length as u64
        } else {
            value
                .checked_sub(1)
                .ok_or(PmTilesError("first entry has no offset"))?
        };
    }
    Ok(entries)
}

/// Resolve `tile_id` against a parsed directory.
pub fn find(entries: &[Entry], tile_id: u64) -> Lookup {
    let i = entries.partition_point(|e| e.tile_id <= tile_id);
    let Some(entry) = i.checked_sub(1).map(|i| entries[i]) else {
        return Lookup::Missing;
    };
    if entry.run_length == 0 {
        Lookup::Leaf {
            offset: entry.offset,
            length: entry.length,
        }
    } else if tile_id - entry.tile_id < entry.run_length as u64 {
        Lookup::Tile {
            offset: entry.offset,
            length: entry.length,
        }
    } else {
        Lookup::Missing
    }
}

/// The archive's id for tile `z/x/y`: tiles of all lower zooms come first,
/// then this zoom's tiles in Hilbert-curve order.
pub fn tile_id(z: u8, x: u32, y: u32) -> u64 {
    let base = ((1u64 << (2 * z as u32)) - 1) / 3;
    let n = 1u64 << z;
    let (mut x, mut y) = (x as u64, y as u64);
    let mut d = 0u64;
    let mut s = n / 2;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - (x & (s - 1));
                y = s - 1 - (y & (s - 1));
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    base + d
}

/// Decompress directory or tile bytes. Only `None` and `Gzip` are supported;
/// gzip is also detected by its magic bytes when the archive says `Unknown`.
pub fn decompress(bytes: &[u8], compression: Compression) -> Result<Vec<u8>, PmTilesError> {
    let gzipped = bytes.starts_with(&[0x1f, 0x8b]);
    match compression {
        Compression::Gzip => gunzip(bytes),
        Compression::Unknown if gzipped => gunzip(bytes),
        Compression::None | Compression::Unknown => Ok(bytes.to_vec()),
        Compression::Brotli => Err(PmTilesError("brotli-compressed tiles are not supported")),
        Compression::Zstd => Err(PmTilesError("zstd-compressed tiles are not supported")),
    }
}

/// Inflate a gzip stream.
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, PmTilesError> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .map_err(|_| PmTilesError("corrupt gzip data"))?;
    Ok(out)
}

/// Why an archive or part of it couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmTilesError(&'static str);

impl fmt::Display for PmTilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for PmTilesError {}

struct Varints<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Varints<'_> {
    fn next(&mut self) -> Result<u64, PmTilesError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .bytes
                .get(self.pos)
                .ok_or(PmTilesError("truncated directory"))?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(PmTilesError("varint too long"))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8 & 0x7f) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    /// Serialize a directory the way writers do (offset 0 for contiguous).
    pub(crate) fn encode_directory(entries: &[Entry]) -> Vec<u8> {
        let mut out = Vec::new();
        varint(&mut out, entries.len() as u64);
        let mut last = 0;
        for e in entries {
            varint(&mut out, e.tile_id - last);
            last = e.tile_id;
        }
        for e in entries {
            varint(&mut out, e.run_length as u64);
        }
        for e in entries {
            varint(&mut out, e.length as u64);
        }
        for (i, e) in entries.iter().enumerate() {
            let contiguous =
                i > 0 && e.offset == entries[i - 1].offset + entries[i - 1].length as u64;
            varint(&mut out, if contiguous { 0 } else { e.offset + 1 });
        }
        out
    }

    pub(crate) fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn tile_ids_follow_the_spec() {
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!(tile_id(1, 0, 0), 1);
        assert_eq!(tile_id(1, 0, 1), 2);
        assert_eq!(tile_id(1, 1, 1), 3);
        assert_eq!(tile_id(1, 1, 0), 4);
        assert_eq!(tile_id(2, 0, 0), 5);
        assert_eq!(tile_id(20, 0, 0), 366_503_875_925);
        assert_eq!(tile_id(12, 3423, 1763), 19_078_479);
        let mut seen = std::collections::HashSet::new();
        for x in 0..8 {
            for y in 0..8 {
                assert!(seen.insert(tile_id(3, x, y)));
            }
        }
        assert_eq!(seen.iter().min(), Some(&21));
        assert_eq!(seen.iter().max(), Some(&84));
    }

    #[test]
    fn directories_round_trip_and_resolve() {
        let entries = vec![
            Entry {
                tile_id: 5,
                offset: 0,
                length: 100,
                run_length: 1,
            },
            Entry {
                tile_id: 6,
                offset: 100,
                length: 50,
                run_length: 3,
            },
            Entry {
                tile_id: 20,
                offset: 0,
                length: 77,
                run_length: 0,
            },
            Entry {
                tile_id: 40,
                offset: 900,
                length: 10,
                run_length: 1,
            },
        ];
        let bytes = encode_directory(&entries);
        let parsed = parse_directory(&gunzip(&gzip(&bytes)).unwrap()).unwrap();
        assert_eq!(parsed, entries);
        assert_eq!(find(&parsed, 4), Lookup::Missing);
        assert_eq!(
            find(&parsed, 5),
            Lookup::Tile {
                offset: 0,
                length: 100
            }
        );
        assert_eq!(
            find(&parsed, 8),
            Lookup::Tile {
                offset: 100,
                length: 50
            }
        );
        assert_eq!(find(&parsed, 9), Lookup::Missing);
        assert_eq!(
            find(&parsed, 25),
            Lookup::Leaf {
                offset: 0,
                length: 77
            }
        );
        assert_eq!(find(&parsed, 41), Lookup::Missing);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Header::parse(b"nope").is_err());
        assert!(parse_directory(&[0x05, 0x01]).is_err());
        assert!(decompress(&[1, 2, 3], Compression::Zstd).is_err());
        assert_eq!(decompress(&[1, 2], Compression::None).unwrap(), vec![1, 2]);
        let gz = gzip(b"mvt");
        assert_eq!(decompress(&gz, Compression::Unknown).unwrap(), b"mvt");
    }
}
