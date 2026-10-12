//! Platform-agnostic GPX rendering core.
//!
//! Parses GPX, projects coordinates into Web Mercator pixel space, and
//! rasterizes frames, vector basemaps, map labels, buildings and terrain with
//! `tiny-skia`. Deliberately free of I/O (tile fetch, video encoding) so the
//! same core compiles for both the native CLI and the `wasm32` browser target.
//!
//! # Public paths
//!
//! Every public item has exactly one path:
//!
//! - Five function-heavy modules whose items read best qualified by their
//!   module are public as a whole, their types and constants included, and
//!   are not re-exported item by item: [`color`], [`math`], [`mercator`]
//!   (e.g. `mercator::Projection`), [`pmtiles`] and [`maplibre`].
//! - Everything else is re-exported at the crate root
//!   (`gpx_motion_core::Timeline`); the modules that define it are private.
//!
//! # Example
//!
//! ```
//! use gpx_motion_core::{parse, Timeline};
//!
//! let gpx = r#"<gpx><trk><trkseg>
//!     <trkpt lat="37.55" lon="126.98"/>
//!     <trkpt lat="37.56" lon="126.99"/>
//! </trkseg></trk></gpx>"#;
//! let points = parse(gpx)?;
//! let timeline = Timeline::build(&points).expect("two distinct points");
//! assert!(timeline.total_distance_m() > 1000.0);
//! # Ok::<(), gpx_motion_core::ParseError>(())
//! ```

#![warn(unreachable_pub)]

mod anim;
pub mod color;
mod geo;
mod gpx;
pub mod math;
mod render;
mod terrain;
#[cfg(test)]
mod test_support;
mod tiles;
mod vector;

pub use geo::mercator;
pub use tiles::pmtiles;
pub use vector::maplibre;

pub use anim::{rise_ahead, steer, Elevation, Timeline};
pub use geo::{
    ground_tile_tree, Camera, GroundDetail, GroundTile, Lens, ViewPoint, MAX_FOV_DEG,
    MAX_PITCH_DEG, MIN_FOV_DEG,
};
pub use gpx::{parse, ParseError, TrackPoint};
pub use render::{
    block_rect, draw_block, draw_buildings, draw_fog, draw_overlay, draw_profile, draw_track_3d,
    fog_band, marker_vertex, needs_shaping, stroke_track, stroke_track_by_elevation,
    track_outline_3d, track_vertices, Anchor, FrameStyle, OverlayStyle, ProfileStyle, Relief,
    SceneView, TextFont, TrackVertex,
};
pub use terrain::{
    draw_terrain, Dem, DepthMap, Surroundings, Terrain, TerrainArea, TERRAIN_CLEARANCE,
};
pub use tiles::{compose, TileId, TilePlacement};
pub use vector::{
    draw_labels, draw_vector_map, draw_vector_map_3d, extrude, extrude_within, halo_radius,
    place_labels_flat, Buildings, DecodeError, Facet, Keepout, LabelMemory, LabelSources,
    LabelStyle, MapStyle, PlacedLabel, StyleError, VectorTile, LABEL_LAYERS,
};
