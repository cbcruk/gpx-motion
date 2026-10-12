//! Vector basemaps: MVT decoding, map styles (native and MapLibre),
//! rasterization flat or in 3D, extruded buildings, and map labels.

pub(crate) mod expr;
pub(crate) mod extrusion;
mod key_hash;
pub(crate) mod labels;
pub mod maplibre;
pub(crate) mod mvt;
pub(crate) mod offset;
pub(crate) mod render;
pub(crate) mod style;

pub use extrusion::{extrude, extrude_within, Buildings, Facet};
pub use labels::{
    draw_labels, halo_radius, place_labels_flat, Keepout, LabelMemory, LabelSources, LabelStyle,
    PlacedLabel, LABEL_LAYERS,
};
pub use mvt::{DecodeError, VectorTile};
pub use render::{draw_vector_map, draw_vector_map_3d};
pub use style::{MapStyle, StyleError};
