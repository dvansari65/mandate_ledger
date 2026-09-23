//! The payment lifecycle state machine and its typestate tokens.
//!
//! ```text
//!             ┌──────────┐
//!             │Authorized│──────────────────────► Expired
//!             └────┬─────┘
//!                  │ record_payment
//!             ┌────▼────┐
//!             │  Paid   │
//!             └────┬────┘
//!        ┌─────────┴──────────┐  record_settlement
//!   ┌────▼────┐       ┌───────▼─────────┐
//!   │ Settled │       │SettlementFailed │
//!   └────┬────┘       └───────┬─────────┘
//!        │ record_delivery    │ compensate
//!   ┌────▼──────┐     ┌───────▼─────┐
//!   │ Delivered │     │ Compensated │
//!   └───────────┘     └─────────────┘
//! ```
//!
//! Each token type (`Authorized`, `Paid`, …) is a *proof* that the ledger
//! recorded that state. Engine methods accept only the token for the state
//! they transition from, so `record_delivery` cannot be called with a `Paid`
//! — it does not compile. The store independently re-checks the transition
//! at runtime, so the guarantee holds even if a token is reconstructed.

use crate::hash::Hash32;
use crate::ids::{ContextId, MandateId, MerchantId};
use crate::money::Money;
use serde::{Deserialize, Serialize};

/// Where a payment context currently is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentState {
    /// Scope and budget checks passed; funds reserved against the mandate.
    Authorized,
    /// A rail proof was verified and bound to this context.
    Paid,
    /// The rail reported finality; delivery is now permitted.
    Settled,
    /// The rail reported that settlement will not happen.
    SettlementFailed,
    /// The host has undone side effects after a failed settlement.
    Compensated,
    /// The merchant recorded fulfilment.
    Delivered,
    /// The authorization lapsed before payment; reservation released.
    Expired,
}

impl PaymentState {
    /// Whether the machine permits `self → next`.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Authorized, Self::Paid | Self::Expired)
                | (Self::Paid, Self::Settled | Self::SettlementFailed)
                | (Self::SettlementFailed, Self::Compensated)
                | (Self::Settled, Self::Delivered)
        )
    }

    /// Whether no further transitions are possible.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Compensated | Self::Delivered | Self::Expired)
    }
}

/// Proof that a context is at least `Authorized`.
#[derive(Clone, Debug)]
pub struct Authorized {
    ctx: ContextId,
    mandate_id: MandateId,
    cart_hash: Hash32,
    merchant: MerchantId,
    amount: Money,
}

impl Authorized {
    pub(crate) const fn new(
        ctx: ContextId,
        mandate_id: MandateId,
        cart_hash: Hash32,
        merchant: MerchantId,
        amount: Money,
    ) -> Self {
        Self {
            ctx,
            mandate_id,
            cart_hash,
            merchant,
            amount,
        }
    }

    /// The payment context.
    #[must_use]
    pub const fn ctx(&self) -> &ContextId {
        &self.ctx
    }
    /// The mandate this authorization was granted under.
    #[must_use]
    pub const fn mandate_id(&self) -> &MandateId {
        &self.mandate_id
    }
    /// Hash of the cart that was authorized.
    #[must_use]
    pub const fn cart_hash(&self) -> &Hash32 {
        &self.cart_hash
    }
    /// The merchant that will be paid.
    #[must_use]
    pub const fn merchant(&self) -> &MerchantId {
        &self.merchant
    }
    /// The amount reserved against the mandate.
    #[must_use]
    pub const fn amount(&self) -> &Money {
        &self.amount
    }
}

/// Proof that a context is at least `Paid`.
#[derive(Clone, Debug)]
pub struct Paid {
    ctx: ContextId,
    rail: String,
    reference: String,
    idempotency_key: Hash32,
}

impl Paid {
    pub(crate) const fn new(
        ctx: ContextId,
        rail: String,
        reference: String,
        idempotency_key: Hash32,
    ) -> Self {
        Self {
            ctx,
            rail,
            reference,
            idempotency_key,
        }
    }

    /// The payment context.
    #[must_use]
    pub const fn ctx(&self) -> &ContextId {
        &self.ctx
    }
    /// Identifier of the rail that verified the proof.
    #[must_use]
    pub fn rail(&self) -> &str {
        &self.rail
    }
    /// The rail's own reference for this payment.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }
    /// `H(ctx ‖ rail ‖ nonce)` — the key under which this payment is unique.
    #[must_use]
    pub const fn idempotency_key(&self) -> &Hash32 {
        &self.idempotency_key
    }
}

/// Proof that a context is `Settled`. Only this token unlocks delivery.
#[derive(Clone, Debug)]
pub struct Settled {
    ctx: ContextId,
    reference: String,
}

impl Settled {
    pub(crate) const fn new(ctx: ContextId, reference: String) -> Self {
        Self { ctx, reference }
    }

    /// The payment context.
    #[must_use]
    pub const fn ctx(&self) -> &ContextId {
        &self.ctx
    }
    /// The rail's settlement reference (block, capture id, …).
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }
}

/// Proof that settlement failed. The reservation has already been released.
#[derive(Clone, Debug)]
pub struct SettlementFailed {
    ctx: ContextId,
    reason: String,
}

impl SettlementFailed {
    pub(crate) const fn new(ctx: ContextId, reason: String) -> Self {
        Self { ctx, reason }
    }

    /// The payment context.
    #[must_use]
    pub const fn ctx(&self) -> &ContextId {
        &self.ctx
    }
    /// Why the rail reported failure.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Proof that the host compensated a failed settlement.
#[derive(Clone, Debug)]
pub struct Compensated {
    ctx: ContextId,
}

impl Compensated {
    pub(crate) const fn new(ctx: ContextId) -> Self {
        Self { ctx }
    }

    /// The payment context.
    #[must_use]
    pub const fn ctx(&self) -> &ContextId {
        &self.ctx
    }
}

/// Proof that delivery was recorded.
#[derive(Clone, Debug)]
pub struct Delivered {
    ctx: ContextId,
    receipt_reference: String,
}

impl Delivered {
    pub(crate) const fn new(ctx: ContextId, receipt_reference: String) -> Self {
        Self {
            ctx,
            receipt_reference,
        }
    }

    /// The payment context.
    #[must_use]
    pub const fn ctx(&self) -> &ContextId {
        &self.ctx
    }
    /// The merchant's fulfilment reference.
    #[must_use]
    pub fn receipt_reference(&self) -> &str {
        &self.receipt_reference
    }
}

/// Outcome of `record_settlement`.
#[derive(Clone, Debug)]
pub enum Settlement {
    /// Finality reached. Deliver.
    Settled(Settled),
    /// Not final yet. No state change; call again with fresher evidence.
    Pending {
        /// What the rail is still waiting for.
        reason: String,
    },
    /// The rail reported failure. Reservation released; compensate.
    Failed(SettlementFailed),
}

/// A context's current position, reconstructed from the store.
///
/// Returned by `Ledger::resume` so a host can pick up a lifecycle across
/// process boundaries — e.g. `authorize` in an HTTP handler, then
/// `record_payment` in a webhook handler minutes later.
#[derive(Clone, Debug)]
pub enum Resumed {
    /// See [`Authorized`].
    Authorized(Authorized),
    /// See [`Paid`].
    Paid(Paid),
    /// See [`Settled`].
    Settled(Settled),
    /// See [`SettlementFailed`].
    SettlementFailed(SettlementFailed),
    /// See [`Compensated`].
    Compensated(Compensated),
    /// See [`Delivered`].
    Delivered(Delivered),
    /// The authorization lapsed.
    Expired {
        /// The payment context.
        ctx: ContextId,
    },
}

/// Every token a context has earned so far: proof of each stage it has
/// passed through, whatever state it is in now.
///
/// A delivered context has earned `Authorized`, `Paid`, `Settled` and
/// `Delivered`; a compensated one `Authorized`, `Paid`, `SettlementFailed`
/// and `Compensated`; an expired one only `Authorized`. Returned by
/// `Ledger::reached`, so a caller holding nothing but a context id — a CLI,
/// a script, a webhook handler that lost its token — can replay any step it
/// has a token for and get the engine's own decision, including a recorded
/// refusal when the step no longer applies.
#[derive(Clone, Debug)]
pub struct Reached {
    /// Where the context is now.
    pub state: PaymentState,
    /// Always earned: a context exists only once it was authorized.
    pub authorized: Authorized,
    /// Earned by every context that was paid, whatever happened after.
    pub paid: Option<Paid>,
    /// Earned by settled and delivered contexts.
    pub settled: Option<Settled>,
    /// Earned by contexts whose settlement failed, compensated or not.
    pub settlement_failed: Option<SettlementFailed>,
    /// Earned once the host compensated.
    pub compensated: Option<Compensated>,
    /// Earned once the merchant delivered.
    pub delivered: Option<Delivered>,
    /// The authorization lapsed; nothing beyond `authorized` was earned.
    pub expired: bool,
}

impl Resumed {
    /// The state this token represents.
    #[must_use]
    pub const fn state(&self) -> PaymentState {
        match self {
            Self::Authorized(_) => PaymentState::Authorized,
            Self::Paid(_) => PaymentState::Paid,
            Self::Settled(_) => PaymentState::Settled,
            Self::SettlementFailed(_) => PaymentState::SettlementFailed,
            Self::Compensated(_) => PaymentState::Compensated,
            Self::Delivered(_) => PaymentState::Delivered,
            Self::Expired { .. } => PaymentState::Expired,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PaymentState as S;

    #[test]
    fn only_documented_transitions_are_legal() {
        let all = [
            S::Authorized,
            S::Paid,
            S::Settled,
            S::SettlementFailed,
            S::Compensated,
            S::Delivered,
            S::Expired,
        ];
        let legal = [
            (S::Authorized, S::Paid),
            (S::Authorized, S::Expired),
            (S::Paid, S::Settled),
            (S::Paid, S::SettlementFailed),
            (S::SettlementFailed, S::Compensated),
            (S::Settled, S::Delivered),
        ];
        for a in all {
            for b in all {
                assert_eq!(
                    a.can_transition_to(b),
                    legal.contains(&(a, b)),
                    "{a:?}->{b:?}"
                );
            }
        }
    }
}
