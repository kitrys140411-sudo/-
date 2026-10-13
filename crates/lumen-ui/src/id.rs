//! Widget identities. An id is a hash of the parent's id and a key (a label,
//! an index, anything hashable), so it stays the same from frame to frame as
//! long as the widget is created in the same place. Widget state (animations,
//! scroll offsets, text cursors) and interactions are keyed by it.

use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(pub u64);

/// FNV-1a: small, fast and stable across runs (ids show up in test logs).
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

impl Id {
    pub const ROOT: Id = Id(0xcbf29ce484222325);

    /// The id of `key` under `self`.
    pub fn with(self, key: impl Hash) -> Id {
        let mut h = Fnv(self.0);
        key.hash(&mut h);
        Id(h.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_distinct() {
        assert_eq!(Id::ROOT.with("a"), Id::ROOT.with("a"));
        assert_ne!(Id::ROOT.with("a"), Id::ROOT.with("b"));
        assert_ne!(Id::ROOT.with("a").with(1), Id::ROOT.with("a").with(2));
        assert_ne!(Id::ROOT.with("a").with("b"), Id::ROOT.with("b").with("a"));
    }
}
