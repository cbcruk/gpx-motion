//! Map labels: place, peak, and water names at points, and road and river
//! names along their lines; chosen by importance, kept clear of each other,
//! the track, and screen overlays, and drawn with a halo.

mod draw;
mod keepout;
mod line_labels;
mod line_stitch;
mod memory;
mod placer;
mod sources;
mod style;
#[cfg(test)]
mod test_support;

pub use draw::{draw_labels, halo_radius};
pub use keepout::Keepout;
pub use memory::LabelMemory;
pub use placer::{place_labels, place_labels_3d, place_labels_flat, PlacedLabel};
pub use sources::{LabelSources, LABEL_LAYERS};
pub use style::LabelStyle;
