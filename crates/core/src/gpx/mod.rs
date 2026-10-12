//! GPX input: parsing `<trkpt>` elements into [`TrackPoint`]s.

pub(crate) mod model;
pub(crate) mod parser;
pub(crate) mod time;

pub use model::TrackPoint;
pub use parser::{parse, ParseError};
