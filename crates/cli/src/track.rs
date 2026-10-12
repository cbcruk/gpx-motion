//! Input tracks: parsed GPX files with their timelines and palette colors.

use std::path::PathBuf;

use anyhow::{Context, Result};
use gpx_motion_core::{parse, Timeline, TrackPoint};
use tiny_skia::Color;

/// Distinct tail/marker colors cycled across multiple input tracks.
const PALETTE: [(u8, u8, u8); 6] = [
    (0xff, 0x5a, 0x36),
    (0x36, 0x9a, 0xff),
    (0x3a, 0xd1, 0x7a),
    (0xff, 0xd1, 0x36),
    (0xc8, 0x5a, 0xff),
    (0xff, 0x36, 0x9a),
];

/// A parsed track with its interpolation timeline and palette color.
pub struct Track {
    pub points: Vec<TrackPoint>,
    pub timeline: Timeline,
    pub color: Color,
}

/// Read and parse every input, coloring the tracks from [`PALETTE`] in
/// order; the first is the lead track the camera and overlays follow.
pub fn load_tracks(inputs: &[PathBuf], elevation_smoothing: f64) -> Result<Vec<Track>> {
    let mut tracks = Vec::new();
    for (i, path) in inputs.iter().enumerate() {
        let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        let points = parse(&text).with_context(|| path.display().to_string())?;
        let mut timeline = Timeline::build(&points)
            .with_context(|| format!("{}: needs at least two distinct points", path.display()))?;
        timeline.set_elevation_smoothing(elevation_smoothing);
        let (r, g, b) = PALETTE[i % PALETTE.len()];
        tracks.push(Track {
            points,
            timeline,
            color: Color::from_rgba8(r, g, b, 0xff),
        });
    }
    Ok(tracks)
}

/// Every point of every track, in input order.
pub fn all_points(tracks: &[Track]) -> Vec<TrackPoint> {
    tracks.iter().flat_map(|t| t.points.clone()).collect()
}
