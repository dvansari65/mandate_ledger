//! The engine. Five decisions, one evidence query.
//!
//! ```text
//! authorize ─► record_payment ─► record_settlement ─► record_delivery
//!                                      │
//!                                      └─► compensate      expire ◄─ authorize
//! ```
//!
//! Every method either returns a typestate token proving the new state, or
//! a [`LedgerError`]. Denials are recorded in the ledger *before* being
//! returned, so the audit trail shows what was refused and why.

use crate::cart::Cart;
use crate::error::{Denied, DenyReason, LedgerError, StoreError};
use crate::event::{CartSnapshot, DeliveryReceipt, EventBody, Stage};
use crate::evidence::EvidenceBundle;
use crate::hash::Hash32;
use crate::ids::{ContextId, MandateId};
use crate::mandate::{Mandate, SignerPolicy};
use crate::rail::{
    FinalityStatus, PaymentExpectation, Rail, RailError, SettlementExpectation, VerifiedProof,
};
use crate::state::{
    Authorized, Compensated, Delivered, Paid, PaymentState, Resumed, Settled, Settlement,
    SettlementFailed,
};
use crate::store::{AppendOutcome, Record, Store};
use crate::time::{Clock, SystemClock, Timestamp};

/// The payment lifecycle engine.
///
/// Generic over its four dependencies so hosts can swap any of them:
/// - `S`: where records and events live ([`crate::store::MemoryStore`], SQL…)
/// - `R`: the rail that verifies proofs and reports finality
/// - `P`: which keys may sign mandates for which principals
/// - `C`: the clock (defaults to wall time)
pub struct Ledger<S, R, P, C = SystemClock> {
    store: S,
    rail: R,
    signers: P,
    clock: C,
}

impl<S, R, P, C> Ledger<S, R, P, C>
where
    S: Store,
    R: Rail,
    P: SignerPolicy,
    C: Clock,
{
    /// Assemble an engine.
    pub const fn new(store: S, rail: R, signers: P, clock: C) -> Self {
        Self {
            store,
            rail,
            signers,
            clock,
        }
    }

    /// The store.
    pub const fn store(&self) -> &S {
        &self.store
    }

    /// The rail.
    pub const fn rail(&self) -> &R {
        &self.rail
    }

    // ───────────────────────────── authorize ─────────────────────────────

    /// Check `cart` against `mandate` and reserve its total.
    ///
    /// `request_key` is the caller's idempotency key for *this purchase
    /// attempt*. Retrying with the same `(mandate, cart, request_key)` returns
    /// the same [`Authorized`] and reserves nothing twice.
    ///
    /// Order of checks: replay → signature → signer trust → revocation →
    /// scope (validity, attestation, merchant, category, currency, per-txn)
    /// → revocation again, velocity and budget (atomically, in the store).
    /// The early revocation check is a fast exit; the one inside the store is
    /// the guarantee, because a `revoke` can land between the two.
    pub fn authorize(
        &self,
        mandate: &Mandate,
        cart: &Cart,
        request_key: &str,
    ) -> Result<Authorized, LedgerError> {
        let now = self.clock.now();
        let ctx = ContextId::derive(mandate.id(), cart.hash(), request_key);

        if let Some(rec) = self.store.record(&ctx)? {
            return Ok(authorized_from(&rec));
        }

        if let Err(e) = mandate.verify() {
            return self.deny(
                &ctx,
                now,
                Stage::Authorize,
                DenyReason::MandateSignatureInvalid,
                e.to_string(),
            );
        }
        if !self
            .signers
            .is_trusted(&mandate.body.principal, &mandate.signer)
        {
            return self.deny(
                &ctx,
                now,
                Stage::Authorize,
                DenyReason::MandateSignerUntrusted,
                format!(
                    "key {} is not trusted for {}",
                    hex::encode(mandate.signer),
                    mandate.body.principal
                ),
            );
        }
        if self.store.is_revoked(mandate.id())? {
            return self.deny(
                &ctx,
                now,
                Stage::Authorize,
                DenyReason::MandateRevoked,
                format!("mandate {} is revoked", mandate.id()),
            );
        }
        if let Err((reason, detail)) =
            mandate
                .body
                .scope
                .admits(cart.claims(), cart.attestation(), now)
        {
            return self.deny(&ctx, now, Stage::Authorize, reason, detail);
        }

        let body = EventBody::Authorized {
            mandate: mandate.clone(),
            cart: CartSnapshot::from(cart),
            request_key: request_key.to_owned(),
        };
        match self.store.append(&ctx, now, body)? {
            AppendOutcome::Appended(_) => Ok(Authorized::new(
                ctx,
                mandate.id().clone(),
                *cart.hash(),
                cart.claims().merchant.clone(),
                cart.claims().total.clone(),
            )),
            AppendOutcome::BudgetExceeded { remaining } => self.deny(
                &ctx,
                now,
                Stage::Authorize,
                DenyReason::ScopeTotalExceeded,
                format!(
                    "cart total {} exceeds remaining budget {remaining}",
                    cart.claims().total
                ),
            ),
            AppendOutcome::VelocityExceeded { count } => self.deny(
                &ctx,
                now,
                Stage::Authorize,
                DenyReason::VelocityExceeded,
                format!("{count} authorizations already in the velocity window"),
            ),
            AppendOutcome::MandateRevoked => self.deny(
                &ctx,
                now,
                Stage::Authorize,
                DenyReason::MandateRevoked,
                format!("mandate {} is revoked", mandate.id()),
            ),
            AppendOutcome::NonceAlreadyUsed => {
                Err(StoreError::Corrupt("nonce outcome returned for Authorized".to_owned()).into())
            }
        }
    }

    // ─────────────────────────── record_payment ──────────────────────────

    /// Verify `proof` with the rail and bind it to the authorized context.
    ///
    /// Denies if the proof's amount differs, if it binds to a different
    /// context or cart, if it binds to nothing, or if its nonce was already
    /// consumed by another context. Re-recording the same proof is idempotent.
    pub fn record_payment(&self, auth: &Authorized, proof: &R::Proof) -> Result<Paid, LedgerError> {
        let now = self.clock.now();
        let ctx = auth.ctx();
        let Some(rec) = self.store.record(ctx)? else {
            return self.deny(
                ctx,
                now,
                Stage::Payment,
                DenyReason::ContextNotFound,
                format!("no context {ctx}"),
            );
        };

        let expected = PaymentExpectation {
            ctx,
            cart_hash: &rec.cart_hash,
            amount: &rec.amount,
            merchant: &rec.merchant,
        };
        let verified = match self.rail.verify_proof(proof, &expected) {
            Ok(v) => v,
            Err(RailError::InvalidProof(detail)) => {
                return self.deny(ctx, now, Stage::Payment, DenyReason::ProofInvalid, detail);
            }
            Err(RailError::Unavailable(detail)) => {
                return Err(LedgerError::RailUnavailable(detail));
            }
        };
        let rail_id = self.rail.id();

        if rec.state != PaymentState::Authorized {
            let same_payment = rec.rail.as_deref() == Some(rail_id)
                && rec.payment_reference.as_deref() == Some(verified.reference.as_str());
            if same_payment {
                let key = rec.idempotency_key.ok_or_else(|| {
                    StoreError::Corrupt("paid record without idempotency key".to_owned())
                })?;
                return Ok(Paid::new(
                    ctx.clone(),
                    rail_id.to_owned(),
                    verified.reference,
                    key,
                ));
            }
            return self.deny(
                ctx,
                now,
                Stage::Payment,
                DenyReason::InvalidState,
                format!(
                    "context is {:?}; a different payment is already recorded",
                    rec.state
                ),
            );
        }

        self.check_proof_binding(ctx, now, &rec, &verified)?;

        let key = idempotency_key(ctx, rail_id, &verified.nonce);
        let body = EventBody::Paid {
            rail: rail_id.to_owned(),
            reference: verified.reference.clone(),
            nonce: verified.nonce,
            amount: verified.amount,
            idempotency_key: key,
        };
        match self.store.append(ctx, now, body)? {
            AppendOutcome::Appended(_) => Ok(Paid::new(
                ctx.clone(),
                rail_id.to_owned(),
                verified.reference,
                key,
            )),
            AppendOutcome::NonceAlreadyUsed => self.deny(
                ctx,
                now,
                Stage::Payment,
                DenyReason::NonceAlreadyUsed,
                "nonce already consumed by another context".to_owned(),
            ),
            AppendOutcome::BudgetExceeded { .. }
            | AppendOutcome::VelocityExceeded { .. }
            | AppendOutcome::MandateRevoked => {
                Err(StoreError::Corrupt("authorize outcome returned for Paid".to_owned()).into())
            }
        }
    }

    /// Amount and binding checks for a verified proof (properties P1, P2).
    fn check_proof_binding(
        &self,
        ctx: &ContextId,
        now: Timestamp,
        rec: &Record,
        verified: &VerifiedProof,
    ) -> Result<(), LedgerError> {
        if verified.amount != rec.amount {
            return self.deny(
                ctx,
                now,
                Stage::Payment,
                DenyReason::AmountMismatch,
                format!(
                    "proof is for {}, authorized {}",
                    verified.amount, rec.amount
                ),
            );
        }
        let ctx_ok = verified.bound_ctx.as_ref().map(|c| c == ctx);
        let cart_ok = verified.bound_cart.as_ref().map(|h| *h == rec.cart_hash);
        let (reason, detail) = match (ctx_ok, cart_ok) {
            (None, None) => (
                DenyReason::UnboundProof,
                "proof binds to neither context nor cart",
            ),
            (Some(false), _) => (
                DenyReason::ContextBindingMismatch,
                "proof binds to a different context",
            ),
            (_, Some(false)) => (
                DenyReason::CartBindingMismatch,
                "proof binds to a different cart",
            ),
            _ => return Ok(()),
        };
        self.deny(ctx, now, Stage::Payment, reason, detail.to_owned())
    }

    // ────────────────────────── record_settlement ────────────────────────

    /// Ask the rail whether the recorded payment is final.
    ///
    /// Returns [`Settlement::Pending`] without changing state if not yet
    /// final; call again later. On failure the reservation is released.
    pub fn record_settlement(
        &self,
        paid: &Paid,
        finality: &R::Finality,
    ) -> Result<Settlement, LedgerError> {
        let now = self.clock.now();
        let ctx = paid.ctx();
        let Some(rec) = self.store.record(ctx)? else {
            return self.deny(
                ctx,
                now,
                Stage::Settlement,
                DenyReason::ContextNotFound,
                format!("no context {ctx}"),
            );
        };
        if let Some(replay) = self.settlement_replay(ctx, now, &rec)? {
            return Ok(replay);
        }
        let rail_id = self.rail.id();
        if rec.rail.as_deref() != Some(rail_id) {
            return self.deny(
                ctx,
                now,
                Stage::Settlement,
                DenyReason::RailMismatch,
                format!("paid via {:?}, evidence from {rail_id}", rec.rail),
            );
        }
        let payment_reference = rec
            .payment_reference
            .as_deref()
            .ok_or_else(|| corrupt("paid without reference"))?;

        let status = match self.rail.check_finality(
            finality,
            &SettlementExpectation {
                ctx,
                payment_reference,
            },
        ) {
            Ok(s) => s,
            Err(RailError::InvalidProof(detail)) => {
                return self.deny(
                    ctx,
                    now,
                    Stage::Settlement,
                    DenyReason::FinalityInvalid,
                    detail,
                );
            }
            Err(RailError::Unavailable(detail)) => {
                return Err(LedgerError::RailUnavailable(detail));
            }
        };
        match status {
            FinalityStatus::Final { reference } => {
                self.append_plain(
                    ctx,
                    now,
                    EventBody::Settled {
                        rail: rail_id.to_owned(),
                        reference: reference.clone(),
                    },
                )?;
                Ok(Settlement::Settled(Settled::new(ctx.clone(), reference)))
            }
            FinalityStatus::Pending { reason } => Ok(Settlement::Pending { reason }),
            FinalityStatus::Failed { reason } => {
                self.append_plain(
                    ctx,
                    now,
                    EventBody::SettlementFailed {
                        rail: rail_id.to_owned(),
                        reason: reason.clone(),
                    },
                )?;
                Ok(Settlement::Failed(SettlementFailed::new(
                    ctx.clone(),
                    reason,
                )))
            }
        }
    }

    /// If `rec` is already past `Paid`, the idempotent result to return.
    fn settlement_replay(
        &self,
        ctx: &ContextId,
        now: Timestamp,
        rec: &Record,
    ) -> Result<Option<Settlement>, LedgerError> {
        match rec.state {
            PaymentState::Paid => Ok(None),
            PaymentState::Settled => {
                let reference = rec
                    .settlement_reference
                    .clone()
                    .ok_or_else(|| corrupt("settled without reference"))?;
                Ok(Some(Settlement::Settled(Settled::new(
                    ctx.clone(),
                    reference,
                ))))
            }
            PaymentState::SettlementFailed => {
                let reason = rec
                    .failure_reason
                    .clone()
                    .ok_or_else(|| corrupt("failed without reason"))?;
                Ok(Some(Settlement::Failed(SettlementFailed::new(
                    ctx.clone(),
                    reason,
                ))))
            }
            other => self.deny(
                ctx,
                now,
                Stage::Settlement,
                DenyReason::InvalidState,
                format!("context is {other:?}"),
            ),
        }
    }

    // ───────────────────────────── compensate ────────────────────────────

    /// Record that the host undid side effects after a failed settlement.
    pub fn compensate(
        &self,
        failed: &SettlementFailed,
        reference: Option<&str>,
    ) -> Result<Compensated, LedgerError> {
        let now = self.clock.now();
        let ctx = failed.ctx();
        match self.state(ctx)? {
            Some(PaymentState::SettlementFailed) => {}
            Some(PaymentState::Compensated) => return Ok(Compensated::new(ctx.clone())),
            Some(other) => {
                return self.deny(
                    ctx,
                    now,
                    Stage::Compensation,
                    DenyReason::InvalidState,
                    format!("context is {other:?}"),
                );
            }
            None => {
                return self.deny(
                    ctx,
                    now,
                    Stage::Compensation,
                    DenyReason::ContextNotFound,
                    format!("no context {ctx}"),
                );
            }
        }
        self.append_plain(
            ctx,
            now,
            EventBody::Compensated {
                reference: reference.map(str::to_owned),
            },
        )?;
        Ok(Compensated::new(ctx.clone()))
    }

    // ─────────────────────────────── expire ──────────────────────────────

    /// Release an authorization that will not be paid. Idempotent.
    pub fn expire(&self, auth: &Authorized) -> Result<(), LedgerError> {
        let now = self.clock.now();
        let ctx = auth.ctx();
        match self.state(ctx)? {
            Some(PaymentState::Authorized) => {}
            Some(PaymentState::Expired) => return Ok(()),
            Some(other) => {
                return self.deny(
                    ctx,
                    now,
                    Stage::Expiry,
                    DenyReason::InvalidState,
                    format!("context is {other:?}"),
                );
            }
            None => {
                return self.deny(
                    ctx,
                    now,
                    Stage::Expiry,
                    DenyReason::ContextNotFound,
                    format!("no context {ctx}"),
                );
            }
        }
        self.append_plain(ctx, now, EventBody::Expired)?;
        Ok(())
    }

    // ─────────────────────────── record_delivery ─────────────────────────

    /// Record fulfilment. Only reachable with a [`Settled`] token.
    pub fn record_delivery(
        &self,
        settled: &Settled,
        receipt: DeliveryReceipt,
    ) -> Result<Delivered, LedgerError> {
        let now = self.clock.now();
        let ctx = settled.ctx();
        match self.store.record(ctx)? {
            Some(r) if r.state == PaymentState::Settled => {}
            Some(r) if r.state == PaymentState::Delivered => {
                let reference = r
                    .receipt_reference
                    .ok_or_else(|| corrupt("delivered without receipt"))?;
                return Ok(Delivered::new(ctx.clone(), reference));
            }
            Some(r) => {
                return self.deny(
                    ctx,
                    now,
                    Stage::Delivery,
                    DenyReason::InvalidState,
                    format!("context is {:?}", r.state),
                );
            }
            None => {
                return self.deny(
                    ctx,
                    now,
                    Stage::Delivery,
                    DenyReason::ContextNotFound,
                    format!("no context {ctx}"),
                );
            }
        }
        let reference = receipt.reference.clone();
        self.append_plain(ctx, now, EventBody::Delivered { receipt })?;
        Ok(Delivered::new(ctx.clone(), reference))
    }

    // ─────────────────────────────── queries ─────────────────────────────

    /// Reconstruct the typestate token for `ctx` from the store.
    ///
    /// Use this when a lifecycle spans requests: `authorize` in one handler,
    /// `resume` + `record_payment` in the webhook that arrives later.
    pub fn resume(&self, ctx: &ContextId) -> Result<Option<Resumed>, LedgerError> {
        let Some(rec) = self.store.record(ctx)? else {
            return Ok(None);
        };
        let resumed = match rec.state {
            PaymentState::Authorized => Resumed::Authorized(authorized_from(&rec)),
            PaymentState::Paid => Resumed::Paid(Paid::new(
                ctx.clone(),
                rec.rail
                    .clone()
                    .ok_or_else(|| corrupt("paid without rail"))?,
                rec.payment_reference
                    .clone()
                    .ok_or_else(|| corrupt("paid without reference"))?,
                rec.idempotency_key
                    .ok_or_else(|| corrupt("paid without idempotency key"))?,
            )),
            PaymentState::Settled => Resumed::Settled(Settled::new(
                ctx.clone(),
                rec.settlement_reference
                    .clone()
                    .ok_or_else(|| corrupt("settled without reference"))?,
            )),
            PaymentState::SettlementFailed => Resumed::SettlementFailed(SettlementFailed::new(
                ctx.clone(),
                rec.failure_reason
                    .clone()
                    .ok_or_else(|| corrupt("failed without reason"))?,
            )),
            PaymentState::Compensated => Resumed::Compensated(Compensated::new(ctx.clone())),
            PaymentState::Delivered => Resumed::Delivered(Delivered::new(
                ctx.clone(),
                rec.receipt_reference
                    .clone()
                    .ok_or_else(|| corrupt("delivered without receipt"))?,
            )),
            PaymentState::Expired => Resumed::Expired { ctx: ctx.clone() },
        };
        Ok(Some(resumed))
    }

    /// Current state of `ctx`, if it exists.
    pub fn state(&self, ctx: &ContextId) -> Result<Option<PaymentState>, LedgerError> {
        Ok(self.store.record(ctx)?.map(|r| r.state))
    }

    /// The full hash-chained history of `ctx`, or `None` if it has none.
    pub fn evidence(&self, ctx: &ContextId) -> Result<Option<EvidenceBundle>, LedgerError> {
        let events = self.store.events(ctx)?;
        if events.is_empty() {
            return Ok(None);
        }
        Ok(Some(EvidenceBundle::new(
            ctx.clone(),
            self.clock.now(),
            events,
        )))
    }

    /// Revoke a mandate. Future `authorize` calls under it are denied;
    /// contexts already authorized are unaffected.
    pub fn revoke(&self, mandate: &MandateId) -> Result<(), LedgerError> {
        Ok(self.store.revoke(mandate)?)
    }

    // ─────────────────────────────── helpers ─────────────────────────────

    fn deny<T>(
        &self,
        ctx: &ContextId,
        now: Timestamp,
        stage: Stage,
        reason: DenyReason,
        detail: String,
    ) -> Result<T, LedgerError> {
        self.store.append(
            ctx,
            now,
            EventBody::Denied {
                stage,
                reason,
                detail: detail.clone(),
            },
        )?;
        Err(Denied {
            ctx: ctx.clone(),
            stage,
            reason,
            detail,
        }
        .into())
    }

    fn append_plain(
        &self,
        ctx: &ContextId,
        now: Timestamp,
        body: EventBody,
    ) -> Result<(), LedgerError> {
        match self.store.append(ctx, now, body)? {
            AppendOutcome::Appended(_) => Ok(()),
            other => {
                Err(StoreError::Corrupt(format!("unexpected append outcome {other:?}")).into())
            }
        }
    }
}

fn authorized_from(rec: &Record) -> Authorized {
    Authorized::new(
        rec.ctx.clone(),
        rec.mandate_id.clone(),
        rec.cart_hash,
        rec.merchant.clone(),
        rec.amount.clone(),
    )
}

fn idempotency_key(ctx: &ContextId, rail: &str, nonce: &str) -> Hash32 {
    let mut buf = Vec::with_capacity(ctx.as_str().len() + rail.len() + nonce.len() + 2);
    buf.extend_from_slice(ctx.as_str().as_bytes());
    buf.push(0x1f);
    buf.extend_from_slice(rail.as_bytes());
    buf.push(0x1f);
    buf.extend_from_slice(nonce.as_bytes());
    Hash32::of(&buf)
}

fn corrupt(msg: &str) -> LedgerError {
    StoreError::Corrupt(msg.to_owned()).into()
}
