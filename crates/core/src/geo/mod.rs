pub mod camera;
pub mod ground_lod;
pub mod mercator;

pub use camera::{Camera, Lens};
pub use ground_lod::{ground_tiles, GroundDetail, GroundTile};
pub use mercator::Projection;

/// Fraction of the way from view `a` to view `b` the center moves once the
/// map scale has shrunk by `ratio` (of `total` at `b`), so that one map point
/// stays put on screen: a straight zoom about a pivot, not a pan plus zoom.
/// Falls back to `s` when the scale doesn't change.
fn pivot_weight(ratio: f64, total: f64, s: f64) -> f64 {
    if (total - 1.0).abs() < 1e-9 {
        s
    } else {
        (ratio - 1.0) / (total - 1.0)
    }
}
