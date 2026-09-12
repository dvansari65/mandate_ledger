//! Append-only, hash-chained ledger events.
//!
//! Every decision the engine makes — including denials — becomes an
//! [`Event`]. Events for one context form a chain: each carries the hash of
//! its predecessor, so an evidence bundle can be verified standalone.

use crate::cart::{Attestation, Cart, ScopeClaims};
use crate::error::DenyReason;
use crate::hash::{CanonicalizeError, Hash32, canonical_json};
use crate::ids::ContextId;
use crate::mandate::Mandate;
use crate::money::Money;
use crate::state::PaymentState;
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Which engine step an event or denial belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// `authorize`
    Authorize,
    /// `record_payment`
    Payment,
    /// `record_settlement`
    Settlement,
    /// `compensate`
    Compensation,
    /// `record_delivery`
    Delivery,
    /// `expire`
    Expiry,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Authorize => "authorize",
            Self::Payment => "payment",
            Self::Settlement => "settlement",
            Self::Compensation => "compensation",
            Self::Delivery => "delivery",
            Self::Expiry => "expiry",
        };
        f.write_str(s)
    }
}

/// The cart exactly as it was authorized, preserved for evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartSnapshot {
    /// `SHA-256(raw)`.
    pub hash: Hash32,
    /// Claims the adapter extracted.
    pub claims: ScopeClaims,
    /// Who vouched for the cart.
    pub attestation: Attestation,
    /// Original canonical bytes, base64.
    #[serde(with = "b64")]
    pub raw: Vec<u8>,
}

impl From<&Cart> for CartSnapshot {
    fn from(cart: &Cart) -> Self {
        Self {
            hash: *cart.hash(),
            claims: cart.claims().clone(),
            attestation: cart.attestation().clone(),
            raw: cart.raw().to_vec(),
        }
    }
}

/// What the merchant reports when it fulfils.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    /// Merchant's fulfilment reference (order id, tracking id, response hash…).
    pub reference: String,
    /// Who vouched for the receipt.
    pub attestation: Attestation,
}

/// The payload of an [`Event`].
///
/// `Authorized` is much larger than the other variants because it carries
/// the full mandate and cart snapshot for evidence. That is deliberate:
/// events are written once and read for audits, not held in hot memory.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventBody {
    /// Scope and budget checks passed. Carries full snapshots for evidence.
    Authorized {
        /// The signed mandate as presented.
        mandate: Mandate,
        /// The cart as normalized.
        cart: CartSnapshot,
        /// Caller-supplied idempotency key for this authorization.
        request_key: String,
    },
    /// A step was refused. Does not change state.
    Denied {
        /// Which step.
        stage: Stage,
        /// Why.
        reason: DenyReason,
        /// Detail.
        detail: String,
    },
    /// A rail proof was verified and bound.
    Paid {
        /// Rail identifier.
        rail: String,
        /// Rail's payment reference.
        reference: String,
        /// Single-use nonce consumed by this payment.
        nonce: String,
        /// Verified amount.
        amount: Money,
        /// `H(ctx ‖ rail ‖ nonce)`.
        idempotency_key: Hash32,
    },
    /// The rail reported finality.
    Settled {
        /// Rail identifier.
        rail: String,
        /// Settlement reference.
        reference: String,
    },
    /// The rail reported settlement failure. Reservation released.
    SettlementFailed {
        /// Rail identifier.
        rail: String,
        /// Reason.
        reason: String,
    },
    /// The host compensated.
    Compensated {
        /// Optional reference to the compensating action (refund id…).
        reference: Option<String>,
    },
    /// The merchant fulfilled.
    Delivered {
        /// The receipt.
        receipt: DeliveryReceipt,
    },
    /// The authorization lapsed. Reservation released.
    Expired,
}

impl EventBody {
    /// The state this event moves the context to, or `None` for
    /// informational events like `Denied`.
    #[must_use]
    pub const fn resulting_state(&self) -> Option<PaymentState> {
        match self {
            Self::Authorized { .. } => Some(PaymentState::Authorized),
            Self::Paid { .. } => Some(PaymentState::Paid),
            Self::Settled { .. } => Some(PaymentState::Settled),
            Self::SettlementFailed { .. } => Some(PaymentState::SettlementFailed),
            Self::Compensated { .. } => Some(PaymentState::Compensated),
            Self::Delivered { .. } => Some(PaymentState::Delivered),
            Self::Expired => Some(PaymentState::Expired),
            Self::Denied { .. } => None,
        }
    }
}

/// One entry in a context's hash chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Store-wide sequence number, strictly increasing.
    pub seq: u64,
    /// The context this event belongs to.
    pub ctx: ContextId,
    /// When the engine recorded it.
    pub at: Timestamp,
    /// Hash of the previous event in this context, or [`Hash32::ZERO`].
    pub prev_hash: Hash32,
    /// `SHA-256(prev_hash ‖ canonical(seq, ctx, at, body))`.
    pub hash: Hash32,
    /// The payload.
    pub body: EventBody,
}

#[derive(Serialize)]
struct Preimage<'a> {
    seq: u64,
    ctx: &'a ContextId,
    at: Timestamp,
    body: &'a EventBody,
}

impl Event {
    /// Build an event, computing its hash.
    pub fn new(
        prev_hash: Hash32,
        seq: u64,
        ctx: ContextId,
        at: Timestamp,
        body: EventBody,
    ) -> Result<Self, CanonicalizeError> {
        let hash = Self::compute_hash(&prev_hash, seq, &ctx, at, &body)?;
        Ok(Self {
            seq,
            ctx,
            at,
            prev_hash,
            hash,
            body,
        })
    }

    /// The hash an event with these fields must carry.
    pub fn compute_hash(
        prev_hash: &Hash32,
        seq: u64,
        ctx: &ContextId,
        at: Timestamp,
        body: &EventBody,
    ) -> Result<Hash32, CanonicalizeError> {
        let bytes = canonical_json(&Preimage { seq, ctx, at, body })?;
        Ok(Hash32::chain(prev_hash, &bytes))
    }

    /// Recompute this event's hash from its fields and compare.
    pub fn verify_hash(&self) -> Result<bool, CanonicalizeError> {
        let expected =
            Self::compute_hash(&self.prev_hash, self.seq, &self.ctx, self.at, &self.body)?;
        Ok(expected == self.hash)
    }
}

mod b64 {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}
