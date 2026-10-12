//! What labels showed in earlier views, keyed stably, and how far each has faded.

use std::collections::HashMap;

use crate::vector::key_hash::KeyHash;

/// What a sequence of views (video frames, or a turning camera) remembers
/// about its labels, so they hold still from one view to the next.
///
/// Within a priority tier, labels shown in the previous view are placed
/// first, so a label of similar importance can't push them out and back in
/// as the camera moves; a more important label still wins. Labels fade in
/// and out over `fade_frames` views (the first view shows everything at
/// once), including ones the terrain starts to hide.
#[derive(Debug, Clone, Default)]
pub struct LabelMemory {
    /// Fade level (`1..=fade_frames`) of each label shown last view, keyed
    /// by name and coarse place so two places sharing a name stay apart.
    pub(super) levels: HashMap<LabelKey, u32, KeyHash>,
    /// Copies each line label (by each of its keys) drew last view.
    pub(super) copies: HashMap<LabelKey, usize, KeyHash>,
    pub(super) fade_frames: u32,
    pub(super) primed: bool,
}

/// A label's name and its position on a coarse world grid.
pub(super) type LabelKey = (u64, i64, i64);

/// A stable hash of a label's name for [`LabelKey`]s.
pub(super) fn name_key(kind: &str, text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (kind, text).hash(&mut hasher);
    hasher.finish()
}

/// Grid of [`LabelKey`] positions: world pixels at this zoom (`256·2^KEY_ZOOM`
/// cells per side, tens of meters each), far finer than places sharing a
/// name are apart.
pub(super) const KEY_ZOOM: i32 = 12;

impl LabelMemory {
    /// An empty memory whose labels fade over `fade_frames` views (`0` or
    /// `1`: no fade).
    pub fn new(fade_frames: u32) -> Self {
        Self {
            levels: HashMap::default(),
            copies: HashMap::default(),
            fade_frames,
            primed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::labels::style::MAX_LABELS;
    use crate::vector::labels::test_support::*;
    use crate::vector::mvt::TestPoint;

    #[test]
    fn labels_shown_before_keep_their_place_under_the_cap() {
        // More cities than the cap, in rows above and below a town.
        let rows = [30, 75, 120, 165, 210, 300, 345, 390, 435, 480];
        let mut points: Vec<TestPoint> = rows
            .iter()
            .flat_map(|&y| (0..7).map(move |c| (40 + 72 * c, y)))
            .enumerate()
            .map(|(i, (x, y))| place(&format!("C{i}"), "city", x * 8, y * 8))
            .collect();
        let town = place("Oldtown", "town", 256 * 8, 256 * 8);
        let mut memory = LabelMemory::new(1);
        let alone = steady(&tile(std::slice::from_ref(&town)), &mut memory);
        assert_eq!(names(&alone), ["Oldtown"]);
        points.push(town);
        let crowded = tile(&points);
        let fresh = steady(&crowded, &mut LabelMemory::new(1));
        assert_eq!(fresh.len(), MAX_LABELS);
        assert!(
            !names(&fresh).contains(&"Oldtown"),
            "a new town waits for room"
        );
        // The town shown before keeps its place; the cities take the rest.
        let kept = steady(&crowded, &mut memory);
        assert!(names(&kept).contains(&"Oldtown"));
        assert_eq!(kept.len(), MAX_LABELS);
        // And the count holds view after view.
        assert_eq!(steady(&crowded, &mut memory).len(), MAX_LABELS);
    }

    #[test]
    fn labels_shown_before_keep_their_place_within_a_tier() {
        let old = place("Oldtown", "town", 2048, 2048);
        let new = place("Newtown", "town", 2060, 2050);
        let mut memory = LabelMemory::new(1);
        steady(&tile(std::slice::from_ref(&old)), &mut memory);
        let both = tile(&[old, new]);
        assert_eq!(names(&steady(&both, &mut memory)), ["Oldtown"]);
        assert_eq!(names(&steady(&both, &mut LabelMemory::new(1))), ["Newtown"]);
    }

    #[test]
    fn a_more_important_label_still_wins() {
        let minor = place("Smallville", "village", 2048, 2048);
        let major = place("Metropolis", "city", 2060, 2050);
        let mut memory = LabelMemory::new(1);
        steady(&tile(std::slice::from_ref(&minor)), &mut memory);
        assert_eq!(
            names(&steady(&tile(&[minor, major]), &mut memory)),
            ["Metropolis"]
        );
    }

    #[test]
    fn places_sharing_a_name_are_remembered_apart() {
        let here = place("Springfield", "town", 600, 600);
        let there = place("Springfield", "village", 3000, 3000);
        let major = place("Metropolis", "city", 3010, 3005);
        let mut memory = LabelMemory::new(4);
        steady(&tile(&[here]), &mut memory);
        let next = steady(&tile(&[there, major]), &mut memory);
        assert_eq!(names(&next), ["Metropolis"]);
        assert!(next[0].opacity < 1.0, "a new place fades in");
    }

    #[test]
    fn new_labels_fade_in_after_the_first_view() {
        let a = place("Elsewhere", "town", 600, 600);
        let b = place("Metropolis", "city", 3000, 3000);
        let mut memory = LabelMemory::new(4);
        let first = steady(&tile(std::slice::from_ref(&a)), &mut memory);
        assert_eq!(first[0].opacity, 1.0, "the first view shows everything");
        let mut seen = Vec::new();
        for _ in 0..5 {
            let labels = steady(&tile(&[a.clone(), b.clone()]), &mut memory);
            let opacity = |name: &str| labels.iter().find(|l| l.text == name).unwrap().opacity;
            assert_eq!(opacity("Elsewhere"), 1.0);
            seen.push(opacity("Metropolis"));
        }
        assert_eq!(seen, [0.25, 0.5, 0.75, 1.0, 1.0]);
    }
}
