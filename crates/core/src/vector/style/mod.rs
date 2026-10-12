//! Map styles for the OpenMapTiles vector schema: the fill and line rules
//! (and building extrusions) a basemap is drawn with, from the built-in
//! presets, gpx-motion style JSON, or an imported MapLibre style. Labels are
//! styled separately.

mod json;
mod model;
mod presets;

pub(crate) use model::MAX_LINE_WIDTH;
pub(crate) use model::{Cap, ColorCurve, Join, Paint, Rule, REFERENCE_ZOOM};
pub use model::{MapStyle, StyleError};
