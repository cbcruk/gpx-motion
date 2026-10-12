//! Ground relief from a digital elevation model, drawn as a textured mesh in
//! the 3D view.

pub mod area;
pub mod dem;
mod grid;
pub mod mesh;
mod raster;

pub use area::TerrainArea;
pub use dem::Dem;
pub use mesh::{draw_terrain, DepthMap, Surroundings, Terrain, TERRAIN_CLEARANCE};
