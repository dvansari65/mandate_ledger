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

/// A store whose `is_revoked` never says yes — the engine's early check
/// disarmed, as a `revoke` committing between that check and the append
/// does — so only what `append` enforces on its own remains.
struct BlindToRevocation(std::sync::Arc<ml_store_postgres::PostgresStore>);

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
fn revocation_is_enforced_inside_append() {
    let f = pg_or_skip!("revoke-in-append");
    let m = f.mandate();
    let ledger = Ledger::new(
        BlindToRevocation(std::sync::Arc::clone(&f.store)),
        ml_adapters::MockRail::new(f.rail_name(), 1),
        f.signers(),
        std::sync::Arc::clone(&f.clock),
    );
    ledger.revoke(m.id()).unwrap();

    let err = ledger.authorize(&m, &f.cart("10.00"), "k").unwrap_err();
    assert_eq!(err.reason(), Some(DenyReason::MandateRevoked), "{err}");
    assert_eq!(
        f.store.reserved(m.id()).unwrap(),
        None,
        "nothing may be reserved under a revoked mandate"
    );
}

#[test]
fn the_log_spans_contexts_and_pages_by_seq() {
    let f = pg_or_skip!("log");
    let m = f.mandate();
    let ledger = f.ledger();
    let a = ledger.authorize(&m, &f.cart("10.00"), "o1").unwrap();
    let b = ledger.authorize(&m, &f.cart("20.00"), "o2").unwrap();
    ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay-1", inr("10.00")))
        .unwrap();
    let denied = ledger
        .authorize(&m, &f.cart("3000.01"), "o3") // over the per-transaction cap
        .unwrap_err()
        .denied()
        .expect("a refusal")
        .ctx
        .clone();

    // Other tests write to the same log at the same time, so read the tail
    // from our first event and keep only our own contexts.
    let mine = [a.ctx(), b.ctx(), &denied];
    let first = f.store.events(a.ctx()).unwrap()[0].seq;
    let ours = |limit: usize| {
        let mut cursor = first - 1;
        let mut kept = Vec::new();
        loop {
            let page = f.store.events_after(cursor, limit).unwrap();
            let Some(last) = page.last() else { break };
            cursor = last.seq;
            kept.extend(page.into_iter().filter(|e| mine.contains(&&e.ctx)));
        }
        kept
    };

    let all = ours(10_000);
    assert_eq!(
        all.iter().map(|e| &e.ctx).collect::<Vec<_>>(),
        [a.ctx(), b.ctx(), a.ctx(), &denied]
    );
    assert!(all.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(ours(2), all, "paging with a small limit reads the same log");
    assert!(f.store.events_after(u64::MAX, 10).unwrap().is_empty());
}

#[test]
fn scan_filters_by_mandate_and_state_in_byte_order() {
    let f = pg_or_skip!("scan");
    let m = f.mandate();
    let ledger = f.ledger();
    let a = ledger.authorize(&m, &f.cart("10.00"), "o1").unwrap();
    let b = ledger.authorize(&m, &f.cart("20.00"), "o2").unwrap();
    ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay-1", inr("10.00")))
        .unwrap();

    // Three more contexts whose ids a locale orders differently from bytes:
    // ASCII puts '_' between 'B' and 'a', en_US puts it first. The scan must
    // agree with the in-memory store, which sorts by bytes.
    let crafted = ["B", "_", "a"].map(|s| ContextId::new(format!("{}-{s}", f.tag)).unwrap());
    for ctx in &crafted {
        let body = EventBody::Authorized {
            mandate: m.clone(),
            cart: CartSnapshot::from(&f.cart("1.00")),
            request_key: "k".into(),
        };
        f.store.append(ctx, Timestamp(T0), body).unwrap();
    }

    // Always narrowed to this test's mandate: the table is shared with the
    // other tests running alongside.
    let under = |state: Option<PaymentState>| RecordFilter {
        mandate: Some(m.id().clone()),
        state,
    };
    let ids = |records: Vec<Record>| records.into_iter().map(|r| r.ctx).collect::<Vec<_>>();
    let mut all = vec![a.ctx().clone(), b.ctx().clone()];
    all.extend(crafted);
    all.sort();

    assert_eq!(ids(f.store.scan(&under(None), 10).unwrap()), all);
    assert_eq!(ids(f.store.scan(&under(None), 1).unwrap()), &all[..1]);
    assert_eq!(
        ids(f.store.scan(&under(Some(PaymentState::Paid)), 10).unwrap()),
        [a.ctx().clone()]
    );
    assert!(
        f.store
            .scan(&under(Some(PaymentState::Delivered)), 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_late_commit_cannot_hide_an_event_from_a_reader_tailing_by_seq() {
    let f = pg_or_skip!("late-commit");
    let m = f.mandate();
    let cart = f.cart("1.00");
    let ledger = f.ledger();

    // The sentinel creates the mandate's reservation row and marks where this
    // test's events begin in the shared log.
    let start = ledger.authorize(&m, &cart, "start").unwrap();
    let start_seq = f.store.events(start.ctx()).unwrap()[0].seq;

    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let pause = std::time::Duration::from_millis(300);

    let (slow_ctx, refused_ctx, first_page, second_page) = thread::scope(|s| {
        // An operator holding the reservation row. It stalls the next
        // authorization *after* that append has drawn its sequence number.
        let (store, mandate_id) = (&f.store, m.id().clone());
        s.spawn(move || {
            let mut conn = store.pool().get().unwrap();
            let mut tx = conn.transaction().unwrap();
            tx.execute(
                "SELECT amount FROM ml_reservations WHERE mandate_id = $1 FOR UPDATE",
                &[&mandate_id.as_str()],
            )
            .unwrap();
            held_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            tx.commit().unwrap();
        });
        held_rx.recv().unwrap();

        let slow = s.spawn(|| ledger.authorize(&m, &cart, "slow").unwrap().ctx().clone());
        thread::sleep(pause); // it draws its seq, then blocks on the row
        // A refusal on a fresh context never touches the reservation row.
        let refused = s.spawn(|| {
            let err = ledger
                .authorize(&m, &f.cart("3000.01"), "over")
                .unwrap_err();
            err.denied().expect("a refusal").ctx.clone()
        });
        thread::sleep(pause);

        // A tailing reader: whatever it sees now, the last seq is its cursor.
        let first_page = f.store.events_after(start_seq, 100).unwrap();
        let cursor = first_page.last().map_or(start_seq, |e| e.seq);

        go_tx.send(()).unwrap(); // the operator lets go; everything commits
        let slow_ctx = slow.join().unwrap();
        let refused_ctx = refused.join().unwrap();
        let second_page = f.store.events_after(cursor, 100).unwrap();
        (slow_ctx, refused_ctx, first_page, second_page)
    });

    let seen: Vec<&ContextId> = first_page
        .iter()
        .chain(&second_page)
        .map(|e| &e.ctx)
        .filter(|c| **c == slow_ctx || **c == refused_ctx)
        .collect();
    assert_eq!(
        seen,
        [&slow_ctx, &refused_ctx],
        "the reader advanced past a sequence number whose transaction had not committed"
    );
}
