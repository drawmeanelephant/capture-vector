//! Determinism ports. Everything that needs "now" or "a new id" goes
//! through these — tests substitute `FixedClock` and a seeded `IdGen`.

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(pub i64);

/// SHA-256 content hash, serialized as lowercase hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub fn from_hex(s: &str) -> crate::Result<Self> {
        if s.len() != 64 {
            return Err(crate::Error::Other(format!("hash hex len {}", s.len())));
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                .map_err(|e| crate::Error::Other(e.to_string()))?;
        }
        Ok(Self(out))
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Hash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

pub trait Clock {
    fn now(&self) -> Timestamp;
}

/// Deterministic id source. Implementations must be seedable so tests
/// and replays produce identical streams.
pub trait IdGen {
    fn next(&mut self) -> crate::id::StableId;
}

/// Fixed clock for tests and golden replays.
pub struct FixedClock(pub Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// Seeded counter-based `IdGen` for tests: `id-000000`, `id-000001`, …
pub struct SeqIdGen {
    pub prefix: String,
    pub next: u64,
}

impl SeqIdGen {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            next: 0,
        }
    }
}

impl IdGen for SeqIdGen {
    fn next(&mut self) -> crate::id::StableId {
        let id = crate::id::StableId(format!("{}-{:06}", self.prefix, self.next));
        self.next += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_hex_roundtrip() {
        let h = Hash([0xabu8; 32]);
        let hex = h.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(Hash::from_hex(&hex).unwrap(), h);
        // serde uses the same hex form
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<Hash>(&json).unwrap(), h);
    }

    #[test]
    fn seq_ids_are_deterministic() {
        let mut a = SeqIdGen::new("note");
        let mut b = SeqIdGen::new("note");
        assert_eq!(a.next(), b.next());
        assert_eq!(a.next(), b.next());
    }
}
