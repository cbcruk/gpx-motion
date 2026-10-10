/// Turn per-frame headings into camera bearings that never swing faster than
/// `max_step_deg` between consecutive frames.
///
/// Headings are compass bearings in degrees; `None` (no movement) holds the
/// previous bearing, and leading `None`s take the first known heading. The
/// result is unwrapped (it may leave `0..360`) so it stays continuous, and
/// each step turns the short way around. A non-positive `max_step_deg`
/// disables the limit.
pub fn steer(headings: &[Option<f64>], max_step_deg: f64) -> Vec<f64> {
    let Some(first) = headings.iter().flatten().next().copied() else {
        return vec![0.0; headings.len()];
    };
    let mut bearing = first;
    headings
        .iter()
        .map(|heading| {
            if let Some(target) = heading {
                let delta = (target - bearing + 180.0).rem_euclid(360.0) - 180.0;
                let step = if max_step_deg > 0.0 {
                    delta.clamp(-max_step_deg, max_step_deg)
                } else {
                    delta
                };
                bearing += step;
            }
            bearing
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_the_short_way_across_north() {
        let out = steer(&[Some(350.0), Some(10.0)], 0.0);
        assert_eq!(out, vec![350.0, 370.0]);
    }

    #[test]
    fn limits_turn_rate() {
        let out = steer(&[Some(0.0), Some(90.0), Some(90.0), Some(90.0)], 30.0);
        assert_eq!(out, vec![0.0, 30.0, 60.0, 90.0]);
    }

    #[test]
    fn holds_bearing_while_stopped() {
        let out = steer(&[None, Some(45.0), None, Some(45.0)], 10.0);
        assert_eq!(out, vec![45.0, 45.0, 45.0, 45.0]);
    }

    #[test]
    fn no_movement_faces_north() {
        assert_eq!(steer(&[None, None], 5.0), vec![0.0, 0.0]);
    }
}
