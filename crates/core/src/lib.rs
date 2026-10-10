//! Platform-agnostic GPX rendering core.
//!
//! Parses GPX, projects coordinates into Web Mercator pixel space, and
//! rasterizes frames and label-free vector basemaps with `tiny-skia`.
//! Deliberately free of I/O (tile fetch, video encoding) so the same core
//! compiles for both the native CLI and the `wasm32` browser target.

pub mod anim;
pub mod geo;
pub mod gpx;
pub mod render;
pub mod tiles;
pub mod vector;

pub use anim::Timeline;
pub use geo::Projection;
pub use gpx::{parse, ParseError, TrackPoint};
pub use render::{
    draw_block, draw_overlay, render_frame, render_track, stroke_track, Anchor, FrameStyle,
    OverlayStyle, TrackStyle,
};
pub use tiles::{compose, TileId, TilePlacement};
pub use vector::{draw_vector_map, MapStyle, VectorTile};
