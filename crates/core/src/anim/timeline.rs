use super::elevation::Elevation;
use crate::gpx::model::TrackPoint;
use crate::gpx::time::parse_epoch_ms;

/// A coordinate sampled at a normalized progress `u` along the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub lat: f64,
    pub lon: f64,
    /// Elevation in meters, when the track has any.
    pub ele: Option<f64>,
    /// Progress in `0.0..=1.0`.
    pub u: f64,
}

/// Maps animation progress (`u` in `0..=1`) to a position along the track.
///
/// The cumulative metric is real elapsed time when every point carries a
/// parseable timestamp (so pauses render as the marker holding still);
/// otherwise it falls back to geographic distance (constant-speed playback).
#[derive(Debug, Clone)]
pub struct Timeline {
    lats: Vec<f64>,
    lons: Vec<f64>,
    cum: Vec<f64>,
    total: f64,
    /// Cumulative great-circle distance in meters at each vertex.
    dist_m: Vec<f64>,
    elevation: Option<Elevation>,
    /// Real track duration in ms, when built from timestamps.
    pub duration_ms: Option<f64>,
}

impl Timeline {
    /// Build a timeline from track points. Returns `None` when there are fewer
    /// than two points or the track has zero spatial and temporal extent.
    pub fn build(points: &[TrackPoint]) -> Option<Self> {
        if points.len() < 2 {
            return None;
        }
        let lats: Vec<f64> = points.iter().map(|p| p.lat).collect();
        let lons: Vec<f64> = points.iter().map(|p| p.lon).collect();
        let elevation = Elevation::build(points);
        let dist_m = haversine_metric(&lats, &lons);

        if let Some(cum) = time_metric(points) {
            let total = *cum.last()?;
            if total > 0.0 {
                return Some(Self {
                    lats,
                    lons,
                    cum,
                    total,
                    dist_m,
                    elevation,
                    duration_ms: Some(total),
                });
            }
        }

        let cum = distance_metric(&lats, &lons);
        let total = *cum.last()?;
        if total <= 0.0 {
            return None;
        }
        Some(Self {
            lats,
            lons,
            cum,
            total,
            dist_m,
            elevation,
            duration_ms: None,
        })
    }

    /// The segment containing progress `u`: vertex index `i` and the fraction
    /// `0..=1` advanced toward vertex `i + 1`.
    pub fn locate(&self, u: f64) -> (usize, f64) {
        let target = u.clamp(0.0, 1.0) * self.total;
        let i = self.segment_index(target);
        let (a, b) = (self.cum[i], self.cum[i + 1]);
        let frac = if b > a { (target - a) / (b - a) } else { 0.0 };
        (i, frac)
    }

    /// Interpolated position at progress `u` (clamped to `0..=1`).
    pub fn position_at(&self, u: f64) -> (f64, f64) {
        let (i, frac) = self.locate(u);
        (
            lerp(self.lats[i], self.lats[i + 1], frac),
            lerp(self.lons[i], self.lons[i + 1], frac),
        )
    }

    /// Distance traveled along the track at progress `u`, in meters.
    pub fn distance_at(&self, u: f64) -> f64 {
        let (i, frac) = self.locate(u);
        lerp(self.dist_m[i], self.dist_m[i + 1], frac)
    }

    /// Total track length in meters.
    pub fn total_distance_m(&self) -> f64 {
        self.dist_m.last().copied().unwrap_or(0.0)
    }

    /// Position `meters` along the track (clamped to its ends).
    pub fn position_at_distance(&self, meters: f64) -> (f64, f64) {
        let last = self.dist_m.len() - 1;
        let target = meters.clamp(0.0, self.dist_m[last]);
        let i = self
            .dist_m
            .partition_point(|&d| d <= target)
            .saturating_sub(1)
            .min(last - 1);
        let (a, b) = (self.dist_m[i], self.dist_m[i + 1]);
        let frac = if b > a { (target - a) / (b - a) } else { 0.0 };
        (
            lerp(self.lats[i], self.lats[i + 1], frac),
            lerp(self.lons[i], self.lons[i + 1], frac),
        )
    }

    /// Direction of travel at progress `u` as a compass bearing in degrees
    /// (`0` north, `90` east), smoothed over `window_m` meters of track
    /// centered on the marker.
    ///
    /// The bearing is the chord from the window's start to its end, which
    /// averages out GPS zigzags. Returns `None` when the window covers less
    /// than a meter of movement (e.g. a track that never moves).
    pub fn heading_at(&self, u: f64, window_m: f64) -> Option<f64> {
        let s = self.distance_at(u);
        let half = window_m.max(1.0) / 2.0;
        let total = self.total_distance_m();
        // Keep the full window length near the ends by sliding it inward.
        let (s0, s1) = if s - half < 0.0 {
            (0.0, (2.0 * half).min(total))
        } else if s + half > total {
            ((total - 2.0 * half).max(0.0), total)
        } else {
            (s - half, s + half)
        };
        let (lat0, lon0) = self.position_at_distance(s0);
        let (lat1, lon1) = self.position_at_distance(s1);
        if haversine_m(lat0, lon0, lat1, lon1) < 1.0 {
            return None;
        }
        Some(compass_bearing(lat0, lon0, lat1, lon1))
    }

    /// Elevation data, or `None` when no point carries `<ele>`.
    pub fn elevation(&self) -> Option<&Elevation> {
        self.elevation.as_ref()
    }

    /// Re-smooth the elevation over a centered window of `window_m` meters of
    /// track (`0` uses the recorded values); see [`Elevation::smooth`].
    pub fn set_elevation_smoothing(&mut self, window_m: f64) {
        if let Some(elevation) = self.elevation.as_mut() {
            elevation.smooth(window_m);
        }
    }

    /// Interpolated elevation in meters at progress `u`.
    pub fn elevation_at(&self, u: f64) -> Option<f64> {
        let (i, frac) = self.locate(u);
        self.elevation.as_ref().map(|e| e.at(i, frac))
    }

    /// Number of track vertices.
    pub fn len(&self) -> usize {
        self.lats.len()
    }

    /// Always `false`: a timeline needs at least two points.
    pub fn is_empty(&self) -> bool {
        self.lats.is_empty()
    }

    /// `(lat, lon)` of vertex `i`.
    pub fn vertex(&self, i: usize) -> (f64, f64) {
        (self.lats[i], self.lons[i])
    }

    /// Ordered samples covering `[u0, u1]`, including interpolated endpoints and
    /// every original vertex strictly between them. `u0` is clamped to `>= 0`.
    pub fn window(&self, u0: f64, u1: f64) -> Vec<Sample> {
        let u0 = u0.clamp(0.0, 1.0);
        let u1 = u1.clamp(0.0, 1.0);
        if u1 <= u0 {
            return Vec::new();
        }

        let mut out = vec![self.sample_at(u0)];
        for i in 0..self.cum.len() {
            let u = self.cum[i] / self.total;
            if u > u0 && u < u1 {
                out.push(Sample {
                    lat: self.lats[i],
                    lon: self.lons[i],
                    ele: self.elevation.as_ref().map(|e| e.vertex(i)),
                    u,
                });
            }
        }
        out.push(self.sample_at(u1));
        out
    }

    /// Instantaneous speed in km/h at progress `u`, or `None` for a
    /// distance-based timeline (no real timestamps to differentiate against).
    pub fn speed_kmh_at(&self, u: f64) -> Option<f64> {
        self.duration_ms?;
        let target = u.clamp(0.0, 1.0) * self.total;
        let i = self.segment_index(target);
        let dt_ms = self.cum[i + 1] - self.cum[i];
        if dt_ms <= 0.0 {
            return Some(0.0);
        }
        let meters = haversine_m(
            self.lats[i],
            self.lons[i],
            self.lats[i + 1],
            self.lons[i + 1],
        );
        Some(meters / (dt_ms / 1000.0) * 3.6)
    }

    fn sample_at(&self, u: f64) -> Sample {
        let (lat, lon) = self.position_at(u);
        Sample {
            lat,
            lon,
            ele: self.elevation_at(u),
            u,
        }
    }

    fn segment_index(&self, target: f64) -> usize {
        // Largest i with cum[i] <= target, bounded so i+1 is valid.
        match self
            .cum
            .binary_search_by(|v| v.partial_cmp(&target).unwrap())
        {
            Ok(i) => i.min(self.cum.len() - 2),
            Err(i) => i.saturating_sub(1).min(self.cum.len() - 2),
        }
    }
}

/// Cumulative elapsed-time metric, or `None` if any timestamp is missing or
/// unparseable (or time runs backwards).
fn time_metric(points: &[TrackPoint]) -> Option<Vec<f64>> {
    let mut times = Vec::with_capacity(points.len());
    for p in points {
        let ms = parse_epoch_ms(p.time.as_deref()?)?;
        times.push(ms);
    }
    let t0 = times[0];
    let mut cum = Vec::with_capacity(times.len());
    for &t in &times {
        let elapsed = (t - t0) as f64;
        if elapsed < cum.last().copied().unwrap_or(0.0) {
            return None;
        }
        cum.push(elapsed);
    }
    Some(cum)
}

/// Cumulative geographic distance metric (equirectangular approximation).
fn distance_metric(lats: &[f64], lons: &[f64]) -> Vec<f64> {
    let mut cum = Vec::with_capacity(lats.len());
    cum.push(0.0);
    for i in 1..lats.len() {
        let mean_lat = ((lats[i] + lats[i - 1]) / 2.0).to_radians();
        let dx = (lons[i] - lons[i - 1]) * mean_lat.cos();
        let dy = lats[i] - lats[i - 1];
        cum.push(cum[i - 1] + dx.hypot(dy));
    }
    cum
}

/// Cumulative great-circle distance in meters at each vertex.
fn haversine_metric(lats: &[f64], lons: &[f64]) -> Vec<f64> {
    let mut cum = Vec::with_capacity(lats.len());
    cum.push(0.0);
    for i in 1..lats.len() {
        cum.push(cum[i - 1] + haversine_m(lats[i - 1], lons[i - 1], lats[i], lons[i]));
    }
    cum
}

/// Compass bearing (degrees, `0..360`) of the screen direction from one
/// point to another in Web Mercator, matching the camera's bearing.
fn compass_bearing(lat0: f64, lon0: f64, lat1: f64, lon1: f64) -> f64 {
    use crate::geo::mercator::{lat_to_pixel, lon_to_pixel};
    let dx = lon_to_pixel(lon1, 20) - lon_to_pixel(lon0, 20);
    let dy = lat_to_pixel(lat1, 20) - lat_to_pixel(lat0, 20);
    dx.atan2(-dy).to_degrees().rem_euclid(360.0)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Great-circle distance in meters between two WGS84 coordinates.
pub(crate) fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed(coords: &[(f64, f64, &str)]) -> Vec<TrackPoint> {
        coords
            .iter()
            .map(|&(lat, lon, t)| {
                let mut p = TrackPoint::new(lat, lon);
                p.time = Some(t.to_string());
                p
            })
            .collect()
    }

    #[test]
    fn endpoints_map_to_first_and_last() {
        let pts = timed(&[
            (0.0, 0.0, "2024-01-01T00:00:00Z"),
            (0.0, 10.0, "2024-01-01T00:01:00Z"),
        ]);
        let tl = Timeline::build(&pts).unwrap();
        assert_eq!(tl.position_at(0.0), (0.0, 0.0));
        assert_eq!(tl.position_at(1.0), (0.0, 10.0));
    }

    #[test]
    fn time_metric_reflects_pauses() {
        // Equal distance per leg, but the second leg takes 3x longer in time.
        let pts = timed(&[
            (0.0, 0.0, "2024-01-01T00:00:00Z"),
            (0.0, 10.0, "2024-01-01T00:00:15Z"),
            (0.0, 20.0, "2024-01-01T00:01:00Z"),
        ]);
        let tl = Timeline::build(&pts).unwrap();
        assert_eq!(tl.duration_ms, Some(60_000.0));
        // At u=0.25 (15s of 60s) we are exactly at the midpoint vertex.
        let (_, lon) = tl.position_at(0.25);
        assert!((lon - 10.0).abs() < 1e-6, "lon was {lon}");
    }

    #[test]
    fn falls_back_to_distance_without_time() {
        let pts = vec![
            TrackPoint::new(0.0, 0.0),
            TrackPoint::new(0.0, 10.0),
            TrackPoint::new(0.0, 20.0),
        ];
        let tl = Timeline::build(&pts).unwrap();
        assert_eq!(tl.duration_ms, None);
        let (_, lon) = tl.position_at(0.5);
        assert!((lon - 10.0).abs() < 1e-6, "lon was {lon}");
    }

    #[test]
    fn window_includes_interpolated_endpoints() {
        let pts = timed(&[
            (0.0, 0.0, "2024-01-01T00:00:00Z"),
            (0.0, 10.0, "2024-01-01T00:01:00Z"),
        ]);
        let tl = Timeline::build(&pts).unwrap();
        let w = tl.window(0.25, 0.75);
        assert_eq!(w.first().unwrap().u, 0.25);
        assert_eq!(w.last().unwrap().u, 0.75);
        assert!((w.first().unwrap().lon - 2.5).abs() < 1e-6);
        assert!((w.last().unwrap().lon - 7.5).abs() < 1e-6);
    }

    #[test]
    fn too_few_points_is_none() {
        assert!(Timeline::build(&[TrackPoint::new(0.0, 0.0)]).is_none());
    }

    #[test]
    fn speed_matches_known_motion() {
        // ~1 degree of longitude at the equator (~111.3 km) over one hour.
        let pts = timed(&[
            (0.0, 0.0, "2024-01-01T00:00:00Z"),
            (0.0, 1.0, "2024-01-01T01:00:00Z"),
        ]);
        let tl = Timeline::build(&pts).unwrap();
        let kmh = tl.speed_kmh_at(0.5).unwrap();
        assert!((kmh - 111.3).abs() < 1.0, "speed was {kmh}");
    }

    #[test]
    fn samples_carry_interpolated_elevation() {
        let mut pts = vec![TrackPoint::new(0.0, 0.0), TrackPoint::new(0.0, 1.0)];
        pts[0].ele = Some(100.0);
        pts[1].ele = Some(200.0);
        let tl = Timeline::build(&pts).unwrap();
        assert_eq!(tl.elevation_at(0.5), Some(150.0));
        let w = tl.window(0.0, 0.5);
        assert_eq!(w.last().unwrap().ele, Some(150.0));
    }

    /// ~1.1 km east along the equator, then ~1.1 km north.
    fn east_then_north() -> Vec<TrackPoint> {
        [(0.0, 0.0), (0.0, 0.01), (0.01, 0.01)]
            .iter()
            .map(|&(lat, lon)| TrackPoint::new(lat, lon))
            .collect()
    }

    #[test]
    fn heading_follows_direction_of_travel() {
        let tl = Timeline::build(&east_then_north()).unwrap();
        let east = tl.heading_at(0.25, 100.0).unwrap();
        let north = tl.heading_at(0.75, 100.0).unwrap();
        assert!((east - 90.0).abs() < 0.5, "east {east}");
        assert!(!(0.5..=359.5).contains(&north), "north {north}");
        let corner = tl.heading_at(0.5, 400.0).unwrap();
        assert!((corner - 45.0).abs() < 1.0, "corner {corner}");
    }

    #[test]
    fn heading_window_slides_inward_at_the_ends() {
        let tl = Timeline::build(&east_then_north()).unwrap();
        let start = tl.heading_at(0.0, 100.0).unwrap();
        assert!((start - 90.0).abs() < 0.5, "start {start}");
    }

    #[test]
    fn heading_is_none_without_movement() {
        let mut pts = timed(&[
            (0.0, 0.0, "2024-01-01T00:00:00Z"),
            (0.0, 0.0, "2024-01-01T00:01:00Z"),
        ]);
        pts[1].lat = 0.000_001;
        let tl = Timeline::build(&pts).unwrap();
        assert_eq!(tl.heading_at(0.5, 100.0), None);
    }

    #[test]
    fn position_at_distance_interpolates() {
        let tl = Timeline::build(&east_then_north()).unwrap();
        let half = tl.total_distance_m() / 4.0;
        let (lat, lon) = tl.position_at_distance(half);
        assert!(
            lat.abs() < 1e-9 && (lon - 0.005).abs() < 1e-5,
            "{lat} {lon}"
        );
    }

    #[test]
    fn speed_is_none_without_timestamps() {
        let pts = vec![TrackPoint::new(0.0, 0.0), TrackPoint::new(0.0, 1.0)];
        assert!(Timeline::build(&pts).unwrap().speed_kmh_at(0.5).is_none());
    }
}
