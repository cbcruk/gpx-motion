//! Platform-agnostic GPX rendering core.
//!
//! Parses GPX, projects coordinates into Web Mercator pixel space, and
//! rasterizes frames and label-free vector basemaps with `tiny-skia`.
//! Deliberately free of I/O (tile fetch, video encoding) so the same core
//! compiles for both the native CLI and the `wasm32` browser target.

pub mod anim;
pub mod color;
pub mod geo;
pub mod gpx;
pub mod math;
pub mod render;
pub mod terrain;
pub mod tiles;
pub mod vector;

pub use anim::{rise_ahead, steer, Elevation, Timeline};
pub use geo::{ground_tiles, Camera, GroundDetail, GroundTile, Lens, Projection, MAX_PITCH_DEG};
pub use gpx::{parse, ParseError, TrackPoint};
pub use render::{
    block_rect, draw_block, draw_buildings, draw_fog, draw_overlay, draw_profile, draw_track_3d,
    elevation_color, fog_band, marker_vertex, needs_shaping, render_frame, render_track,
    stroke_track, stroke_track_by_elevation, track_outline_3d, track_vertices, Anchor, FrameStyle,
    OverlayStyle, ProfileStyle, Relief, SceneView, TextFont, TrackStyle, TrackVertex,
};
pub use terrain::{
    draw_terrain, Dem, DepthMap, Surroundings, Terrain, TerrainArea, TERRAIN_CLEARANCE,
};
pub use tiles::{compose, TileId, TilePlacement};
pub use vector::{
    draw_labels, draw_vector_map, draw_vector_map_3d, extrude, extrude_within, halo_radius,
    place_labels, place_labels_3d, place_labels_flat, Buildings, Facet, Keepout, LabelMemory,
    LabelSources, LabelStyle, MapStyle, PlacedLabel, VectorTile, LABEL_LAYERS,
};
