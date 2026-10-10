//! Ground relief from a digital elevation model, drawn as a textured mesh in
//! the 3D view.

pub mod dem;
pub mod mesh;

pub use dem::Dem;
pub use mesh::{draw_terrain, Terrain};
