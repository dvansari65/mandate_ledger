//! A rail that reports whatever you tell it. **Tests and examples only.**
//!
//! The mock deliberately does *not* check amounts or bindings — it returns
//! what the proof claims, so tests can confirm the engine catches mismatches.

use ml_core::{
    ContextId, FinalityStatus, Hash32, Money, PaymentExpectation, Rail, RailError,
    SettlementExpectation, VerifiedProof,
};

/// A fake payment proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockProof {
    /// Rail reference for the payment.
    pub reference: String,
    /// Single-use nonce.
    pub nonce: String,
    /// Amount the proof is for.
    pub amount: Money,
    /// Context the proof claims to bind to.
    pub bound_ctx: Option<ContextId>,
    /// Cart hash the proof claims to bind to.
    pub bound_cart: Option<Hash32>,
    /// Whether the "signature" verifies.
    pub valid: bool,
}

impl MockProof {
    /// A valid proof bound to `ctx`, using `reference` as both reference and nonce.
    #[must_use]
    pub fn bound_to(ctx: &ContextId, reference: impl Into<String>, amount: Money) -> Self {
        let reference = reference.into();
        Self {
            nonce: reference.clone(),
            reference,
            amount,
            bound_ctx: Some(ctx.clone()),
            bound_cart: None,
            valid: true,
        }
    }
}

/// Fake finality evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockFinality {
    /// The payment this evidence is about.
    pub reference: String,
    /// Confirmations observed.
    pub confirmations: u32,
    /// If set, the payment failed for this reason.
    pub failure: Option<String>,
}

impl MockFinality {
    /// Evidence of `confirmations` confirmations for `reference`.
    #[must_use]
    pub fn confirmed(reference: impl Into<String>, confirmations: u32) -> Self {
        Self {
            reference: reference.into(),
            confirmations,
            failure: None,
        }
    }

    /// Evidence that `reference` failed.
    #[must_use]
    pub fn failed(reference: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            confirmations: 0,
            failure: Some(reason.into()),
        }
    }
}

/// The mock rail.
#[derive(Clone, Debug)]
pub struct MockRail {
    id: String,
    min_confirmations: u32,
}

impl MockRail {
    /// A rail named `id` that considers a payment final at `min_confirmations`.
    #[must_use]
    pub fn new(id: impl Into<String>, min_confirmations: u32) -> Self {
        Self {
            id: id.into(),
            min_confirmations,
        }
    }
}

impl Rail for MockRail {
    type Proof = MockProof;
    type Finality = MockFinality;

    fn id(&self) -> &str {
        &self.id
    }

    fn verify_proof(
        &self,
        proof: &MockProof,
        _expected: &PaymentExpectation<'_>,
    ) -> Result<VerifiedProof, RailError> {
        if !proof.valid {
            return Err(RailError::InvalidProof(
                "mock signature check failed".to_owned(),
            ));
        }
        Ok(VerifiedProof {
            reference: proof.reference.clone(),
            nonce: proof.nonce.clone(),
            amount: proof.amount.clone(),
            bound_ctx: proof.bound_ctx.clone(),
            bound_cart: proof.bound_cart,
        })
    }

    fn check_finality(
        &self,
        finality: &MockFinality,
        expected: &SettlementExpectation<'_>,
    ) -> Result<FinalityStatus, RailError> {
        if finality.reference != expected.payment_reference {
            return Err(RailError::InvalidProof(format!(
                "evidence is for `{}`, payment is `{}`",
                finality.reference, expected.payment_reference
            )));
        }
        if let Some(reason) = &finality.failure {
            return Ok(FinalityStatus::Failed {
                reason: reason.clone(),
            });
        }
        if finality.confirmations >= self.min_confirmations {
            Ok(FinalityStatus::Final {
                reference: format!("{}@{}", finality.reference, finality.confirmations),
            })
        } else {
            Ok(FinalityStatus::Pending {
                reason: format!(
                    "need {} confirmations, have {}",
                    self.min_confirmations, finality.confirmations
                ),
            })
        }
    }
}
