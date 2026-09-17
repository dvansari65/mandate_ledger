//! Persistence boundary and the in-memory reference store.
//!
//! A [`Store`] does three things: keep the current [`Record`] per context,
//! keep the hash-chained [`Event`] log, and — crucially — **apply an event's
//! side effects atomically with appending it**. Budget reservation, velocity
//! counting, nonce consumption and the revocation check all happen inside
//! [`Store::append`], so there is no window between "check" and "commit" for
//! a concurrent request — or a concurrent `revoke` — to slip through. A SQL
//! implementation should make `append` one transaction.

use crate::error::StoreError;
use crate::event::{Event, EventBody};
use crate::hash::Hash32;
use crate::ids::{ContextId, MandateId, MerchantId};
use crate::money::Money;
use crate::state::PaymentState;
use crate::time::Timestamp;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// The current position of one payment context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The context.
    pub ctx: ContextId,
    /// Current state.
    pub state: PaymentState,
    /// Mandate the context was authorized under.
    pub mandate_id: MandateId,
    /// Cart hash bound at authorization.
    pub cart_hash: Hash32,
    /// Merchant bound at authorization.
    pub merchant: MerchantId,
    /// Amount reserved at authorization.
    pub amount: Money,
    /// Rail that recorded payment, once paid.
    pub rail: Option<String>,
    /// Rail payment reference, once paid.
    pub payment_reference: Option<String>,
    /// Idempotency key, once paid.
    pub idempotency_key: Option<Hash32>,
    /// Settlement reference, once settled.
    pub settlement_reference: Option<String>,
    /// Failure reason, once failed.
    pub failure_reason: Option<String>,
    /// Receipt reference, once delivered.
    pub receipt_reference: Option<String>,
}

/// What happened when the store tried to append an event.
#[allow(clippy::large_enum_variant)] // `Appended` carries the event; the others are tiny.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    /// Appended; side effects applied.
    Appended(Event),
    /// `Authorized` refused: reserving the amount would exceed `max_total`.
    BudgetExceeded {
        /// Budget still available before this request.
        remaining: Money,
    },
    /// `Authorized` refused: too many authorizations in the velocity window.
    VelocityExceeded {
        /// Authorizations already in the window.
        count: u32,
    },
    /// `Paid` refused: the nonce was consumed by a different context.
    NonceAlreadyUsed,
    /// `Authorized` refused: the mandate is revoked. The engine checks this
    /// before appending as well, but only the store can decide it atomically
    /// with the reservation.
    MandateRevoked,
}

/// Persistence for the ledger.
///
/// All methods take `&self`; implementations provide their own locking or
/// transactions. Every method must be safe to call concurrently.
pub trait Store: Send + Sync {
    /// The current record for `ctx`, if any.
    fn record(&self, ctx: &ContextId) -> Result<Option<Record>, StoreError>;

    /// All events for `ctx`, in order.
    fn events(&self, ctx: &ContextId) -> Result<Vec<Event>, StoreError>;

    /// Append `body` to `ctx`'s chain and apply its side effects, atomically.
    ///
    /// Must enforce [`PaymentState::can_transition_to`] and return
    /// [`StoreError::IllegalTransition`] otherwise. `Denied` events never
    /// change state and may be appended to a context that does not exist.
    fn append(
        &self,
        ctx: &ContextId,
        at: Timestamp,
        body: EventBody,
    ) -> Result<AppendOutcome, StoreError>;

    /// Total currently reserved against `mandate` (live authorizations).
    fn reserved(&self, mandate: &MandateId) -> Result<Option<Money>, StoreError>;

    /// Whether `mandate` has been revoked.
    fn is_revoked(&self, mandate: &MandateId) -> Result<bool, StoreError>;

    /// Revoke `mandate`. Idempotent. Does not affect contexts already
    /// authorized; every `Authorized` appended afterwards must be refused with
    /// [`AppendOutcome::MandateRevoked`].
    fn revoke(&self, mandate: &MandateId) -> Result<(), StoreError>;
}

#[derive(Default)]
struct Inner {
    records: HashMap<ContextId, Record>,
    events: Vec<Event>,
    by_ctx: HashMap<ContextId, Vec<usize>>,
    reserved: HashMap<MandateId, Money>,
    auth_times: HashMap<MandateId, Vec<Timestamp>>,
    nonces: HashMap<(String, String), ContextId>,
    revoked: HashSet<MandateId>,
}

/// In-memory [`Store`]. Reference implementation and test double; not durable.
#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every event in the store, in global order. For inspection and tests.
    pub fn all_events(&self) -> Result<Vec<Event>, StoreError> {
        Ok(self.lock()?.events.clone())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Backend("memory store lock poisoned".to_owned()))
    }
}

impl Inner {
    fn check_transition(
        &self,
        ctx: &ContextId,
        to: PaymentState,
    ) -> Result<Option<&Record>, StoreError> {
        let existing = self.records.get(ctx);
        let legal = match existing {
            None => to == PaymentState::Authorized,
            Some(r) => r.state.can_transition_to(to),
        };
        if legal {
            Ok(existing)
        } else {
            Err(StoreError::IllegalTransition {
                ctx: ctx.clone(),
                from: existing.map(|r| r.state),
                to,
            })
        }
    }

    fn release(&mut self, mandate: &MandateId, amount: &Money) -> Result<(), StoreError> {
        let current = self
            .reserved
            .get(mandate)
            .ok_or_else(|| StoreError::Corrupt(format!("no reservation for {mandate}")))?;
        let next = current.checked_sub(amount)?;
        if next.is_negative() {
            return Err(StoreError::Corrupt(format!(
                "releasing {amount} from {current} on {mandate} goes negative"
            )));
        }
        self.reserved.insert(mandate.clone(), next);
        Ok(())
    }
}

impl Store for MemoryStore {
    fn record(&self, ctx: &ContextId) -> Result<Option<Record>, StoreError> {
        Ok(self.lock()?.records.get(ctx).cloned())
    }

    fn events(&self, ctx: &ContextId) -> Result<Vec<Event>, StoreError> {
        let g = self.lock()?;
        Ok(g.by_ctx
            .get(ctx)
            .map(|idx| idx.iter().map(|&i| g.events[i].clone()).collect())
            .unwrap_or_default())
    }

    #[allow(clippy::too_many_lines)]
    fn append(
        &self,
        ctx: &ContextId,
        at: Timestamp,
        body: EventBody,
    ) -> Result<AppendOutcome, StoreError> {
        let mut g = self.lock()?;

        // ── Phase 1: validate. Nothing is mutated until every check passes. ──
        let mut new_record: Option<Record> = None;
        let mut reserve: Option<(MandateId, Money)> = None;
        let mut release: Option<(MandateId, Money)> = None;
        let mut consume_nonce: Option<(String, String)> = None;

        match &body {
            EventBody::Authorized { mandate, cart, .. } => {
                g.check_transition(ctx, PaymentState::Authorized)?;
                let scope = &mandate.body.scope;
                let mandate_id = &mandate.body.id;

                if g.revoked.contains(mandate_id) {
                    return Ok(AppendOutcome::MandateRevoked);
                }

                if let Some(v) = scope.velocity {
                    let since = at.saturating_sub_secs(v.window_secs);
                    let count = g
                        .auth_times
                        .get(mandate_id)
                        .map_or(0, |ts| ts.iter().filter(|t| **t >= since).count());
                    let count = u32::try_from(count).unwrap_or(u32::MAX);
                    if count >= v.max_count {
                        return Ok(AppendOutcome::VelocityExceeded { count });
                    }
                }

                let amount = &cart.claims.total;
                let current = g
                    .reserved
                    .get(mandate_id)
                    .cloned()
                    .unwrap_or_else(|| Money::zero(amount.currency().clone()));
                let next = current.checked_add(amount)?;
                if let Some(cap) = &scope.max_total {
                    if next.cmp_same_currency(cap)? == Ordering::Greater {
                        return Ok(AppendOutcome::BudgetExceeded {
                            remaining: cap.checked_sub(&current)?,
                        });
                    }
                }
                reserve = Some((mandate_id.clone(), next));
                new_record = Some(Record {
                    ctx: ctx.clone(),
                    state: PaymentState::Authorized,
                    mandate_id: mandate_id.clone(),
                    cart_hash: cart.hash,
                    merchant: cart.claims.merchant.clone(),
                    amount: amount.clone(),
                    rail: None,
                    payment_reference: None,
                    idempotency_key: None,
                    settlement_reference: None,
                    failure_reason: None,
                    receipt_reference: None,
                });
            }
            EventBody::Paid { rail, nonce, .. } => {
                g.check_transition(ctx, PaymentState::Paid)?;
                let key = (rail.clone(), nonce.clone());
                if g.nonces.get(&key).is_some_and(|owner| owner != ctx) {
                    return Ok(AppendOutcome::NonceAlreadyUsed);
                }
                consume_nonce = Some(key);
            }
            EventBody::Settled { .. } => {
                g.check_transition(ctx, PaymentState::Settled)?;
            }
            EventBody::SettlementFailed { .. } => {
                let r = g.check_transition(ctx, PaymentState::SettlementFailed)?;
                let r = r.ok_or_else(|| StoreError::Corrupt("no record".to_owned()))?;
                release = Some((r.mandate_id.clone(), r.amount.clone()));
            }
            EventBody::Compensated { .. } => {
                g.check_transition(ctx, PaymentState::Compensated)?;
            }
            EventBody::Delivered { .. } => {
                g.check_transition(ctx, PaymentState::Delivered)?;
            }
            EventBody::Expired => {
                let r = g.check_transition(ctx, PaymentState::Expired)?;
                let r = r.ok_or_else(|| StoreError::Corrupt("no record".to_owned()))?;
                release = Some((r.mandate_id.clone(), r.amount.clone()));
            }
            EventBody::Denied { .. } => {}
        }

        let prev_hash = g
            .by_ctx
            .get(ctx)
            .and_then(|idx| idx.last())
            .map_or(Hash32::ZERO, |&i| g.events[i].hash);
        let seq = g.events.len() as u64 + 1;
        let event = Event::new(prev_hash, seq, ctx.clone(), at, body)?;

        // ── Phase 2: commit. ──
        if let Some((m, next)) = reserve {
            g.reserved.insert(m.clone(), next);
            g.auth_times.entry(m).or_default().push(at);
        }
        if let Some((m, amount)) = release {
            g.release(&m, &amount)?;
        }
        if let Some(key) = consume_nonce {
            g.nonces.insert(key, ctx.clone());
        }
        if let Some(r) = new_record {
            g.records.insert(ctx.clone(), r);
        }
        if let Some(r) = g.records.get_mut(ctx) {
            match &event.body {
                EventBody::Paid {
                    rail,
                    reference,
                    idempotency_key,
                    ..
                } => {
                    r.state = PaymentState::Paid;
                    r.rail = Some(rail.clone());
                    r.payment_reference = Some(reference.clone());
                    r.idempotency_key = Some(*idempotency_key);
                }
                EventBody::Settled { reference, .. } => {
                    r.state = PaymentState::Settled;
                    r.settlement_reference = Some(reference.clone());
                }
                EventBody::SettlementFailed { reason, .. } => {
                    r.state = PaymentState::SettlementFailed;
                    r.failure_reason = Some(reason.clone());
                }
                EventBody::Compensated { .. } => r.state = PaymentState::Compensated,
                EventBody::Delivered { receipt } => {
                    r.state = PaymentState::Delivered;
                    r.receipt_reference = Some(receipt.reference.clone());
                }
                EventBody::Expired => r.state = PaymentState::Expired,
                EventBody::Authorized { .. } | EventBody::Denied { .. } => {}
            }
        }

        let index = g.events.len();
        g.events.push(event.clone());
        g.by_ctx.entry(ctx.clone()).or_default().push(index);
        Ok(AppendOutcome::Appended(event))
    }

    fn reserved(&self, mandate: &MandateId) -> Result<Option<Money>, StoreError> {
        Ok(self.lock()?.reserved.get(mandate).cloned())
    }

    fn is_revoked(&self, mandate: &MandateId) -> Result<bool, StoreError> {
        Ok(self.lock()?.revoked.contains(mandate))
    }

    fn revoke(&self, mandate: &MandateId) -> Result<(), StoreError> {
        self.lock()?.revoked.insert(mandate.clone());
        Ok(())
    }
}

impl<S: Store + ?Sized> Store for std::sync::Arc<S> {
    fn record(&self, ctx: &ContextId) -> Result<Option<Record>, StoreError> {
        (**self).record(ctx)
    }
    fn events(&self, ctx: &ContextId) -> Result<Vec<Event>, StoreError> {
        (**self).events(ctx)
    }
    fn append(
        &self,
        ctx: &ContextId,
        at: Timestamp,
        body: EventBody,
    ) -> Result<AppendOutcome, StoreError> {
        (**self).append(ctx, at, body)
    }
    fn reserved(&self, mandate: &MandateId) -> Result<Option<Money>, StoreError> {
        (**self).reserved(mandate)
    }
    fn is_revoked(&self, mandate: &MandateId) -> Result<bool, StoreError> {
        (**self).is_revoked(mandate)
    }
    fn revoke(&self, mandate: &MandateId) -> Result<(), StoreError> {
        (**self).revoke(mandate)
    }
}
