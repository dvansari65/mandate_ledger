//! Opaque string identifiers.
//!
//! Each identifier is a distinct newtype so a [`MandateId`] can never be
//! passed where an [`AgentId`] is expected. Identifiers are 1–256 bytes.

use crate::hash::Hash32;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Error constructing an identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("identifier must be 1..=256 bytes, got {0}")]
pub struct IdError(pub usize);

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident, normalize = $norm:expr) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Validate and construct.
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let norm: fn(String) -> String = $norm;
                let s = norm(value.into());
                if s.is_empty() || s.len() > 256 {
                    return Err(IdError(s.len()));
                }
                Ok(Self(s))
            }

            /// The identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::new(s)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.0)
            }
        }
    };
}

fn identity(s: String) -> String {
    s
}

fn lowercase_trimmed(mut s: String) -> String {
    s.make_ascii_lowercase();
    let end = s.trim_end().len();
    s.truncate(end);
    let start = s.len() - s.trim_start().len();
    s.drain(..start);
    s
}

string_id!(
    /// Identifies a signed mandate.
    MandateId,
    normalize = identity
);
string_id!(
    /// The human or organisation that granted a mandate.
    PrincipalId,
    normalize = identity
);
string_id!(
    /// The agent a mandate was granted to.
    AgentId,
    normalize = identity
);
string_id!(
    /// A merchant, normalized to lower-case (typically a domain or DID).
    MerchantId,
    normalize = lowercase_trimmed
);
string_id!(
    /// A purchase category, normalized to lower-case.
    Category,
    normalize = lowercase_trimmed
);
string_id!(
    /// Identifies one payment lifecycle. Derived deterministically — see [`ContextId::derive`].
    ContextId,
    normalize = identity
);

impl ContextId {
    /// Derive the context id for `(mandate, cart, request_key)`.
    ///
    /// The same inputs always yield the same id, which is what makes
    /// `authorize` idempotent: a retried request lands on the same context
    /// instead of creating a second one.
    #[must_use]
    pub fn derive(mandate: &MandateId, cart: &Hash32, request_key: &str) -> Self {
        let mut buf = Vec::with_capacity(mandate.0.len() + 32 + request_key.len() + 2);
        buf.extend_from_slice(mandate.0.as_bytes());
        buf.push(0x1f);
        buf.extend_from_slice(cart.as_bytes());
        buf.push(0x1f);
        buf.extend_from_slice(request_key.as_bytes());
        let digest = Hash32::of(&buf);
        Self(format!("ctx_{}", hex::encode(&digest.as_bytes()[..16])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merchant_is_normalized() {
        let m = MerchantId::new("  BigBasket.COM ").unwrap();
        assert_eq!(m.as_str(), "bigbasket.com");
    }

    #[test]
    fn empty_is_rejected() {
        assert!(AgentId::new("").is_err());
        assert!(MerchantId::new("   ").is_err());
    }

    #[test]
    fn context_id_is_deterministic() {
        let m = MandateId::new("m1").unwrap();
        let h = Hash32::of(b"cart");
        assert_eq!(
            ContextId::derive(&m, &h, "r1"),
            ContextId::derive(&m, &h, "r1")
        );
        assert_ne!(
            ContextId::derive(&m, &h, "r1"),
            ContextId::derive(&m, &h, "r2")
        );
    }
}
