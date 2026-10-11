//! A quick hasher for label memory keys, which are rebuilt every view.

use std::hash::{BuildHasherDefault, Hasher};

/// The FxHash mix (as in rustc): fast on small integer keys, and plenty for
/// keys that aren't chosen by an adversary.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KeyHasher(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(SEED);
    }

    fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Builds [`KeyHasher`]s for `HashMap`s and `HashSet`s.
pub(crate) type KeyHash = BuildHasherDefault<KeyHasher>;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn keys_stay_apart() {
        let mut map: HashMap<(u64, i64, i64), u32, KeyHash> = HashMap::default();
        for x in -50..50 {
            for y in -50..50 {
                map.insert((7, x, y), (x * 1000 + y) as u32);
            }
        }
        assert_eq!(map.len(), 10_000);
        assert_eq!(map[&(7, -3, 41)], (-3000 + 41_i64) as u32);
    }
}
