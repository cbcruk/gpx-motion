//! The track point read from GPX.

/// One GPX `<trkpt>`: a position with its optional elevation and time.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackPoint {
    /// Latitude in degrees (WGS 84).
    pub lat: f64,
    /// Longitude in degrees (WGS 84).
    pub lon: f64,
    /// Elevation in meters from `<ele>`, if present.
    pub ele: Option<f64>,
    /// The `<time>` text as written (ISO 8601), if present.
    pub time: Option<String>,
}

impl TrackPoint {
    /// A point at `lat`, `lon` without elevation or time.
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            lat,
            lon,
            ele: None,
            time: None,
        }
    }
}
