//! A minimal Mapbox Vector Tile encoder for the benchmark tiles: string
//! tags and point, line and polygon features.

/// Tile coordinate extent of every layer written here.
pub const EXTENT: i32 = 4096;

const WIRE_VARINT: u64 = 0;
const WIRE_LEN: u64 = 2;

fn varint(out: &mut Vec<u8>, mut value: u64) {
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

fn packed(out: &mut Vec<u8>, field: u64, values: &[u32]) {
    let mut body = Vec::new();
    for &v in values {
        varint(&mut body, u64::from(v));
    }
    bytes_field(out, field, &body);
}

fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

fn command(id: u32, count: usize) -> u32 {
    id | ((count as u32) << 3)
}

/// Geometry commands for a path through `points`, closed into a ring when
/// `close` is set.
fn path(points: &[(i32, i32)], close: bool) -> Vec<u32> {
    let mut out = vec![command(1, 1)];
    let mut cursor = (0, 0);
    for (i, &(x, y)) in points.iter().enumerate() {
        if i == 1 {
            out.push(command(2, points.len() - 1));
        }
        out.extend([zigzag(x - cursor.0), zigzag(y - cursor.1)]);
        cursor = (x, y);
    }
    if close {
        out.push(command(7, 1));
    }
    out
}

/// A feature's geometry, in tile coordinates.
pub enum Geometry {
    /// A single point.
    Point(i32, i32),
    /// An open line through the points.
    Line(Vec<(i32, i32)>),
    /// A polygon's outer ring, clockwise on screen.
    Polygon(Vec<(i32, i32)>),
}

/// One layer being written: its features and the key and value tables
/// their tags index into.
pub struct Layer {
    name: String,
    keys: Vec<String>,
    values: Vec<String>,
    features: Vec<Vec<u8>>,
}

impl Layer {
    /// An empty layer called `name`.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            keys: Vec::new(),
            values: Vec::new(),
            features: Vec::new(),
        }
    }

    fn index(table: &mut Vec<String>, item: &str) -> u32 {
        let i = table.iter().position(|t| t == item).unwrap_or_else(|| {
            table.push(item.to_string());
            table.len() - 1
        });
        i as u32
    }

    /// Add a feature with string `tags`.
    pub fn add(&mut self, geometry: Geometry, tags: &[(&str, &str)]) {
        let mut tag_ids = Vec::new();
        for (k, v) in tags {
            tag_ids.push(Self::index(&mut self.keys, k));
            tag_ids.push(Self::index(&mut self.values, v));
        }
        let (kind, commands) = match geometry {
            Geometry::Point(x, y) => (1, vec![command(1, 1), zigzag(x), zigzag(y)]),
            Geometry::Line(points) => (2, path(&points, false)),
            Geometry::Polygon(points) => (3, path(&points, true)),
        };
        let mut feature = Vec::new();
        if !tag_ids.is_empty() {
            packed(&mut feature, 2, &tag_ids);
        }
        varint_field(&mut feature, 3, kind);
        packed(&mut feature, 4, &commands);
        self.features.push(feature);
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        varint_field(&mut out, 15, 2);
        bytes_field(&mut out, 1, self.name.as_bytes());
        for feature in &self.features {
            bytes_field(&mut out, 2, feature);
        }
        for k in &self.keys {
            bytes_field(&mut out, 3, k.as_bytes());
        }
        for v in &self.values {
            let mut value = Vec::new();
            bytes_field(&mut value, 1, v.as_bytes());
            bytes_field(&mut out, 4, &value);
        }
        varint_field(&mut out, 5, EXTENT as u64);
        out
    }
}

/// A tile made of `layers`.
pub fn tile(layers: &[Layer]) -> Vec<u8> {
    let mut out = Vec::new();
    for layer in layers {
        bytes_field(&mut out, 3, &layer.encode());
    }
    out
}
