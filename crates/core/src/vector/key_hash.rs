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
            self.write_u64(u64::from(b));
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
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn nearby_keys_hash_apart() {
        use std::hash::BuildHasher;
        // Coarse grid cells around one name: their hashes, and the low bits a
        // map buckets them by, stay distinct.
        let hash = |key: (u64, i64, i64)| KeyHash::default().hash_one(key);
        let hashes: Vec<u64> = (-16..16)
            .flat_map(|x| (-16..16).map(move |y| hash((0x9e37_79b9_7f4a_7c15, x, y))))
            .collect();
        let distinct: HashSet<u64> = hashes.iter().copied().collect();
        assert_eq!(distinct.len(), hashes.len());
        let buckets: HashSet<u64> = hashes.iter().map(|h| h >> 54).collect();
        assert!(buckets.len() > 600, "{} of 1024 buckets", buckets.len());
    }
}
