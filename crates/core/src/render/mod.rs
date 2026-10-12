//! Rasterizing the track, the HUD, and the 3D scene with `tiny-skia`.

pub(crate) mod extrusion;
pub(crate) mod frame;
mod occlusion;
pub(crate) mod overlay;
pub(crate) mod profile;
pub(crate) mod ramp;
pub(crate) mod sample;
pub(crate) mod scene;
pub(crate) mod scene_data;
pub(crate) mod text;
pub(crate) mod track;

pub use extrusion::draw_buildings;
pub use frame::{draw_overlay, FrameStyle};
pub use overlay::{block_rect, draw_block, Anchor, OverlayStyle};
pub use profile::{draw_profile, ProfileStyle};
pub use scene::{draw_fog, draw_track_3d, fog_band, track_outline_3d, Relief, SceneView};
pub use scene_data::{marker_vertex, track_vertices, TrackVertex};
pub use text::{needs_shaping, TextFont};
pub use track::{stroke_track, stroke_track_by_elevation};
