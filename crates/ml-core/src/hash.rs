//! Content hashing and JSON canonicalization.
//!
//! Every binding in the ledger — cart ↔ payment, event ↔ previous event,
//! mandate ↔ signature — is a SHA-256 over RFC 8785 canonical JSON. Two
//! parties serializing the same value independently get the same hash,
//! regardless of key order or whitespace.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;

/// A SHA-256 digest. Displays and serializes as `sha256:<64 hex chars>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash32([u8; 32]);

impl Hash32 {
    /// The all-zero digest, used as the chain anchor before a context's first event.
    pub const ZERO: Hash32 = Hash32([0u8; 32]);

    /// Hash arbitrary bytes.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Hash `bytes` chained onto `prev`: `SHA-256(prev ‖ bytes)`.
    #[must_use]
    pub fn chain(prev: &Hash32, bytes: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(prev.0);
        h.update(bytes);
        Self(h.finalize().into())
    }

    /// Raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lower-case hex, no prefix.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

impl fmt::Debug for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Error parsing a [`Hash32`] from its string form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid hash: expected `sha256:<64 hex chars>`")]
pub struct HashParseError;

impl FromStr for Hash32 {
    type Err = HashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex_part = s.strip_prefix("sha256:").unwrap_or(s);
        let bytes = hex::decode(hex_part).map_err(|_| HashParseError)?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| HashParseError)?;
        Ok(Self(arr))
    }
}

impl Serialize for Hash32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Hash32 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Error produced when a value cannot be canonicalized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("canonicalization failed: {0}")]
pub struct CanonicalizeError(pub String);

/// Serialize `value` as RFC 8785 (JCS) canonical JSON bytes.
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalizeError> {
    serde_jcs::to_vec(value).map_err(|e| CanonicalizeError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_roundtrip() {
        let h = Hash32::of(b"hello");
        let s = h.to_string();
        assert!(s.starts_with("sha256:"));
        assert_eq!(s.parse::<Hash32>().unwrap(), h);
    }

    #[test]
    fn canonical_json_is_key_order_independent() {
        let a: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":"x"}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{ "a" : "x" , "b" : 1 }"#).unwrap();
        assert_eq!(canonical_json(&a).unwrap(), canonical_json(&b).unwrap());
    }

    #[test]
    fn chain_depends_on_prev() {
        let x = Hash32::chain(&Hash32::ZERO, b"e");
        let y = Hash32::chain(&Hash32::of(b"p"), b"e");
        assert_ne!(x, y);
    }
}
