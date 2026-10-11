//! Grid lines of a terrain mesh: dense over the area the map is finest in,
//! sparser around it.

/// Most grid cells along either side of an area; larger areas use coarser
/// cells to bound the triangle count.
const MAX_CELLS: f64 = 64.0;
/// Smallest grid cell in world pixels.
const MIN_CELL_PX: f64 = 12.0;

/// Offsets of the grid lines along one side `size` long, from `0` to `size`.
///
/// They are evenly spaced, or with an `inner` range `(start, end)` inside,
/// spaced for that range within it (landing on its ends) and for the whole
/// side outside it, so the two meet on shared lines.
pub(crate) fn lines(size: f64, longest: f64, inner: Option<(f64, f64, f64)>) -> Vec<f64> {
    let spacing = |longest: f64| (longest / MAX_CELLS).max(MIN_CELL_PX);
    let Some((start, end, inner_longest)) = inner else {
        return even(0.0, size, spacing(longest));
    };
    let (start, end) = (start.clamp(0.0, size), end.clamp(0.0, size));
    let outer = spacing(longest);
    let mut out = even(0.0, start, outer);
    out.pop();
    out.extend(even(start, end, spacing(inner_longest)));
    out.pop();
    out.extend(even(end, size, outer));
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    out
}

/// Lines from `start` to `end` about `spacing` apart, both ends included.
fn even(start: f64, end: f64, spacing: f64) -> Vec<f64> {
    let cells = ((end - start) / spacing).round().max(1.0) as usize;
    (0..=cells)
        .map(|i| match i {
            i if i == cells => end,
            i => start + (end - start) * i as f64 / cells as f64,
        })
        .collect()
}

/// The cell of `lines` holding `v` and how far across it `v` lies (`0..=1`),
/// or `None` outside the lines.
pub(crate) fn locate(lines: &[f64], v: f64) -> Option<(usize, f64)> {
    let (first, last) = (*lines.first()?, *lines.last()?);
    if lines.len() < 2 || v.is_nan() || v < first || v > last {
        return None;
    }
    let cell = lines.partition_point(|&l| l <= v).clamp(1, lines.len() - 1) - 1;
    let (a, b) = (lines[cell], lines[cell + 1]);
    Some((cell, ((v - a) / (b - a)).clamp(0.0, 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_lines_match_the_cell_size() {
        let xs = lines(1280.0, 1280.0, None);
        assert_eq!(xs.len(), 65);
        assert_eq!((xs[0], xs[1], xs[64]), (0.0, 20.0, 1280.0));
        assert_eq!(lines(100.0, 100.0, None).len(), 9);
    }

    #[test]
    fn an_inner_range_gets_its_own_spacing_on_shared_lines() {
        let xs = lines(6400.0, 6400.0, Some((2000.0, 2640.0, 640.0)));
        assert!(xs.windows(2).all(|w| w[0] < w[1]));
        assert!(xs.contains(&2000.0) && xs.contains(&2640.0));
        let inside = xs
            .iter()
            .filter(|&&x| (2000.0..=2640.0).contains(&x))
            .count();
        assert_eq!(inside, 54, "640 px at 12 px cells");
        let gaps: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().all(|&g| g > 11.0 && g < 101.0), "{gaps:?}");
        assert_eq!((xs[0], *xs.last().unwrap()), (0.0, 6400.0));
    }

    #[test]
    fn locate_finds_the_cell_and_fraction() {
        let xs = [0.0, 10.0, 30.0];
        assert_eq!(locate(&xs, 5.0), Some((0, 0.5)));
        assert_eq!(locate(&xs, 20.0), Some((1, 0.5)));
        assert_eq!(locate(&xs, 30.0), Some((1, 1.0)));
        assert_eq!(locate(&xs, 0.0), Some((0, 0.0)));
        assert_eq!(locate(&xs, -1.0), None);
        assert_eq!(locate(&xs, 31.0), None);
    }
}
