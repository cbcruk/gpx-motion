//! Minimal Mapbox Vector Tile (MVT 2.1) decoder.
//!
//! Hand-rolled protobuf reading keeps the core dependency-free and `wasm32`
//! friendly. Geometry stays as raw command integers until a feature is drawn,
//! so layers the style never touches (labels, POIs) cost only a byte copy.

use std::fmt;

/// A decoded vector tile: its named layers in source order.
#[derive(Debug, Clone, Default)]
pub struct VectorTile {
    pub(crate) layers: Vec<Layer>,
}

impl VectorTile {
    /// Decode an uncompressed MVT protobuf.
    ///
    /// # Errors
    ///
    /// A [`DecodeError`] when `bytes` isn't a well-formed MVT protobuf.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes);
        let mut layers = Vec::new();
        while let Some((field, wire)) = reader.key()? {
            match (field, wire) {
                (3, WIRE_LEN) => layers.push(Layer::decode(reader.bytes()?)?),
                _ => reader.skip(wire)?,
            }
        }
        Ok(Self { layers })
    }

    /// Decode only the layers named in `names`, skipping the rest unparsed
    /// (e.g. just the label layers, far cheaper than the whole tile).
    ///
    /// # Errors
    ///
    /// A [`DecodeError`] when `bytes` isn't a well-formed MVT protobuf.
    pub fn decode_layers(bytes: &[u8], names: &[&str]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes);
        let mut layers = Vec::new();
        while let Some((field, wire)) = reader.key()? {
            match (field, wire) {
                (3, WIRE_LEN) => {
                    let layer = reader.bytes()?;
                    if names.contains(&Layer::name_of(layer)?.as_str()) {
                        layers.push(Layer::decode(layer)?);
                    }
                }
                _ => reader.skip(wire)?,
            }
        }
        Ok(Self { layers })
    }

    /// The layer with the given name, if present.
    pub(crate) fn layer(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name == name)
    }
}

/// One named layer: features plus the key/value tables their tags index into.
#[derive(Debug, Clone)]
pub(crate) struct Layer {
    pub name: String,
    /// Coordinate range of a tile edge in this layer (usually 4096).
    pub extent: u32,
    pub(crate) features: Vec<Feature>,
    keys: Vec<String>,
    values: Vec<Value>,
}

impl Layer {
    /// The layer's name, read without decoding its features.
    fn name_of(bytes: &[u8]) -> Result<String, DecodeError> {
        let mut reader = Reader::new(bytes);
        while let Some((field, wire)) = reader.key()? {
            match (field, wire) {
                (1, WIRE_LEN) => return reader.string(),
                _ => reader.skip(wire)?,
            }
        }
        Ok(String::new())
    }

    fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes);
        let mut layer = Layer {
            name: String::new(),
            extent: 4096,
            features: Vec::new(),
            keys: Vec::new(),
            values: Vec::new(),
        };
        while let Some((field, wire)) = reader.key()? {
            match (field, wire) {
                (1, WIRE_LEN) => layer.name = reader.string()?,
                (2, WIRE_LEN) => layer.features.push(Feature::decode(reader.bytes()?)?),
                (3, WIRE_LEN) => layer.keys.push(reader.string()?),
                (4, WIRE_LEN) => layer.values.push(Value::decode(reader.bytes()?)?),
                (5, WIRE_VARINT) => layer.extent = reader.varint()? as u32,
                _ => reader.skip(wire)?,
            }
        }
        Ok(layer)
    }

    /// Look up a feature property by key.
    pub(crate) fn property(&self, feature: &Feature, key: &str) -> Option<&Value> {
        let (pairs, _) = feature.tags.as_chunks::<2>();
        pairs.iter().find_map(|&[k, v]| {
            if self.keys.get(k as usize)? == key {
                self.values.get(v as usize)
            } else {
                None
            }
        })
    }
}

/// Geometry type of a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeomType {
    Unknown,
    Point,
    LineString,
    Polygon,
}

/// A test point feature: tile coordinates and its properties.
#[cfg(test)]
pub(crate) type TestPoint = ((i32, i32), Vec<(&'static str, Value)>);

#[cfg(test)]
impl Layer {
    /// A layer of point features at tile coordinates, each with properties.
    pub(crate) fn test_points(name: &str, points: &[TestPoint]) -> Self {
        let mut layer = Layer {
            name: name.to_string(),
            extent: 4096,
            features: Vec::new(),
            keys: Vec::new(),
            values: Vec::new(),
        };
        let zigzag = |v: i32| ((v << 1) ^ (v >> 31)) as u32;
        for ((x, y), props) in points {
            let mut tags = Vec::new();
            for (key, value) in props {
                tags.push(layer.keys.len() as u32);
                layer.keys.push(key.to_string());
                tags.push(layer.values.len() as u32);
                layer.values.push(value.clone());
            }
            layer.features.push(Feature {
                geom_type: GeomType::Point,
                tags,
                geometry: vec![9, zigzag(*x), zigzag(*y)],
            });
        }
        layer
    }

    /// A layer with one square polygon feature spanning `lo..hi` on both
    /// axes in tile coordinates.
    pub(crate) fn test_square(name: &str, lo: i32, hi: i32) -> Self {
        Self::test_square_with(name, lo, hi, Vec::new())
    }

    /// [`Layer::test_square`] with properties.
    pub(crate) fn test_square_with(
        name: &str,
        lo: i32,
        hi: i32,
        props: Vec<(&'static str, Value)>,
    ) -> Self {
        let mut layer = Self::test_points(name, &[((lo, lo), props)]);
        let zigzag = |v: i32| ((v << 1) ^ (v >> 31)) as u32;
        let side = hi - lo;
        let feature = &mut layer.features[0];
        feature.geom_type = GeomType::Polygon;
        feature.geometry = vec![
            9,
            zigzag(lo),
            zigzag(lo),
            26,
            zigzag(side),
            zigzag(0),
            zigzag(0),
            zigzag(side),
            zigzag(-side),
            zigzag(0),
            15,
        ];
        layer
    }

    /// A layer with one horizontal line feature from `(x0, y)` to `(x1, y)`
    /// in tile coordinates, with properties.
    pub(crate) fn test_line(
        name: &str,
        x0: i32,
        x1: i32,
        y: i32,
        props: &[(&'static str, Value)],
    ) -> Self {
        let mut layer = Self::test_points(name, &[((x0, y), props.to_vec())]);
        let zigzag = |v: i32| ((v << 1) ^ (v >> 31)) as u32;
        let feature = &mut layer.features[0];
        feature.geom_type = GeomType::LineString;
        feature.geometry = vec![9, zigzag(x0), zigzag(y), 10, zigzag(x1 - x0), zigzag(0)];
        layer
    }
}

/// A single feature with undecoded geometry commands.
#[derive(Debug, Clone)]
pub(crate) struct Feature {
    pub geom_type: GeomType,
    tags: Vec<u32>,
    geometry: Vec<u32>,
}

impl Feature {
    fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes);
        let mut feature = Feature {
            geom_type: GeomType::Unknown,
            tags: Vec::new(),
            geometry: Vec::new(),
        };
        while let Some((field, wire)) = reader.key()? {
            match (field, wire) {
                (2, WIRE_LEN) => feature.tags = reader.packed()?,
                (3, WIRE_VARINT) => {
                    feature.geom_type = match reader.varint()? {
                        1 => GeomType::Point,
                        2 => GeomType::LineString,
                        3 => GeomType::Polygon,
                        _ => GeomType::Unknown,
                    }
                }
                (4, WIRE_LEN) => feature.geometry = reader.packed()?,
                _ => reader.skip(wire)?,
            }
        }
        Ok(feature)
    }

    /// Decode the geometry into parts in tile coordinates (`0..extent`).
    ///
    /// Each `MoveTo` starts a new part: a line for line strings, a ring for
    /// polygons (closed implicitly; the closing vertex is not repeated).
    /// Truncated command streams yield the parts decoded so far.
    pub(crate) fn parts(&self) -> Vec<Vec<(i32, i32)>> {
        let mut parts: Vec<Vec<(i32, i32)>> = Vec::new();
        let (mut x, mut y) = (0i32, 0i32);
        let mut i = 0;
        while i < self.geometry.len() {
            let command = self.geometry[i];
            i += 1;
            let (id, count) = (command & 0x7, (command >> 3) as usize);
            match id {
                1 | 2 => {
                    for _ in 0..count {
                        let (Some(&dx), Some(&dy)) =
                            (self.geometry.get(i), self.geometry.get(i + 1))
                        else {
                            return parts;
                        };
                        i += 2;
                        x = x.wrapping_add(zigzag(dx));
                        y = y.wrapping_add(zigzag(dy));
                        if id == 1 || parts.is_empty() {
                            parts.push(Vec::new());
                        }
                        if let Some(part) = parts.last_mut() {
                            part.push((x, y));
                        }
                    }
                }
                7 => {}
                _ => return parts,
            }
        }
        parts
    }
}

/// A feature property value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    String(String),
    Float(f64),
    Int(i64),
    Bool(bool),
}

impl Value {
    fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes);
        let mut value = Value::Bool(false);
        while let Some((field, wire)) = reader.key()? {
            value = match (field, wire) {
                (1, WIRE_LEN) => Value::String(reader.string()?),
                (2, WIRE_32) => Value::Float(f64::from(f32::from_bits(reader.fixed32()?))),
                (3, WIRE_64) => Value::Float(f64::from_bits(reader.fixed64()?)),
                (4 | 5, WIRE_VARINT) => Value::Int(reader.varint()? as i64),
                (6, WIRE_VARINT) => Value::Int(zigzag64(reader.varint()?)),
                (7, WIRE_VARINT) => Value::Bool(reader.varint()? != 0),
                _ => {
                    reader.skip(wire)?;
                    continue;
                }
            };
        }
        Ok(value)
    }

    /// The string payload, if this is a string value.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a number, when it is one.
    pub(crate) fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
}

/// A malformed or truncated protobuf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(&'static str);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid vector tile: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

const WIRE_VARINT: u8 = 0;
const WIRE_64: u8 = 1;
const WIRE_LEN: u8 = 2;
const WIRE_32: u8 = 5;

fn zigzag(n: u32) -> i32 {
    ((n >> 1) as i32) ^ -((n & 1) as i32)
}

fn zigzag64(n: u64) -> i64 {
    ((n >> 1) as i64) ^ -((n & 1) as i64)
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn key(&mut self) -> Result<Option<(u32, u8)>, DecodeError> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        Ok(Some(((key >> 3) as u32, (key & 0x7) as u8)))
    }

    fn varint(&mut self) -> Result<u64, DecodeError> {
        let mut result = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .buf
                .get(self.pos)
                .ok_or(DecodeError("truncated varint"))?;
            self.pos += 1;
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
        Err(DecodeError("varint too long"))
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.buf.len())
            .ok_or(DecodeError("truncated field"))?;
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.varint()? as usize;
        self.take(len)
    }

    fn string(&mut self) -> Result<String, DecodeError> {
        let bytes = self.bytes()?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn packed(&mut self) -> Result<Vec<u32>, DecodeError> {
        let mut inner = Reader::new(self.bytes()?);
        let mut out = Vec::new();
        while inner.pos < inner.buf.len() {
            out.push(inner.varint()? as u32);
        }
        Ok(out)
    }

    fn fixed32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn fixed64(&mut self) -> Result<u64, DecodeError> {
        let b = self.take(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(u64::from_le_bytes(arr))
    }

    fn skip(&mut self, wire: u8) -> Result<(), DecodeError> {
        match wire {
            WIRE_VARINT => self.varint().map(|_| ()),
            WIRE_64 => self.take(8).map(|_| ()),
            WIRE_LEN => self.bytes().map(|_| ()),
            WIRE_32 => self.take(4).map(|_| ()),
            _ => Err(DecodeError("unsupported wire type")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::proptest_config;
    use proptest::collection::vec;
    use proptest::option;
    use proptest::prelude::*;
    use proptest::sample::Index;

    fn varint(mut n: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn len_field(field: u32, payload: &[u8], out: &mut Vec<u8>) {
        varint(u64::from((field << 3) | u32::from(WIRE_LEN)), out);
        varint(payload.len() as u64, out);
        out.extend_from_slice(payload);
    }

    fn packed(values: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        for &v in values {
            varint(u64::from(v), &mut out);
        }
        out
    }

    fn zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }

    /// A tile with one `water` polygon (square ring) tagged `class=lake`.
    fn sample_tile() -> Vec<u8> {
        let geometry = [
            (1 << 3) | 1,
            zz(10),
            zz(10),
            (3 << 3) | 2,
            zz(100),
            zz(0),
            zz(0),
            zz(100),
            zz(-100),
            zz(0),
            (1 << 3) | 7,
        ];
        let mut feature = Vec::new();
        len_field(2, &packed(&[0, 0]), &mut feature);
        varint((3 << 3) as u64, &mut feature);
        varint(3, &mut feature);
        len_field(4, &packed(&geometry), &mut feature);

        let mut value = Vec::new();
        len_field(1, b"lake", &mut value);

        let mut layer = Vec::new();
        varint((15 << 3) as u64, &mut layer);
        varint(2, &mut layer);
        len_field(1, b"water", &mut layer);
        len_field(2, &feature, &mut layer);
        len_field(3, b"class", &mut layer);
        len_field(4, &value, &mut layer);
        varint((5 << 3) as u64, &mut layer);
        varint(4096, &mut layer);

        let mut tile = Vec::new();
        len_field(3, &layer, &mut tile);
        tile
    }

    #[test]
    fn decodes_layer_feature_and_properties() {
        let tile = VectorTile::decode(&sample_tile()).unwrap();
        let water = tile.layer("water").unwrap();
        assert_eq!(water.extent, 4096);
        assert_eq!(water.features.len(), 1);

        let feature = &water.features[0];
        assert_eq!(feature.geom_type, GeomType::Polygon);
        assert_eq!(
            water.property(feature, "class").and_then(Value::as_str),
            Some("lake")
        );
        assert_eq!(water.property(feature, "missing"), None);
    }

    #[test]
    fn decodes_delta_encoded_ring() {
        let tile = VectorTile::decode(&sample_tile()).unwrap();
        let parts = tile.layer("water").unwrap().features[0].parts();
        assert_eq!(
            parts,
            vec![vec![(10, 10), (110, 10), (110, 110), (10, 110)]]
        );
    }

    #[test]
    fn multi_line_string_splits_on_move_to() {
        let feature = Feature {
            geom_type: GeomType::LineString,
            tags: Vec::new(),
            geometry: vec![
                (1 << 3) | 1,
                zz(0),
                zz(0),
                (1 << 3) | 2,
                zz(5),
                zz(0),
                (1 << 3) | 1,
                zz(0),
                zz(5),
                (1 << 3) | 2,
                zz(-5),
                zz(0),
            ],
        };
        assert_eq!(
            feature.parts(),
            vec![vec![(0, 0), (5, 0)], vec![(5, 5), (0, 5)]]
        );
    }

    #[test]
    fn decode_layers_keeps_only_the_named_layers() {
        let tile = VectorTile::decode_layers(&sample_tile(), &["water"]).unwrap();
        assert_eq!(tile.layers.len(), 1);
        assert_eq!(tile.layers[0].features.len(), 1);
        let none = VectorTile::decode_layers(&sample_tile(), &["place"]).unwrap();
        assert!(none.layers.is_empty());
    }

    #[test]
    fn truncated_input_is_an_error() {
        let bytes = sample_tile();
        assert!(VectorTile::decode(&bytes[..bytes.len() - 3]).is_err());
    }

    /// Decode `bytes` both ways and read every decoded feature, as the
    /// renderers would.
    fn decode_and_read(bytes: &[u8]) {
        for tile in [
            VectorTile::decode(bytes),
            VectorTile::decode_layers(bytes, &["water"]),
        ]
        .into_iter()
        .flatten()
        {
            for layer in &tile.layers {
                for feature in &layer.features {
                    let _ = feature.parts();
                    let _ = layer.property(feature, "class");
                }
            }
        }
    }

    proptest! {
        #![proptest_config(proptest_config())]

        #[test]
        fn arbitrary_bytes_never_panic(bytes in vec(any::<u8>(), 0..512)) {
            decode_and_read(&bytes);
        }

        #[test]
        fn a_corrupted_tile_never_panics(
            edits in vec((any::<Index>(), any::<u8>()), 0..4),
            cut in option::weighted(0.25, any::<Index>()),
        ) {
            let mut bytes = sample_tile();
            for (at, byte) in edits {
                let at = at.index(bytes.len());
                bytes[at] = byte;
            }
            if let Some(cut) = cut {
                bytes.truncate(cut.index(bytes.len() + 1));
            }
            decode_and_read(&bytes);
        }

        #[test]
        fn arbitrary_geometry_commands_never_panic(geometry in vec(any::<u32>(), 0..64)) {
            let feature = Feature {
                geom_type: GeomType::LineString,
                tags: Vec::new(),
                geometry,
            };
            let points: usize = feature.parts().iter().map(Vec::len).sum();
            prop_assert!(points <= feature.geometry.len() / 2);
        }
    }
}
