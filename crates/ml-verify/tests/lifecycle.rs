//! The happy path and every idempotent replay along it.

#![allow(clippy::many_single_char_names)]

mod common;

use common::*;
use ml_adapters::{MockFinality, MockProof};
use ml_core::*;

#[test]
fn authorize_pay_settle_deliver() {
    let h = harness();
    let m = mandate();
    let c = cart("bigbasket.com", "128.00");

    let auth = h.ledger.authorize(&m, &c, "order-1").unwrap();
    assert_eq!(auth.amount(), &inr("128.00"));
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("128.00")));
    assert_eq!(
        h.ledger.state(auth.ctx()).unwrap(),
        Some(PaymentState::Authorized)
    );

    let proof = MockProof::bound_to(auth.ctx(), "pay_1", inr("128.00"));
    let paid = h.ledger.record_payment(&auth, &proof).unwrap();
    assert_eq!(paid.reference(), "pay_1");

    let pending = h
        .ledger
        .record_settlement(&paid, &MockFinality::confirmed("pay_1", 0))
        .unwrap();
    assert!(matches!(pending, Settlement::Pending { .. }));
    assert_eq!(
        h.ledger.state(auth.ctx()).unwrap(),
        Some(PaymentState::Paid)
    );

    let Settlement::Settled(settled) = h
        .ledger
        .record_settlement(&paid, &MockFinality::confirmed("pay_1", 1))
        .unwrap()
    else {
        panic!("expected settled");
    };
    assert_eq!(settled.reference(), "pay_1@1");

    let receipt = DeliveryReceipt {
        reference: "BB-88121".into(),
        attestation: Attestation::MerchantSigned {
            key_id: "bb-2026".into(),
        },
    };
    let delivered = h.ledger.record_delivery(&settled, receipt).unwrap();
    assert_eq!(delivered.receipt_reference(), "BB-88121");
    assert_eq!(
        h.ledger.state(auth.ctx()).unwrap(),
        Some(PaymentState::Delivered)
    );

    let bundle = h.ledger.evidence(auth.ctx()).unwrap().unwrap();
    bundle.verify().unwrap();
    assert_eq!(bundle.final_state(), Some(PaymentState::Delivered));
    assert_eq!(bundle.mandate().unwrap().id(), m.id());
    assert_eq!(bundle.cart().unwrap().hash, *c.hash());
    let kinds: Vec<_> = bundle
        .events
        .iter()
        .map(|e| e.body.resulting_state())
        .collect();
    assert_eq!(
        kinds,
        vec![
            Some(PaymentState::Authorized),
            Some(PaymentState::Paid),
            Some(PaymentState::Settled),
            Some(PaymentState::Delivered),
        ]
    );
}

#[test]
fn every_step_is_idempotent() {
    let h = harness();
    let m = mandate();
    let c = cart("bigbasket.com", "100");

    let a1 = h.ledger.authorize(&m, &c, "order-1").unwrap();
    let a2 = h.ledger.authorize(&m, &c, "order-1").unwrap();
    assert_eq!(a1.ctx(), a2.ctx());
    assert_eq!(
        h.store.reserved(m.id()).unwrap(),
        Some(inr("100")),
        "replay must not reserve twice"
    );

    let proof = MockProof::bound_to(a1.ctx(), "pay_1", inr("100"));
    let p1 = h.ledger.record_payment(&a1, &proof).unwrap();
    let p2 = h.ledger.record_payment(&a1, &proof).unwrap();
    assert_eq!(p1.idempotency_key(), p2.idempotency_key());

    let f = MockFinality::confirmed("pay_1", 3);
    let Settlement::Settled(s1) = h.ledger.record_settlement(&p1, &f).unwrap() else {
        panic!()
    };
    let Settlement::Settled(s2) = h.ledger.record_settlement(&p1, &f).unwrap() else {
        panic!()
    };
    assert_eq!(s1.reference(), s2.reference());

    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    let d1 = h.ledger.record_delivery(&s1, receipt.clone()).unwrap();
    let d2 = h.ledger.record_delivery(&s1, receipt).unwrap();
    assert_eq!(d1.receipt_reference(), d2.receipt_reference());

    // Exactly one state-changing event per stage; replays add nothing.
    let bundle = h.ledger.evidence(a1.ctx()).unwrap().unwrap();
    assert_eq!(bundle.events.len(), 4);
}

#[test]
fn a_different_request_key_is_a_different_purchase() {
    let h = harness();
    let m = mandate();
    let c = cart("bigbasket.com", "100");
    let a = h.ledger.authorize(&m, &c, "order-1").unwrap();
    let b = h.ledger.authorize(&m, &c, "order-2").unwrap();
    assert_ne!(a.ctx(), b.ctx());
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("200")));
}

#[test]
fn failed_settlement_releases_budget_then_compensates() {
    let h = harness();
    let m = mandate();
    let a = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "500"), "o1")
        .unwrap();
    let p = h
        .ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay_1", inr("500")))
        .unwrap();
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("500")));

    let Settlement::Failed(failed) = h
        .ledger
        .record_settlement(&p, &MockFinality::failed("pay_1", "reverted"))
        .unwrap()
    else {
        panic!("expected failed");
    };
    assert_eq!(failed.reason(), "reverted");
    assert_eq!(
        h.store.reserved(m.id()).unwrap(),
        Some(inr("0")),
        "budget released on failure"
    );

    let comp = h.ledger.compensate(&failed, Some("refund_9")).unwrap();
    assert_eq!(
        h.ledger.state(comp.ctx()).unwrap(),
        Some(PaymentState::Compensated)
    );
    // idempotent
    h.ledger.compensate(&failed, None).unwrap();
    assert_eq!(h.ledger.evidence(a.ctx()).unwrap().unwrap().events.len(), 4);
}

#[test]
fn expire_releases_budget() {
    let h = harness();
    let m = mandate();
    let a = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "300"), "o1")
        .unwrap();
    h.ledger.expire(&a).unwrap();
    h.ledger.expire(&a).unwrap(); // idempotent
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("0")));
    assert_eq!(
        h.ledger.state(a.ctx()).unwrap(),
        Some(PaymentState::Expired)
    );
    // An expired context cannot be paid.
    let err = h
        .ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "p", inr("300")))
        .unwrap_err();
    assert_eq!(reason(&err), DenyReason::InvalidState);
}

#[test]
fn resume_reconstructs_tokens_across_requests() {
    let h = harness();
    let m = mandate();
    let a = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "10"), "o1")
        .unwrap();
    let ctx = a.ctx().clone();
    drop(a); // the HTTP handler returned; the webhook arrives later

    let Some(Resumed::Authorized(a)) = h.ledger.resume(&ctx).unwrap() else {
        panic!()
    };
    let p = h
        .ledger
        .record_payment(&a, &MockProof::bound_to(&ctx, "pay", inr("10")))
        .unwrap();
    drop(p);

    let Some(Resumed::Paid(p)) = h.ledger.resume(&ctx).unwrap() else {
        panic!()
    };
    let Settlement::Settled(s) = h
        .ledger
        .record_settlement(&p, &MockFinality::confirmed("pay", 1))
        .unwrap()
    else {
        panic!()
    };
    drop(s);

    let Some(Resumed::Settled(s)) = h.ledger.resume(&ctx).unwrap() else {
        panic!()
    };
    h.ledger
        .record_delivery(
            &s,
            DeliveryReceipt {
                reference: "r".into(),
                attestation: Attestation::AgentReported,
            },
        )
        .unwrap();
    assert!(matches!(
        h.ledger.resume(&ctx).unwrap(),
        Some(Resumed::Delivered(_))
    ));
    assert!(
        h.ledger
            .resume(&ContextId::new("ctx_nope").unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn settlement_replays_from_every_later_state() {
    // Finality webhooks are retried, and they arrive whenever the rail feels
    // like it — including after the merchant has already delivered. A retry
    // must get the same answer as the first call, and must not be written
    // into the evidence chain as a refusal.
    let h = harness();
    let m = mandate();

    let a = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "10"), "o1")
        .unwrap();
    let p = h
        .ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay_1", inr("10")))
        .unwrap();
    let f = MockFinality::confirmed("pay_1", 1);
    let Settlement::Settled(s) = h.ledger.record_settlement(&p, &f).unwrap() else {
        panic!("expected settled");
    };
    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    h.ledger.record_delivery(&s, receipt).unwrap();

    let Settlement::Settled(again) = h.ledger.record_settlement(&p, &f).unwrap() else {
        panic!("a retried finality report after delivery must still read as settled");
    };
    assert_eq!(again.reference(), s.reference());
    assert_eq!(
        h.ledger.evidence(a.ctx()).unwrap().unwrap().events.len(),
        4,
        "the retry must not add a Denied event"
    );

    // The same after the failure path has been compensated.
    let b = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "10"), "o2")
        .unwrap();
    let p = h
        .ledger
        .record_payment(&b, &MockProof::bound_to(b.ctx(), "pay_2", inr("10")))
        .unwrap();
    let f = MockFinality::failed("pay_2", "reverted");
    let Settlement::Failed(failed) = h.ledger.record_settlement(&p, &f).unwrap() else {
        panic!("expected failed");
    };
    h.ledger.compensate(&failed, Some("refund_1")).unwrap();

    let Settlement::Failed(again) = h.ledger.record_settlement(&p, &f).unwrap() else {
        panic!("a retried failure report after compensation must still read as failed");
    };
    assert_eq!(again.reason(), "reverted");
    assert_eq!(h.ledger.evidence(b.ctx()).unwrap().unwrap().events.len(), 4);
}
