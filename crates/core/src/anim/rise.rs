//! Easing per-frame needs over time, so a camera rises ahead of what it
//! must clear.

/// Spread per-frame needs (e.g. how far a camera must rise to see past a
/// hill) over time so a camera meets them smoothly: each frame gets the
/// average, over `window` frames either side, of the largest need within
/// `2 × window` frames. So it never falls below a frame's own need: a need
/// starting at frame `k` is eased into from `k − 3 × window` and fully met
/// by `k − window`; after it ends the value holds for `window` frames, then
/// eases out over `2 × window`.
pub fn rise_ahead(needs: &[f64], window: usize) -> Vec<f64> {
    if window == 0 || needs.is_empty() {
        return needs.to_vec();
    }
    let n = needs.len();
    let span = |i: usize, r: usize| (i.saturating_sub(r), (i + r).min(n - 1));
    let peaks: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b) = span(i, 2 * window);
            needs[a..=b].iter().copied().fold(0.0, f64::max)
        })
        .collect();
    (0..n)
        .map(|i| {
            let (a, b) = span(i, window);
            peaks[a..=b].iter().sum::<f64>() / (b - a + 1) as f64
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_around_a_need_and_never_falls_short() {
        let mut needs = vec![0.0; 40];
        for n in &mut needs[18..22] {
            *n = 10.0;
        }
        let eased = rise_ahead(&needs, 4);
        assert!(eased.iter().zip(&needs).all(|(e, n)| e >= n));
        assert!(eased[10] > 0.0 && eased[10] < 10.0, "eases in early");
        assert_eq!(eased[0], 0.0);
        assert_eq!(eased[39], 0.0);
        let steps: f64 = eased
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max);
        assert!(
            steps <= 10.0 / 9.0 + 1e-9,
            "no jump bigger than one window step"
        );
        assert_eq!(rise_ahead(&needs, 0), needs);
    }
}
