pub mod composer;
pub mod pmtiles;

pub use composer::compose;

/// A slippy-map tile address (TMS/XYZ scheme).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileId {
    pub zoom: u32,
    pub x: u32,
    pub y: u32,
}

/// A visible tile together with where its top-left corner lands in viewport
/// pixel space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TilePlacement {
    pub id: TileId,
    pub dx: f64,
    pub dy: f64,
    /// Edge length in viewport pixels: 256, or larger when overzoomed.
    pub size: f64,
}

impl TilePlacement {
    /// A tile drawn at its own zoom with its top-left corner at the origin.
    pub fn whole(id: TileId) -> Self {
        Self {
            id,
            dx: 0.0,
            dy: 0.0,
            size: 256.0,
        }
    }
}
