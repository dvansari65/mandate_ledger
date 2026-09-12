//! The rail boundary: how the engine talks to whatever moves money.
//!
//! A rail (Stripe, Razorpay, UPI, USDC-on-Base, x402 facilitator…) is
//! anything that can (1) verify a payment proof and (2) say whether a
//! payment is final. The engine supplies what it *expects*; the rail
//! reports what it *sees*; the engine compares. Rails never decide.

use crate::hash::Hash32;
use crate::ids::{ContextId, MerchantId};
use crate::money::Money;

/// What the engine authorized, handed to the rail for proof verification.
#[derive(Clone, Copy, Debug)]
pub struct PaymentExpectation<'a> {
    /// The context being paid.
    pub ctx: &'a ContextId,
    /// The cart hash that was authorized.
    pub cart_hash: &'a Hash32,
    /// The authorized amount.
    pub amount: &'a Money,
    /// The authorized merchant.
    pub merchant: &'a MerchantId,
}

/// What a rail extracted from a valid proof.
///
/// At least one of `bound_ctx` / `bound_cart` must be `Some`, or the engine
/// denies `UNBOUND_PROOF`: a proof that cannot be tied to what was
/// authorized is not evidence of paying for *this* cart (property P1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedProof {
    /// The rail's reference for the payment (payment id, tx hash…).
    pub reference: String,
    /// A single-use value the rail guarantees is unique per payment
    /// (on-chain nonce, PSP payment id…). Consumed by the store.
    pub nonce: String,
    /// The amount the proof is for.
    pub amount: Money,
    /// The context id the proof carries, if the rail can embed one
    /// (PSP metadata, order receipt field…).
    pub bound_ctx: Option<ContextId>,
    /// The cart hash the proof carries, if the protocol binds to it
    /// (AP2 payment mandate, x402 resource hash…).
    pub bound_cart: Option<Hash32>,
}

/// What the engine recorded at payment, handed to the rail for finality.
#[derive(Clone, Copy, Debug)]
pub struct SettlementExpectation<'a> {
    /// The context awaiting settlement.
    pub ctx: &'a ContextId,
    /// The payment reference recorded at `record_payment`.
    pub payment_reference: &'a str,
}

/// Whether a payment has reached finality.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinalityStatus {
    /// Irreversible per this rail's policy. Delivery may proceed.
    Final {
        /// Settlement reference (block number, capture id…).
        reference: String,
    },
    /// Not yet. Ask again later with fresher evidence.
    Pending {
        /// What is still outstanding.
        reason: String,
    },
    /// It will not settle (reverted, declined, expired).
    Failed {
        /// Why.
        reason: String,
    },
}

/// Errors a rail can report.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RailError {
    /// The proof or evidence is malformed or does not verify. A denial.
    #[error("{0}")]
    InvalidProof(String),
    /// The rail could not be consulted right now. Not a denial; retry.
    #[error("{0}")]
    Unavailable(String),
}

/// A money-moving system the engine can gate.
pub trait Rail: Send + Sync {
    /// The proof-of-payment type this rail understands.
    type Proof;
    /// The finality evidence type this rail understands.
    type Finality;

    /// Stable identifier, recorded in every event (e.g. `"razorpay"`, `"x402:base"`).
    fn id(&self) -> &str;

    /// Verify a proof cryptographically / against the rail, and extract what it claims.
    ///
    /// Do **not** compare amounts or bindings here — return what the proof
    /// says and let the engine compare against `expected`. Rails that
    /// second-guess the engine end up disagreeing with it.
    fn verify_proof(
        &self,
        proof: &Self::Proof,
        expected: &PaymentExpectation<'_>,
    ) -> Result<VerifiedProof, RailError>;

    /// Decide whether `finality` shows the payment in `expected` is final.
    fn check_finality(
        &self,
        finality: &Self::Finality,
        expected: &SettlementExpectation<'_>,
    ) -> Result<FinalityStatus, RailError>;
}

impl<R: Rail + ?Sized> Rail for std::sync::Arc<R> {
    type Proof = R::Proof;
    type Finality = R::Finality;
    fn id(&self) -> &str {
        (**self).id()
    }
    fn verify_proof(
        &self,
        proof: &Self::Proof,
        expected: &PaymentExpectation<'_>,
    ) -> Result<VerifiedProof, RailError> {
        (**self).verify_proof(proof, expected)
    }
    fn check_finality(
        &self,
        finality: &Self::Finality,
        expected: &SettlementExpectation<'_>,
    ) -> Result<FinalityStatus, RailError> {
        (**self).check_finality(finality, expected)
    }
}
