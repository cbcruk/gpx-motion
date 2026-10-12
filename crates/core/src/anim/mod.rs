//! Playback over time: the track timeline, its elevation profile, and the
//! smoothing that keeps camera headings and heights steady.

pub(crate) mod elevation;
pub(crate) mod heading;
pub(crate) mod rise;
pub(crate) mod timeline;

pub use elevation::Elevation;
pub use heading::steer;
pub use rise::rise_ahead;
pub(crate) use timeline::Sample;
pub use timeline::Timeline;
