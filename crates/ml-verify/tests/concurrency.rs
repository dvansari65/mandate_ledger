//! Properties A2 and A6 under real thread interleaving.

#![allow(clippy::many_single_char_names)]

mod common;

use common::*;
use ml_adapters::MockProof;
use ml_core::*;
use std::thread;

#[test]
fn a6_budget_never_exceeded_under_parallel_authorizations() {
    let h = harness();
    let scope = Scope {
        max_total: Some(inr("1000")),
        velocity: None,
        ..scope()
    };
    let m = mandate_with("mnd-par", scope);
    let c = cart("bigbasket.com", "100");

    let successes: usize = thread::scope(|s| {
        let handles: Vec<_> = (0..16)
            .map(|t| {
                let ledger = &h.ledger;
                let (m, c) = (&m, &c);
                s.spawn(move || {
                    (0..10)
                        .filter(|i| match ledger.authorize(m, c, &format!("t{t}-{i}")) {
                            Ok(_) => true,
                            Err(e) => {
                                assert_eq!(reason(&e), DenyReason::ScopeTotalExceeded);
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
        "exactly cap / amount authorizations may succeed"
    );
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("1000")));
}

#[test]
fn a2_one_nonce_pays_exactly_one_context() {
    let h = harness();
    let scope = Scope {
        max_total: None,
        velocity: None,
        ..scope()
    };
    let m = mandate_with("mnd-nonce", scope);
    let auths: Vec<_> = (0..8)
        .map(|i| {
            h.ledger
                .authorize(&m, &cart("bigbasket.com", "5"), &format!("o{i}"))
                .unwrap()
        })
        .collect();

    let paid: usize = thread::scope(|s| {
        let handles: Vec<_> = auths
            .iter()
            .map(|a| {
                let ledger = &h.ledger;
                s.spawn(move || {
                    let proof = MockProof::bound_to(a.ctx(), "shared-nonce", inr("5"));
                    match ledger.record_payment(a, &proof) {
                        Ok(_) => 1,
                        Err(e) => {
                            assert_eq!(reason(&e), DenyReason::NonceAlreadyUsed);
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
