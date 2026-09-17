//! The Postgres store must behave exactly like `MemoryStore`.
//!
//! These are the same properties `ml-verify` asserts against the in-memory
//! store, re-run against real transactions — the budget and nonce races in
//! particular, since that is where a SQL implementation is most likely to
//! diverge.

#![allow(clippy::many_single_char_names)]

mod common;

use common::*;
use ml_adapters::MockProof;
use ml_core::*;
use std::thread;

#[test]
fn budget_holds_under_parallel_authorizations() {
    let f = pg_or_skip!("par-budget");
    // 1,000 cap, 100 per cart: exactly ten may pass, whatever the interleaving.
    let scope = Scope {
        max_per_txn: Some(inr("1000.00")),
        max_total: Some(inr("1000.00")),
        velocity: None,
        ..f.scope()
    };
    let m = f.mandate_with(scope);
    let cart = f.cart("100.00");

    let successes: usize = thread::scope(|s| {
        let handles: Vec<_> = (0..16)
            .map(|t| {
                let ledger = f.ledger();
                let (m, c) = (&m, &cart);
                s.spawn(move || {
                    (0..10)
                        .filter(|i| match ledger.authorize(m, c, &format!("t{t}-{i}")) {
                            Ok(_) => true,
                            Err(e) => {
                                assert_eq!(e.reason(), Some(DenyReason::ScopeTotalExceeded), "{e}");
                                false
                            }
                        })
                        .count()
                })
            })
            .collect();
        handles.into_iter().map(|j| j.join().unwrap()).sum()
    });

    assert_eq!(
        successes, 10,
        "cap / amount authorizations may succeed, no more"
    );
    assert_eq!(f.store.reserved(m.id()).unwrap(), Some(inr("1000.00")));
}

#[test]
fn one_nonce_pays_exactly_one_context() {
    let f = pg_or_skip!("par-nonce");
    let scope = Scope {
        max_total: None,
        velocity: None,
        ..f.scope()
    };
    let m = f.mandate_with(scope);
    let ledger = f.ledger();

    let auths: Vec<_> = (0..8)
        .map(|i| {
            ledger
                .authorize(&m, &f.cart("5.00"), &format!("o{i}"))
                .unwrap()
        })
        .collect();

    let paid: usize = thread::scope(|s| {
        let handles: Vec<_> = auths
            .iter()
            .map(|a| {
                let ledger = f.ledger();
                s.spawn(move || {
                    let proof = MockProof::bound_to(a.ctx(), "shared-nonce", inr("5.00"));
                    match ledger.record_payment(a, &proof) {
                        Ok(_) => 1,
                        Err(e) => {
                            assert_eq!(e.reason(), Some(DenyReason::NonceAlreadyUsed), "{e}");
                            0
                        }
                    }
                })
            })
            .collect();
        handles.into_iter().map(|j| j.join().unwrap()).sum()
    });
    assert_eq!(paid, 1);
}

#[test]
fn velocity_window_is_enforced_and_then_reopens() {
    let f = pg_or_skip!("velocity");
    let scope = Scope {
        max_total: None,
        velocity: Some(Velocity {
            max_count: 3,
            window_secs: 86_400,
        }),
        ..f.scope()
    };
    let m = f.mandate_with(scope);
    let ledger = f.ledger();

    for i in 0..3 {
        ledger
            .authorize(&m, &f.cart("1.00"), &format!("o{i}"))
            .unwrap();
    }
    let err = ledger.authorize(&m, &f.cart("1.00"), "over").unwrap_err();
    assert_eq!(err.reason(), Some(DenyReason::VelocityExceeded));

    f.clock.advance(86_401);
    ledger.authorize(&m, &f.cart("1.00"), "later").unwrap();
}

#[test]
fn the_store_refuses_an_illegal_transition() {
    let f = pg_or_skip!("illegal");
    let m = f.mandate();
    let ledger = f.ledger();
    let a = ledger.authorize(&m, &f.cart("1.00"), "o").unwrap();
    ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay-1", inr("1.00")))
        .unwrap();

    // Delivery from Paid is unreachable through the typed API; this is the
    // store's own guard against a hand-built event.
    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    let err = f
        .store
        .append(a.ctx(), Timestamp(T0), EventBody::Delivered { receipt })
        .unwrap_err();
    assert!(
        matches!(
            err,
            StoreError::IllegalTransition {
                from: Some(PaymentState::Paid),
                to: PaymentState::Delivered,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn authorize_is_idempotent_and_reserves_once() {
    let f = pg_or_skip!("idempotent");
    let m = f.mandate();
    let cart = f.cart("100.00");
    let ledger = f.ledger();

    let a = ledger.authorize(&m, &cart, "order-1").unwrap();
    let b = ledger.authorize(&m, &cart, "order-1").unwrap();
    assert_eq!(a.ctx(), b.ctx());
    assert_eq!(
        f.store.reserved(m.id()).unwrap(),
        Some(inr("100.00")),
        "replay must not reserve twice"
    );
    assert_eq!(f.store.events(a.ctx()).unwrap().len(), 1);
}

#[test]
fn a_failed_settlement_releases_the_reservation() {
    let f = pg_or_skip!("release");
    let m = f.mandate();
    let ledger = f.ledger();
    let a = ledger.authorize(&m, &f.cart("500.00"), "o1").unwrap();
    let p = ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay-1", inr("500.00")))
        .unwrap();
    assert_eq!(f.store.reserved(m.id()).unwrap(), Some(inr("500.00")));

    let Settlement::Failed(failed) = ledger
        .record_settlement(&p, &ml_adapters::MockFinality::failed("pay-1", "reverted"))
        .unwrap()
    else {
        panic!("expected failure");
    };
    assert_eq!(
        f.store.reserved(m.id()).unwrap(),
        Some(inr("0.00")),
        "budget returns on failure"
    );
    ledger.compensate(&failed, Some("refund-1")).unwrap();
    assert_eq!(
        ledger.state(a.ctx()).unwrap(),
        Some(PaymentState::Compensated)
    );

    // A retried failure report after compensation still reads as failed: the
    // row keeps its failure reason across the later transition.
    let Settlement::Failed(again) = ledger
        .record_settlement(&p, &ml_adapters::MockFinality::failed("pay-1", "reverted"))
        .unwrap()
    else {
        panic!("expected the failure to replay");
    };
    assert_eq!(again.reason(), "reverted");
    assert_eq!(f.store.events(a.ctx()).unwrap().len(), 4);
}

#[test]
fn denials_are_recorded_against_their_context() {
    let f = pg_or_skip!("denials");
    let m = f.mandate();
    let ledger = f.ledger();

    // A cart outside the mandate's merchant scope.
    let doc = ml_adapters::NativeCart {
        merchant: "amazon.in".into(),
        total: inr("10.00"),
        category: None,
        items: vec![],
        attestation: None,
    }
    .sign("bb", &merchant_key())
    .unwrap();
    let outside = f.adapter().normalize_cart(&doc).unwrap();

    let err = ledger.authorize(&m, &outside, "k").unwrap_err();
    let denied = err.denied().expect("a denial");
    assert_eq!(denied.reason, DenyReason::ScopeMerchantMismatch);

    let bundle = ledger
        .evidence(&denied.ctx)
        .unwrap()
        .expect("denial was recorded");
    bundle.verify().unwrap();
    assert_eq!(bundle.final_state(), None, "a refusal is not a state");
    assert!(matches!(
        &bundle.events[0].body,
        EventBody::Denied {
            reason: DenyReason::ScopeMerchantMismatch,
            ..
        }
    ));
}

#[test]
fn migrate_on_a_live_database_never_deadlocks_with_appends() {
    let f = pg_or_skip!("migrate-live");
    let scope = Scope {
        max_total: None,
        velocity: None,
        ..f.scope()
    };
    let m = f.mandate_with(scope);
    let cart = f.cart("1.00");
    // One extra pool for every booting replica to share; a pool per thread
    // would exhaust the server's connection limit under `cargo test`.
    let booting = f.reconnect();

    // Half the threads are a service under load; the other half are replicas
    // calling migrate() on boot against a database that is already migrated.
    // Neither side may ever be the victim of a 40P01.
    thread::scope(|s| {
        for t in 0..4 {
            let ledger = f.ledger();
            let (m, c) = (&m, &cart);
            s.spawn(move || {
                for i in 0..40 {
                    ledger.authorize(m, c, &format!("t{t}-{i}")).unwrap();
                }
            });
        }
        for _ in 0..4 {
            let store = std::sync::Arc::clone(&booting);
            s.spawn(move || {
                for _ in 0..40 {
                    store
                        .migrate()
                        .expect("a re-run of migrate must not deadlock against appends");
                }
            });
        }
    });
}
