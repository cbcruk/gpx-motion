//! Platform-agnostic GPX rendering core.
//!
//! Parses GPX, projects coordinates into Web Mercator pixel space, and
//! rasterizes frames with `tiny-skia`. Deliberately free of I/O (tile fetch,
//! video encoding) so the same core compiles for both the native CLI and a
//! future `wasm32` browser target.

pub mod anim;
pub mod geo;
pub mod gpx;
pub mod render;
pub mod tiles;

pub use anim::Timeline;
pub use geo::Projection;
pub use gpx::{parse, ParseError, TrackPoint};
pub use render::{
    draw_block, draw_overlay, render_frame, render_track, stroke_track, Anchor, FrameStyle,
    OverlayStyle, TrackStyle,
};
pub use tiles::{compose, TileId, TilePlacement};
