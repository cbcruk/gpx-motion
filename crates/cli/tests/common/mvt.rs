//! A minimal Mapbox Vector Tile encoder for test fixtures.

/// Tile coordinate extent of every layer written here.
pub const EXTENT: u32 = 4096;

const WIRE_VARINT: u64 = 0;
const WIRE_LEN: u64 = 2;

/// Appends `value` as a protobuf varint.
pub fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn key(out: &mut Vec<u8>, field: u64, wire: u64) {
    varint(out, (field << 3) | wire);
}

fn bytes_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    key(out, field, WIRE_LEN);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    key(out, field, WIRE_VARINT);
    varint(out, value);
}

fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

/// Geometry commands for one closed ring through `points`, in tile
/// coordinates.
fn ring(points: &[(i32, i32)]) -> Vec<u32> {
    let command = |id: u32, count: usize| id | ((count as u32) << 3);
    let mut out = vec![command(1, 1)];
    let mut cursor = (0, 0);
    for (i, &(x, y)) in points.iter().enumerate() {
        if i == 1 {
            out.push(command(2, points.len() - 1));
        }
        out.extend([zigzag(x - cursor.0), zigzag(y - cursor.1)]);
        cursor = (x, y);
    }
    out.push(command(7, 1));
    out
}

/// One layer `name` holding a single untagged polygon feature: the ring
/// through `points` (clockwise on screen, as the spec wants outer rings).
pub fn polygon_layer(name: &str, points: &[(i32, i32)]) -> Vec<u8> {
    let mut geometry = Vec::new();
    for command in ring(points) {
        varint(&mut geometry, u64::from(command));
    }
    let mut feature = Vec::new();
    varint_field(&mut feature, 3, 3);
    bytes_field(&mut feature, 4, &geometry);

    let mut layer = Vec::new();
    varint_field(&mut layer, 15, 2);
    bytes_field(&mut layer, 1, name.as_bytes());
    bytes_field(&mut layer, 2, &feature);
    varint_field(&mut layer, 5, u64::from(EXTENT));
    layer
}

/// A tile made of `layers` (each from [`polygon_layer`]).
pub fn tile(layers: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for layer in layers {
        bytes_field(&mut out, 3, layer);
    }
    out
}
