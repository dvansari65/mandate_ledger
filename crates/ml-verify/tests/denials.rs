//! One test per deny reason, each mapped to a row in docs/threat-model.md.

#![allow(clippy::many_single_char_names)]

mod common;

use common::*;
use ml_adapters::{MockFinality, MockProof, MockRail};
use ml_core::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn authorize_err(h: &Harness, m: &Mandate, c: &Cart) -> DenyReason {
    reason(&h.ledger.authorize(m, c, "k").unwrap_err())
}

#[test]
fn a7_merchant_outside_scope() {
    let h = harness();
    assert_eq!(
        authorize_err(&h, &mandate(), &cart("amazon.in", "10")),
        DenyReason::ScopeMerchantMismatch
    );
    // suffix pattern admits sub-domains only
    assert!(
        h.ledger
            .authorize(&mandate(), &cart("api.zepto.com", "10"), "z")
            .is_ok()
    );
    assert_eq!(
        authorize_err(&h, &mandate(), &cart("zepto.com", "10")),
        DenyReason::ScopeMerchantMismatch
    );
}

#[test]
fn a7_category_missing_fails_closed() {
    let h = harness();
    let no_category = adapter()
        .normalize_cart(
            &native_cart("bigbasket.com", "10", None)
                .sign("bb-2026", &merchant_key())
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        authorize_err(&h, &mandate(), &no_category),
        DenyReason::UnverifiableScope
    );
    let toys = adapter()
        .normalize_cart(
            &native_cart("bigbasket.com", "10", Some("toys"))
                .sign("bb-2026", &merchant_key())
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        authorize_err(&h, &mandate(), &toys),
        DenyReason::ScopeCategoryMismatch
    );
}

#[test]
fn a9_unsigned_cart_when_mandate_requires_signature() {
    let h = harness();
    assert_eq!(
        authorize_err(&h, &mandate(), &unsigned_cart("bigbasket.com", "10")),
        DenyReason::AttestationInsufficient
    );
}

#[test]
fn a7_per_transaction_cap() {
    let h = harness();
    assert_eq!(
        authorize_err(&h, &mandate(), &cart("bigbasket.com", "2000.01")),
        DenyReason::ScopePerTxnExceeded
    );
    assert!(
        h.ledger
            .authorize(&mandate(), &cart("bigbasket.com", "2000.00"), "k")
            .is_ok()
    );
}

#[test]
fn a6_total_budget_across_authorizations() {
    let h = harness();
    let m = mandate();
    for i in 0..4 {
        h.ledger
            .authorize(&m, &cart("bigbasket.com", "2000"), &format!("o{i}"))
            .unwrap();
    }
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("8000")));
    let err = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "0.01"), "o5")
        .unwrap_err();
    assert_eq!(reason(&err), DenyReason::ScopeTotalExceeded);
    assert!(err.to_string().contains("remaining budget 0"));
}

#[test]
fn velocity_window() {
    let h = harness();
    let m = mandate(); // 5 per 24h
    for i in 0..5 {
        h.ledger
            .authorize(&m, &cart("bigbasket.com", "1"), &format!("o{i}"))
            .unwrap();
    }
    assert_eq!(
        authorize_err(&h, &m, &cart("bigbasket.com", "1")),
        DenyReason::VelocityExceeded
    );
    h.clock.advance(86_401);
    assert!(
        h.ledger
            .authorize(&m, &cart("bigbasket.com", "1"), "later")
            .is_ok()
    );
}

#[test]
fn a8_revoked_and_expired_mandates() {
    let h = harness();
    let m = mandate();
    h.ledger.revoke(m.id()).unwrap();
    assert_eq!(
        authorize_err(&h, &m, &cart("bigbasket.com", "1")),
        DenyReason::MandateRevoked
    );

    let h = harness();
    h.clock.set(T0 - 1);
    assert_eq!(
        authorize_err(&h, &mandate(), &cart("bigbasket.com", "1")),
        DenyReason::MandateNotYetValid
    );
    h.clock.set(T0 + 31 * 86_400);
    assert_eq!(
        authorize_err(&h, &mandate(), &cart("bigbasket.com", "1")),
        DenyReason::MandateExpired
    );
}

#[test]
fn forged_or_untrusted_mandates() {
    let h = harness();
    let mut tampered = mandate();
    tampered.body.scope.max_total = None;
    assert_eq!(
        authorize_err(&h, &tampered, &cart("bigbasket.com", "1")),
        DenyReason::MandateSignatureInvalid
    );

    // Valid signature, but from a key nobody trusts for this principal.
    let forged = Mandate::sign(mandate().body, &attacker_key()).unwrap();
    assert!(forged.verify().is_ok());
    assert_eq!(
        authorize_err(&h, &forged, &cart("bigbasket.com", "1")),
        DenyReason::MandateSignerUntrusted
    );
}

#[test]
fn a1_a2_a5_payment_binding_and_nonce() {
    let h = harness();
    let m = mandate();
    let a = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "100"), "o1")
        .unwrap();

    let mut wrong_amount = MockProof::bound_to(a.ctx(), "p", inr("99"));
    assert_eq!(
        reason(&h.ledger.record_payment(&a, &wrong_amount).unwrap_err()),
        DenyReason::AmountMismatch
    );
    wrong_amount.amount = inr("100");

    let unbound = MockProof {
        bound_ctx: None,
        bound_cart: None,
        ..wrong_amount.clone()
    };
    assert_eq!(
        reason(&h.ledger.record_payment(&a, &unbound).unwrap_err()),
        DenyReason::UnboundProof
    );

    let other_ctx = ContextId::new("ctx_other").unwrap();
    let wrong_ctx = MockProof {
        bound_ctx: Some(other_ctx),
        ..wrong_amount.clone()
    };
    assert_eq!(
        reason(&h.ledger.record_payment(&a, &wrong_ctx).unwrap_err()),
        DenyReason::ContextBindingMismatch
    );

    let wrong_cart = MockProof {
        bound_ctx: None,
        bound_cart: Some(Hash32::of(b"other")),
        ..wrong_amount.clone()
    };
    assert_eq!(
        reason(&h.ledger.record_payment(&a, &wrong_cart).unwrap_err()),
        DenyReason::CartBindingMismatch
    );

    let invalid = MockProof {
        valid: false,
        ..wrong_amount.clone()
    };
    assert_eq!(
        reason(&h.ledger.record_payment(&a, &invalid).unwrap_err()),
        DenyReason::ProofInvalid
    );

    // Cart-bound proof is enough on its own.
    let cart_bound = MockProof {
        bound_ctx: None,
        bound_cart: Some(*a.cart_hash()),
        ..wrong_amount.clone()
    };
    h.ledger.record_payment(&a, &cart_bound).unwrap();

    // Same nonce presented for a *different* context is refused.
    let b = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "100"), "o2")
        .unwrap();
    let replay = MockProof::bound_to(b.ctx(), "p", inr("100"));
    assert_eq!(
        reason(&h.ledger.record_payment(&b, &replay).unwrap_err()),
        DenyReason::NonceAlreadyUsed
    );

    // A second, different payment against an already-paid context is refused.
    let second = MockProof::bound_to(a.ctx(), "p2", inr("100"));
    assert_eq!(
        reason(&h.ledger.record_payment(&a, &second).unwrap_err()),
        DenyReason::InvalidState
    );
}

#[test]
fn a10_the_payee_must_be_the_authorized_merchant() {
    let h = harness();
    let a = h
        .ledger
        .authorize(&mandate(), &cart("bigbasket.com", "100"), "o1")
        .unwrap();

    // Bound to the right context and the right cart, paying someone else.
    let redirected = MockProof {
        bound_cart: Some(*a.cart_hash()),
        bound_merchant: Some(MerchantId::new("attacker.example").unwrap()),
        ..MockProof::bound_to(a.ctx(), "p", inr("100"))
    };
    assert_eq!(
        reason(&h.ledger.record_payment(&a, &redirected).unwrap_err()),
        DenyReason::MerchantBindingMismatch
    );

    // The payee is not a binding on its own: the right merchant, for nothing
    // in particular, is still an unbound proof.
    let payee_only = MockProof {
        bound_ctx: None,
        bound_merchant: Some(MerchantId::new("bigbasket.com").unwrap()),
        ..MockProof::bound_to(a.ctx(), "p", inr("100"))
    };
    assert_eq!(
        reason(&h.ledger.record_payment(&a, &payee_only).unwrap_err()),
        DenyReason::UnboundProof
    );

    // Compared as merchant ids are normalized, so spelling is not a mismatch.
    let honest = MockProof {
        bound_merchant: Some(MerchantId::new(" BigBasket.COM ").unwrap()),
        ..MockProof::bound_to(a.ctx(), "p", inr("100"))
    };
    h.ledger.record_payment(&a, &honest).unwrap();
    assert_eq!(
        h.ledger.evidence(a.ctx()).unwrap().unwrap().events.len(),
        4,
        "two refusals, one payment"
    );
}

#[test]
fn a14_a_retry_must_present_the_mandate_the_context_was_authorized_under() {
    let h = harness();
    let m = mandate();
    let c = cart("bigbasket.com", "100");
    let a = h.ledger.authorize(&m, &c, "o1").unwrap();
    let retry = |m: &Mandate| h.ledger.authorize(m, &c, "o1");
    let events = || h.ledger.evidence(a.ctx()).unwrap().unwrap().events.len();

    // The same mandate again is the same purchase attempt.
    assert_eq!(retry(&m).unwrap().ctx(), a.ctx());

    // The same body signed by someone else lands on the same context, since
    // the id is what the context derives from — and is not handed its token.
    let forged = Mandate::sign(m.body.clone(), &attacker_key()).unwrap();
    assert_eq!(
        reason(&retry(&forged).unwrap_err()),
        DenyReason::ContextMismatch
    );

    // The same id and signer with a wider scope is not that purchase either.
    let mut wider = scope();
    wider.max_per_txn = None;
    assert_eq!(
        reason(&retry(&mandate_with("mnd-1", wider)).unwrap_err()),
        DenyReason::ContextMismatch
    );

    // A bad signature is refused for what it is, not as a different mandate.
    let mut corrupted = m.clone();
    corrupted.signature[0] ^= 1;
    assert_eq!(
        reason(&retry(&corrupted).unwrap_err()),
        DenyReason::MandateSignatureInvalid
    );

    // Every refusal is on the context's chain, nothing more was reserved,
    // and the honest retry still answers.
    assert_eq!(events(), 4);
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("100")));
    assert_eq!(retry(&m).unwrap().ctx(), a.ctx());

    // The same holds once the purchase has gone all the way through.
    let p = h
        .ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "p", inr("100")))
        .unwrap();
    let Settlement::Settled(s) = h
        .ledger
        .record_settlement(&p, &MockFinality::confirmed("p", 1))
        .unwrap()
    else {
        panic!("final at one confirmation")
    };
    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    h.ledger.record_delivery(&s, receipt).unwrap();
    assert_eq!(
        reason(&retry(&forged).unwrap_err()),
        DenyReason::ContextMismatch
    );
    assert_eq!(retry(&m).unwrap().ctx(), a.ctx());
    assert_eq!(events(), 8);
}

#[test]
fn a3_delivery_requires_settlement_at_the_store_too() {
    // The type system already prevents `record_delivery(&paid, ..)`.
    // This checks the store's independent guard against a hand-built event.
    let h = harness();
    let a = h
        .ledger
        .authorize(&mandate(), &cart("bigbasket.com", "1"), "o")
        .unwrap();
    h.ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "p", inr("1")))
        .unwrap();
    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    let err = h
        .store
        .append(a.ctx(), Timestamp(T0), EventBody::Delivered { receipt })
        .unwrap_err();
    assert!(matches!(
        err,
        StoreError::IllegalTransition {
            from: Some(PaymentState::Paid),
            to: PaymentState::Delivered,
            ..
        }
    ));
}

#[test]
fn finality_evidence_for_the_wrong_payment_or_rail() {
    let h = harness();
    let a = h
        .ledger
        .authorize(&mandate(), &cart("bigbasket.com", "1"), "o")
        .unwrap();
    let p = h
        .ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay_1", inr("1")))
        .unwrap();
    let err = h
        .ledger
        .record_settlement(&p, &MockFinality::confirmed("pay_OTHER", 5))
        .unwrap_err();
    assert_eq!(reason(&err), DenyReason::FinalityInvalid);

    // Same store, a ledger wired to a different rail: its evidence is not accepted.
    let signers = TrustedSigners::new().allow(principal(), user_key().verifying_key().to_bytes());
    let other = Ledger::new(
        Arc::clone(&h.store),
        MockRail::new("other-rail", 1),
        signers,
        Arc::clone(&h.clock),
    );
    let err = other
        .record_settlement(&p, &MockFinality::confirmed("pay_1", 5))
        .unwrap_err();
    assert_eq!(reason(&err), DenyReason::RailMismatch);
}

#[test]
fn denials_are_recorded_as_evidence() {
    let h = harness();
    let m = mandate();
    let c = cart("amazon.in", "1");
    let err = h.ledger.authorize(&m, &c, "k").unwrap_err();
    let ctx = err.denied().unwrap().ctx.clone();
    let bundle = h.ledger.evidence(&ctx).unwrap().unwrap();
    bundle.verify().unwrap();
    assert_eq!(bundle.final_state(), None, "a denial is not a state");
    assert!(matches!(
        &bundle.events[0].body,
        EventBody::Denied {
            reason: DenyReason::ScopeMerchantMismatch,
            ..
        }
    ));
}

/// A store whose `is_revoked` never says yes. It disarms the engine's early
/// check, which is what a `revoke` landing between that check and the append
/// does in production — leaving only whatever the store enforces inside
/// `append` itself.
struct BlindToRevocation(Arc<MemoryStore>);

impl Store for BlindToRevocation {
    fn record(&self, ctx: &ContextId) -> Result<Option<Record>, StoreError> {
        self.0.record(ctx)
    }
    fn events(&self, ctx: &ContextId) -> Result<Vec<Event>, StoreError> {
        self.0.events(ctx)
    }
    fn append(
        &self,
        ctx: &ContextId,
        at: Timestamp,
        body: EventBody,
    ) -> Result<AppendOutcome, StoreError> {
        self.0.append(ctx, at, body)
    }
    fn events_after(&self, after: u64, limit: usize) -> Result<Vec<Event>, StoreError> {
        self.0.events_after(after, limit)
    }
    fn scan(&self, filter: &RecordFilter, limit: usize) -> Result<Vec<Record>, StoreError> {
        self.0.scan(filter, limit)
    }
    fn last_seq(&self) -> Result<u64, StoreError> {
        self.0.last_seq()
    }
    fn reserved(&self, mandate: &MandateId) -> Result<Option<Money>, StoreError> {
        self.0.reserved(mandate)
    }
    fn is_revoked(&self, _: &MandateId) -> Result<bool, StoreError> {
        Ok(false)
    }
    fn revoke(&self, mandate: &MandateId) -> Result<(), StoreError> {
        self.0.revoke(mandate)
    }
}

#[test]
fn a8_revocation_is_enforced_inside_append_not_only_before_it() {
    let store = Arc::new(MemoryStore::new());
    let signers = TrustedSigners::new().allow(principal(), user_key().verifying_key().to_bytes());
    let ledger = Ledger::new(
        BlindToRevocation(Arc::clone(&store)),
        MockRail::new("mock", 1),
        signers,
        Arc::new(FixedClock::at(T0 + 60)),
    );
    let m = mandate();
    ledger.revoke(m.id()).unwrap();

    let err = ledger
        .authorize(&m, &cart("bigbasket.com", "10"), "k")
        .unwrap_err();
    assert_eq!(reason(&err), DenyReason::MandateRevoked);
    assert_eq!(
        store.reserved(m.id()).unwrap(),
        None,
        "nothing may be reserved under a revoked mandate"
    );
}

/// A store that, once, claims a context has no record although it does:
/// the losing side of two concurrent first authorizations of the same
/// purchase attempt, whose read ran before the winner's commit and whose
/// append runs after it.
struct ForgetsOnce {
    inner: Arc<MemoryStore>,
    forget: AtomicBool,
}

impl Store for ForgetsOnce {
    fn record(&self, ctx: &ContextId) -> Result<Option<Record>, StoreError> {
        if self.forget.swap(false, Ordering::SeqCst) {
            return Ok(None);
        }
        self.inner.record(ctx)
    }
    fn events(&self, ctx: &ContextId) -> Result<Vec<Event>, StoreError> {
        self.inner.events(ctx)
    }
    fn append(
        &self,
        ctx: &ContextId,
        at: Timestamp,
        body: EventBody,
    ) -> Result<AppendOutcome, StoreError> {
        self.inner.append(ctx, at, body)
    }
    fn events_after(&self, after: u64, limit: usize) -> Result<Vec<Event>, StoreError> {
        self.inner.events_after(after, limit)
    }
    fn scan(&self, filter: &RecordFilter, limit: usize) -> Result<Vec<Record>, StoreError> {
        self.inner.scan(filter, limit)
    }
    fn last_seq(&self) -> Result<u64, StoreError> {
        self.inner.last_seq()
    }
    fn reserved(&self, mandate: &MandateId) -> Result<Option<Money>, StoreError> {
        self.inner.reserved(mandate)
    }
    fn is_revoked(&self, mandate: &MandateId) -> Result<bool, StoreError> {
        self.inner.is_revoked(mandate)
    }
    fn revoke(&self, mandate: &MandateId) -> Result<(), StoreError> {
        self.inner.revoke(mandate)
    }
}

#[test]
fn a_retry_that_loses_the_race_with_the_first_authorization_is_still_a_retry() {
    let store = Arc::new(ForgetsOnce {
        inner: Arc::new(MemoryStore::new()),
        forget: AtomicBool::new(false),
    });
    let signers = TrustedSigners::new().allow(principal(), user_key().verifying_key().to_bytes());
    let ledger = Ledger::new(
        Arc::clone(&store),
        MockRail::new("mock", 1),
        signers,
        Arc::new(FixedClock::at(T0 + 60)),
    );
    let m = mandate();
    let c = cart("bigbasket.com", "10");
    let a = ledger.authorize(&m, &c, "k").unwrap();

    // The retry reads no record, passes every check, and its append meets
    // the winner's: the same token, nothing reserved twice, nothing added.
    store.forget.store(true, Ordering::SeqCst);
    assert_eq!(ledger.authorize(&m, &c, "k").unwrap().ctx(), a.ctx());
    assert_eq!(store.inner.reserved(m.id()).unwrap(), Some(inr("10")));
    assert_eq!(store.inner.events(a.ctx()).unwrap().len(), 1);
}
