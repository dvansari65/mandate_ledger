//! The reason this crate exists: the four guarantees must survive a restart.
//!
//! With the in-memory store every one of these tests fails on the second half,
//! because a new store starts empty.

#![allow(clippy::many_single_char_names)]

mod common;

use common::*;
use ml_adapters::{MockFinality, MockProof};
use ml_core::*;

#[test]
fn budget_survives_a_restart() {
    let f = pg_or_skip!("budget-restart");
    let m = f.mandate(); // max_total 8000
    let cart = f.cart("3000.00");

    // First process: spend 6,000 of 8,000.
    let ledger = f.ledger();
    for i in 0..2 {
        ledger.authorize(&m, &cart, &format!("order-{i}")).unwrap();
    }
    assert_eq!(f.store.reserved(m.id()).unwrap(), Some(inr("6000.00")));
    drop(ledger);

    // Second process, same database. The spend must still be remembered.
    let restarted = f.ledger_on(f.reconnect());
    assert_eq!(
        restarted.store().reserved(m.id()).unwrap(),
        Some(inr("6000.00"))
    );

    // 6,000 + 3,000 > 8,000, so this must be refused — not silently allowed
    // because the ledger forgot.
    let err = restarted
        .authorize(&m, &cart, "order-after-restart")
        .unwrap_err();
    assert_eq!(err.reason(), Some(DenyReason::ScopeTotalExceeded));
    assert!(
        err.to_string().contains("remaining budget 2000.00 INR"),
        "{err}"
    );
    assert_eq!(
        restarted.store().reserved(m.id()).unwrap(),
        Some(inr("6000.00"))
    );
}

#[test]
fn revocation_survives_a_restart() {
    let f = pg_or_skip!("revoke-restart");
    let m = f.mandate();
    let cart = f.cart("100.00");

    let ledger = f.ledger();
    ledger.authorize(&m, &cart, "before").unwrap();
    ledger.revoke(m.id()).unwrap();
    drop(ledger);

    // The stop button must stay pressed.
    let restarted = f.ledger_on(f.reconnect());
    assert!(restarted.store().is_revoked(m.id()).unwrap());
    let err = restarted.authorize(&m, &cart, "after").unwrap_err();
    assert_eq!(err.reason(), Some(DenyReason::MandateRevoked));
}

#[test]
fn a_spent_nonce_stays_spent_across_a_restart() {
    let f = pg_or_skip!("nonce-restart");
    let scope = Scope {
        max_total: None,
        ..f.scope()
    };
    let m = f.mandate_with(scope);

    let ledger = f.ledger();
    let a = ledger.authorize(&m, &f.cart("100.00"), "first").unwrap();
    ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay-1", inr("100.00")))
        .unwrap();
    drop(ledger);

    // A different context replaying the same payment proof must be refused.
    let restarted = f.ledger_on(f.reconnect());
    let b = restarted
        .authorize(&m, &f.cart("100.00"), "second")
        .unwrap();
    let replay = MockProof::bound_to(b.ctx(), "pay-1", inr("100.00"));
    let err = restarted.record_payment(&b, &replay).unwrap_err();
    assert_eq!(err.reason(), Some(DenyReason::NonceAlreadyUsed));
}

#[test]
fn the_evidence_chain_survives_a_restart_and_still_verifies() {
    let f = pg_or_skip!("evidence-restart");
    let m = f.mandate();
    let cart = f.cart("128.00");

    let ledger = f.ledger();
    let a = ledger.authorize(&m, &cart, "order-1").unwrap();
    let ctx = a.ctx().clone();
    let p = ledger
        .record_payment(&a, &MockProof::bound_to(&ctx, "pay-1", inr("128.00")))
        .unwrap();
    let Settlement::Settled(s) = ledger
        .record_settlement(&p, &MockFinality::confirmed("pay-1", 3))
        .unwrap()
    else {
        panic!("expected settled");
    };
    let receipt = DeliveryReceipt {
        reference: "BB-1".into(),
        attestation: Attestation::AgentReported,
    };
    ledger.record_delivery(&s, receipt).unwrap();
    drop(ledger);

    // Read the chain back from the database in a fresh process and verify it.
    // This also proves the hash preimage round-trips through JSONB unchanged.
    let restarted = f.ledger_on(f.reconnect());
    let bundle = restarted.evidence(&ctx).unwrap().expect("chain is present");
    bundle
        .verify()
        .expect("chain verifies after a round trip through Postgres");
    assert_eq!(bundle.final_state(), Some(PaymentState::Delivered));
    assert_eq!(bundle.events.len(), 4);
    assert_eq!(bundle.mandate().unwrap().id(), m.id());
    assert_eq!(bundle.cart().unwrap().hash, *cart.hash());
    for e in &bundle.events {
        assert!(
            e.verify_hash().unwrap(),
            "event {} does not hash to its contents",
            e.seq
        );
    }
}

#[test]
fn a_resumed_context_can_be_driven_to_completion() {
    let f = pg_or_skip!("resume-restart");
    let m = f.mandate();

    // Request one: authorize, then the process ends.
    let ctx = {
        let ledger = f.ledger();
        let a = ledger.authorize(&m, &f.cart("50.00"), "order-1").unwrap();
        a.ctx().clone()
    };

    // Request two, new process: pick the lifecycle back up from storage.
    let ledger = f.ledger_on(f.reconnect());
    let Some(Resumed::Authorized(a)) = ledger.resume(&ctx).unwrap() else {
        panic!("expected a resumable Authorized context");
    };
    let p = ledger
        .record_payment(&a, &MockProof::bound_to(&ctx, "pay-1", inr("50.00")))
        .unwrap();
    let Settlement::Settled(s) = ledger
        .record_settlement(&p, &MockFinality::confirmed("pay-1", 1))
        .unwrap()
    else {
        panic!("expected settled");
    };
    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    ledger.record_delivery(&s, receipt).unwrap();
    assert_eq!(ledger.state(&ctx).unwrap(), Some(PaymentState::Delivered));
}
