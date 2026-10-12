//! Label-free map styles for the OpenMapTiles vector schema.

mod json;
mod model;
mod presets;

pub(crate) use model::MAX_LINE_WIDTH;
pub use model::{Cap, ColorCurve, Join, MapStyle, Paint, Rule, StyleError, REFERENCE_ZOOM};
