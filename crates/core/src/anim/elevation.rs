use super::timeline::haversine_m;
use crate::gpx::model::TrackPoint;
use crate::math::lerp;

/// Minimum elevation change (m) counted toward gain/loss, filtering GPS jitter.
const GAIN_THRESHOLD_M: f64 = 3.0;

/// Default smoothing window (m of track distance) applied by [`Elevation::build`].
pub const DEFAULT_SMOOTHING_M: f64 = 100.0;

/// Per-vertex elevation, distance, and cumulative climb for a track.
///
/// Points without `<ele>` borrow the nearest preceding known value (or the
/// first known one at the start). Recorded elevations are smoothed with a
/// distance-based moving average, and gain and loss use a small hysteresis,
/// so GPS noise on flat ground does not accumulate as climbing.
#[derive(Debug, Clone)]
pub struct Elevation {
    raw: Vec<f64>,
    ele: Vec<f64>,
    dist_m: Vec<f64>,
    cum_gain: Vec<f64>,
    cum_loss: Vec<f64>,
    /// Lowest elevation on the track, in meters.
    pub min: f64,
    /// Highest elevation on the track, in meters.
    pub max: f64,
}

impl Elevation {
    /// Build from track points with [`DEFAULT_SMOOTHING_M`], or `None` when no
    /// point carries an elevation.
    pub fn build(points: &[TrackPoint]) -> Option<Self> {
        Self::build_smoothed(points, DEFAULT_SMOOTHING_M)
    }

    /// Build from track points, averaging each elevation over a centered
    /// window of `window_m` meters of track (`0` keeps the recorded values).
    pub fn build_smoothed(points: &[TrackPoint], window_m: f64) -> Option<Self> {
        let first_known = points.iter().find_map(|p| p.ele)?;
        let mut last = first_known;
        let ele: Vec<f64> = points
            .iter()
            .map(|p| {
                if let Some(e) = p.ele {
                    last = e;
                }
                last
            })
            .collect();

        let mut dist_m = Vec::with_capacity(points.len());
        dist_m.push(0.0);
        for w in points.windows(2) {
            let d = haversine_m(w[0].lat, w[0].lon, w[1].lat, w[1].lon);
            dist_m.push(dist_m.last().copied().unwrap_or(0.0) + d);
        }

        let mut elevation = Self {
            raw: ele,
            ele: Vec::new(),
            dist_m,
            cum_gain: Vec::new(),
            cum_loss: Vec::new(),
            min: 0.0,
            max: 0.0,
        };
        elevation.smooth(window_m);
        Some(elevation)
    }

    /// Re-derive elevations, range, and climb from the recorded values with a
    /// new smoothing window (meters; `0` disables smoothing).
    pub fn smooth(&mut self, window_m: f64) {
        self.ele = moving_average(&self.raw, &self.dist_m, window_m.max(0.0) / 2.0);

        let (mut gain, mut loss) = (0.0, 0.0);
        let mut anchor = self.ele[0];
        self.cum_gain.clear();
        self.cum_loss.clear();
        for &e in &self.ele {
            let delta = e - anchor;
            if delta >= GAIN_THRESHOLD_M {
                gain += delta;
                anchor = e;
            } else if delta <= -GAIN_THRESHOLD_M {
                loss -= delta;
                anchor = e;
            }
            self.cum_gain.push(gain);
            self.cum_loss.push(loss);
        }

        self.min = self.ele.iter().copied().fold(f64::INFINITY, f64::min);
        self.max = self.ele.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    }

    /// Elevation (m) at vertex `i` advanced `frac` toward vertex `i + 1`.
    pub fn at(&self, i: usize, frac: f64) -> f64 {
        lerp_at(&self.ele, i, frac)
    }

    /// Distance from the start (m) at vertex `i` advanced `frac` toward `i + 1`.
    pub fn distance_at(&self, i: usize, frac: f64) -> f64 {
        lerp_at(&self.dist_m, i, frac)
    }

    /// Cumulative `(gain, loss)` in meters up to vertex `i`.
    pub fn gain_loss_at(&self, i: usize) -> (f64, f64) {
        let i = i.min(self.ele.len() - 1);
        (self.cum_gain[i], self.cum_loss[i])
    }

    /// Elevation of vertex `i` (m).
    pub fn vertex(&self, i: usize) -> f64 {
        self.ele[i.min(self.ele.len() - 1)]
    }

    /// Total track length in meters.
    pub fn total_distance_m(&self) -> f64 {
        self.dist_m.last().copied().unwrap_or(0.0)
    }

    /// `(distance_m, elevation_m)` for every vertex, in track order.
    pub fn profile(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.dist_m.iter().copied().zip(self.ele.iter().copied())
    }

    /// Map an elevation onto `0..=1` across the track's range (`0.5` when flat).
    pub fn normalize(&self, ele: f64) -> f64 {
        let span = self.max - self.min;
        if span <= f64::EPSILON {
            0.5
        } else {
            ((ele - self.min) / span).clamp(0.0, 1.0)
        }
    }
}

/// Mean of `values` whose distance lies within `half_m` of each vertex.
fn moving_average(values: &[f64], dist_m: &[f64], half_m: f64) -> Vec<f64> {
    if half_m <= 0.0 {
        return values.to_vec();
    }
    let mut prefix = Vec::with_capacity(values.len() + 1);
    prefix.push(0.0);
    for v in values {
        prefix.push(prefix.last().copied().unwrap_or(0.0) + v);
    }
    let (mut lo, mut hi) = (0, 0);
    (0..values.len())
        .map(|i| {
            while dist_m[i] - dist_m[lo] > half_m {
                lo += 1;
            }
            while hi + 1 < values.len() && dist_m[hi + 1] - dist_m[i] <= half_m {
                hi += 1;
            }
            (prefix[hi + 1] - prefix[lo]) / (hi + 1 - lo) as f64
        })
        .collect()
}

fn lerp_at(values: &[f64], i: usize, frac: f64) -> f64 {
    let last = values.len() - 1;
    let a = values[i.min(last)];
    let b = values[(i + 1).min(last)];
    lerp(a, b, frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_ele(coords: &[(f64, f64, Option<f64>)]) -> Vec<TrackPoint> {
        coords
            .iter()
            .map(|&(lat, lon, ele)| {
                let mut p = TrackPoint::new(lat, lon);
                p.ele = ele;
                p
            })
            .collect()
    }

    #[test]
    fn none_without_any_elevation() {
        let pts = with_ele(&[(0.0, 0.0, None), (0.0, 1.0, None)]);
        assert!(Elevation::build(&pts).is_none());
    }

    #[test]
    fn missing_values_borrow_neighbors() {
        let pts = with_ele(&[(0.0, 0.0, None), (0.0, 0.1, Some(100.0)), (0.0, 0.2, None)]);
        let e = Elevation::build(&pts).unwrap();
        assert_eq!(
            (e.vertex(0), e.vertex(1), e.vertex(2)),
            (100.0, 100.0, 100.0)
        );
    }

    #[test]
    fn gain_and_loss_ignore_small_jitter() {
        let pts = with_ele(&[
            (0.0, 0.0, Some(100.0)),
            (0.0, 0.1, Some(101.0)),
            (0.0, 0.2, Some(100.0)),
            (0.0, 0.3, Some(150.0)),
            (0.0, 0.4, Some(120.0)),
        ]);
        let e = Elevation::build(&pts).unwrap();
        assert_eq!(e.gain_loss_at(2), (0.0, 0.0));
        assert_eq!(e.gain_loss_at(4), (50.0, 30.0));
        assert_eq!((e.min, e.max), (100.0, 150.0));
    }

    #[test]
    fn interpolates_between_vertices() {
        let pts = with_ele(&[(0.0, 0.0, Some(0.0)), (0.0, 1.0, Some(100.0))]);
        let e = Elevation::build(&pts).unwrap();
        assert_eq!(e.at(0, 0.25), 25.0);
        assert!((e.distance_at(0, 0.5) - e.total_distance_m() / 2.0).abs() < 1e-6);
    }

    /// ~11 m steps along the equator, alternating ±3 m around 100 m, then a
    /// steady 50 m climb.
    fn noisy_then_climb() -> Vec<TrackPoint> {
        let step = 0.0001;
        let mut coords: Vec<(f64, f64, Option<f64>)> = (0..100)
            .map(|i| {
                (
                    0.0,
                    i as f64 * step,
                    Some(if i % 2 == 0 { 97.0 } else { 103.0 }),
                )
            })
            .collect();
        coords
            .extend((0..100).map(|i| (0.0, (100 + i) as f64 * step, Some(100.0 + i as f64 * 0.5))));
        with_ele(&coords)
    }

    #[test]
    fn smoothing_removes_jitter_but_keeps_real_climbs() {
        let pts = noisy_then_climb();
        let raw = Elevation::build_smoothed(&pts, 0.0).unwrap();
        let smooth = Elevation::build(&pts).unwrap();
        let (raw_gain, _) = raw.gain_loss_at(pts.len() - 1);
        let (gain, _) = smooth.gain_loss_at(pts.len() - 1);
        assert!(raw_gain > 300.0, "raw gain {raw_gain}");
        assert!((40.0..=55.0).contains(&gain), "smoothed gain {gain}");
    }

    #[test]
    fn zero_window_keeps_recorded_values() {
        let pts = noisy_then_climb();
        let mut e = Elevation::build(&pts).unwrap();
        e.smooth(0.0);
        assert_eq!((e.vertex(0), e.vertex(1)), (97.0, 103.0));
    }

    #[test]
    fn flat_track_normalizes_to_middle() {
        let pts = with_ele(&[(0.0, 0.0, Some(5.0)), (0.0, 1.0, Some(5.0))]);
        assert_eq!(Elevation::build(&pts).unwrap().normalize(5.0), 0.5);
    }
}
