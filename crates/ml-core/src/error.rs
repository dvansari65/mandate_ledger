//! Error and denial types.
//!
//! A [`Denied`] is a *decision* — the ledger looked at the request and said
//! no, with a machine-readable [`DenyReason`]. A [`StoreError`] or rail
//! outage is an *operational failure* — the ledger could not decide. Hosts
//! should surface the first to the caller and retry the second.

use crate::event::Stage;
use crate::hash::CanonicalizeError;
use crate::ids::ContextId;
use crate::money::MoneyError;
use crate::state::PaymentState;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

macro_rules! deny_reasons {
    ($( $(#[$meta:meta])* $variant:ident => $code:literal ),* $(,)?) => {
        /// Why a step was refused. Serializes as its `SCREAMING_SNAKE_CASE` code.
        #[non_exhaustive]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum DenyReason {
            $( $(#[$meta])* $variant, )*
        }

        impl DenyReason {
            /// Stable machine-readable code.
            #[must_use]
            pub const fn code(self) -> &'static str {
                match self { $( Self::$variant => $code, )* }
            }

            /// Parse a code produced by [`DenyReason::code`].
            #[must_use]
            pub fn from_code(code: &str) -> Option<Self> {
                match code { $( $code => Some(Self::$variant), )* _ => None }
            }
        }
    };
}

deny_reasons! {
    /// The mandate's Ed25519 signature did not verify.
    MandateSignatureInvalid => "MANDATE_SIGNATURE_INVALID",
    /// The signing key is not trusted for the mandate's principal.
    MandateSignerUntrusted => "MANDATE_SIGNER_UNTRUSTED",
    /// `now < scope.valid_from`.
    MandateNotYetValid => "MANDATE_NOT_YET_VALID",
    /// `now > scope.valid_until`.
    MandateExpired => "MANDATE_EXPIRED",
    /// The mandate is on the revocation list.
    MandateRevoked => "MANDATE_REVOKED",
    /// The cart's attestation is weaker than the mandate requires.
    AttestationInsufficient => "ATTESTATION_INSUFFICIENT",
    /// The merchant is not in the mandate's merchant patterns.
    ScopeMerchantMismatch => "SCOPE_MERCHANT_MISMATCH",
    /// The category is not in the mandate's category list.
    ScopeCategoryMismatch => "SCOPE_CATEGORY_MISMATCH",
    /// The mandate constrains a claim the cart does not carry (fail closed).
    UnverifiableScope => "UNVERIFIABLE_SCOPE",
    /// The cart currency differs from the mandate currency.
    CurrencyMismatch => "CURRENCY_MISMATCH",
    /// The amount is negative or otherwise not chargeable.
    AmountInvalid => "AMOUNT_INVALID",
    /// The cart total exceeds `scope.max_per_txn`.
    ScopePerTxnExceeded => "SCOPE_PER_TXN_EXCEEDED",
    /// Reserving this amount would exceed `scope.max_total`.
    ScopeTotalExceeded => "SCOPE_TOTAL_EXCEEDED",
    /// Too many authorizations inside `scope.velocity.window_secs`.
    VelocityExceeded => "VELOCITY_EXCEEDED",
    /// The context is in a state that does not permit this step.
    InvalidState => "INVALID_STATE",
    /// No context with this id exists.
    ContextNotFound => "CONTEXT_NOT_FOUND",
    /// The rail rejected the payment proof.
    ProofInvalid => "PROOF_INVALID",
    /// The proof's amount differs from the authorized amount.
    AmountMismatch => "AMOUNT_MISMATCH",
    /// The proof binds to neither the context nor the cart (property P1).
    UnboundProof => "UNBOUND_PROOF",
    /// The proof binds to a different cart hash.
    CartBindingMismatch => "CART_BINDING_MISMATCH",
    /// The proof binds to a different context id.
    ContextBindingMismatch => "CONTEXT_BINDING_MISMATCH",
    /// The proof's nonce was already consumed by another context (property P3).
    NonceAlreadyUsed => "NONCE_ALREADY_USED",
    /// The rail rejected the finality evidence.
    FinalityInvalid => "FINALITY_INVALID",
    /// Evidence came from a different rail than the one that recorded payment.
    RailMismatch => "RAIL_MISMATCH",
}

impl fmt::Display for DenyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl Serialize for DenyReason {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for DenyReason {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_code(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown deny reason `{s}`")))
    }
}

/// A refused step. Always recorded in the ledger before being returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{stage} denied: {reason} — {detail}")]
pub struct Denied {
    /// The context the denial was recorded under.
    pub ctx: ContextId,
    /// Which step refused.
    pub stage: Stage,
    /// Machine-readable reason.
    pub reason: DenyReason,
    /// Human-readable detail. Never contains secrets.
    pub detail: String,
}

/// Any error from a ledger operation.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// The step was evaluated and refused.
    #[error(transparent)]
    Denied(#[from] Denied),
    /// The store failed; the step was not evaluated. Retry.
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    /// The rail could not be reached; the step was not evaluated. Retry.
    #[error("rail unavailable: {0}")]
    RailUnavailable(String),
    /// A value could not be canonicalized for hashing.
    #[error(transparent)]
    Canonicalize(#[from] CanonicalizeError),
}

impl LedgerError {
    /// The denial, if this error is one.
    #[must_use]
    pub const fn denied(&self) -> Option<&Denied> {
        match self {
            Self::Denied(d) => Some(d),
            _ => None,
        }
    }

    /// The deny reason, if this error is a denial.
    #[must_use]
    pub const fn reason(&self) -> Option<DenyReason> {
        match self {
            Self::Denied(d) => Some(d.reason),
            _ => None,
        }
    }
}

/// Errors from a [`crate::store::Store`] implementation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The store refused a state transition the machine does not allow.
    /// This is the last line of defence; the engine should never trigger it.
    #[error("illegal transition {from:?} -> {to:?} on {ctx}")]
    IllegalTransition {
        /// Context the transition was attempted on.
        ctx: ContextId,
        /// Current state, or `None` if the context does not exist.
        from: Option<PaymentState>,
        /// Requested state.
        to: PaymentState,
    },
    /// Stored data is inconsistent with what the engine expects.
    #[error("store corrupt: {0}")]
    Corrupt(String),
    /// The backing storage failed (I/O, lock poisoning, connection loss).
    #[error("store backend: {0}")]
    Backend(String),
    /// A value could not be canonicalized for hashing.
    #[error(transparent)]
    Canonicalize(#[from] CanonicalizeError),
    /// Money arithmetic failed inside the store.
    #[error(transparent)]
    Money(#[from] MoneyError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_reason_code_roundtrip() {
        let r = DenyReason::NonceAlreadyUsed;
        assert_eq!(DenyReason::from_code(r.code()), Some(r));
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#""NONCE_ALREADY_USED""#);
        assert_eq!(serde_json::from_str::<DenyReason>(&json).unwrap(), r);
    }
}
