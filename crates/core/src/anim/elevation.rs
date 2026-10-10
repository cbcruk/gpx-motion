use super::timeline::haversine_m;
use crate::gpx::model::TrackPoint;

/// Minimum elevation change (m) counted toward gain/loss, filtering GPS jitter.
const GAIN_THRESHOLD_M: f64 = 2.0;

/// Per-vertex elevation, distance, and cumulative climb for a track.
///
/// Points without `<ele>` borrow the nearest preceding known value (or the
/// first known one at the start). Gain and loss use a small hysteresis so
/// noise around a flat elevation does not accumulate as climbing.
#[derive(Debug, Clone)]
pub struct Elevation {
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
    /// Build from track points, or `None` when no point carries an elevation.
    pub fn build(points: &[TrackPoint]) -> Option<Self> {
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

        let (mut gain, mut loss) = (0.0, 0.0);
        let mut anchor = ele[0];
        let mut cum_gain = Vec::with_capacity(ele.len());
        let mut cum_loss = Vec::with_capacity(ele.len());
        for &e in &ele {
            let delta = e - anchor;
            if delta >= GAIN_THRESHOLD_M {
                gain += delta;
                anchor = e;
            } else if delta <= -GAIN_THRESHOLD_M {
                loss -= delta;
                anchor = e;
            }
            cum_gain.push(gain);
            cum_loss.push(loss);
        }

        let min = ele.iter().copied().fold(f64::INFINITY, f64::min);
        let max = ele.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Some(Self {
            ele,
            dist_m,
            cum_gain,
            cum_loss,
            min,
            max,
        })
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

fn lerp_at(values: &[f64], i: usize, frac: f64) -> f64 {
    let last = values.len() - 1;
    let a = values[i.min(last)];
    let b = values[(i + 1).min(last)];
    a + (b - a) * frac
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

    #[test]
    fn flat_track_normalizes_to_middle() {
        let pts = with_ele(&[(0.0, 0.0, Some(5.0)), (0.0, 1.0, Some(5.0))]);
        assert_eq!(Elevation::build(&pts).unwrap().normalize(5.0), 0.5);
    }
}
