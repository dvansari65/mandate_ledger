//! One test per deny reason, each mapped to a row in docs/threat-model.md.

#![allow(clippy::many_single_char_names)]

mod common;

use common::*;
use ml_adapters::{MockFinality, MockProof, MockRail};
use ml_core::*;
use std::sync::Arc;

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
    let attacker = SigningKey::from_bytes(&[42u8; 32]);
    let forged = Mandate::sign(mandate().body, &attacker).unwrap();
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
