//! The cart as the engine sees it.
//!
//! The engine never interprets line items. A protocol adapter turns whatever
//! the wire format is (AP2 `CartMandate`, ACP checkout session, x402 terms,
//! a merchant's own JSON) into a [`Cart`]: a **hash** of the canonical bytes
//! plus the handful of **claims** a mandate can be scoped on. Items are
//! opaque — they are bound by hash, never parsed by the engine.

use crate::hash::Hash32;
use crate::ids::{Category, IdError, MerchantId};
use crate::money::{Money, MoneyError};
use serde::{Deserialize, Serialize};

/// The claims a mandate's [`crate::mandate::Scope`] can constrain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeClaims {
    /// Who gets paid.
    pub merchant: MerchantId,
    /// How much.
    pub total: Money,
    /// What kind of purchase, if the source protocol can say.
    pub category: Option<Category>,
    /// How many line items, if the source protocol has line items.
    pub line_count: Option<u32>,
}

/// How strongly the cart's contents are vouched for, weakest to strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationLevel {
    /// The agent asserted the cart. No independent proof.
    AgentReported = 0,
    /// The merchant signed the cart.
    MerchantSigned = 1,
}

/// Who vouched for the cart, with the key that did so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attestation {
    /// The merchant signed the canonical cart bytes.
    MerchantSigned {
        /// Which merchant key. The adapter has already verified the signature.
        key_id: String,
    },
    /// No signature; the agent (or host) asserted the contents.
    AgentReported,
}

impl Attestation {
    /// The strength of this attestation.
    #[must_use]
    pub const fn level(&self) -> AttestationLevel {
        match self {
            Self::MerchantSigned { .. } => AttestationLevel::MerchantSigned,
            Self::AgentReported => AttestationLevel::AgentReported,
        }
    }
}

/// A normalized cart: canonical bytes, their hash, and extracted claims.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cart {
    hash: Hash32,
    claims: ScopeClaims,
    attestation: Attestation,
    raw: Vec<u8>,
}

impl Cart {
    /// Build a cart from canonical bytes. The hash is computed here, once.
    ///
    /// `raw` **must** already be canonical (e.g. RFC 8785 JSON) — the adapter
    /// is responsible for that. If two parties canonicalize differently the
    /// hashes will not match and binding will fail closed.
    #[must_use]
    pub fn new(raw: impl Into<Vec<u8>>, claims: ScopeClaims, attestation: Attestation) -> Self {
        let raw = raw.into();
        Self {
            hash: Hash32::of(&raw),
            claims,
            attestation,
            raw,
        }
    }

    /// `SHA-256(raw)`. This is what every later stage binds to.
    #[must_use]
    pub const fn hash(&self) -> &Hash32 {
        &self.hash
    }
    /// Claims for scope checking.
    #[must_use]
    pub const fn claims(&self) -> &ScopeClaims {
        &self.claims
    }
    /// Who vouched.
    #[must_use]
    pub const fn attestation(&self) -> &Attestation {
        &self.attestation
    }
    /// The canonical bytes.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

/// Turns a protocol-specific wire payload into a [`Cart`].
///
/// Implementations must: canonicalize, verify any merchant signature, and
/// only populate a claim (e.g. `category`) if the protocol actually carries
/// it. Never guess a claim — a missing claim fails closed at `authorize`.
pub trait CartAdapter: Send + Sync {
    /// Normalize `raw` bytes from the wire.
    fn normalize(&self, raw: &[u8]) -> Result<Cart, AdapterError>;
}

/// Errors an adapter can produce.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    /// The payload did not parse as the expected format.
    #[error("malformed payload: {0}")]
    Malformed(String),
    /// A signature was present but did not verify, or the key is unknown.
    #[error("signature invalid: {0}")]
    SignatureInvalid(String),
    /// The payload uses a variant this adapter does not support.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// An identifier in the payload was invalid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// A money value in the payload was invalid.
    #[error(transparent)]
    Money(#[from] MoneyError),
    /// Canonicalization failed.
    #[error(transparent)]
    Canonicalize(#[from] crate::hash::CanonicalizeError),
}
