//! The read surface an operator or a script needs: the whole log, across
//! contexts, and the current position of every context.

mod common;

use common::*;
use ml_adapters::{MockFinality, MockProof};
use ml_core::*;

#[test]
fn the_log_spans_contexts_and_pages_by_seq() {
    let h = harness();
    assert_eq!(h.store.last_seq().unwrap(), 0, "an empty log ends at 0");
    let m = mandate();
    let a = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "10"), "o1")
        .unwrap();
    let b = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "20"), "o2")
        .unwrap();
    h.ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay_1", inr("10")))
        .unwrap();
    // A refusal is part of the record too.
    let err = h
        .ledger
        .authorize(&m, &cart("amazon.in", "1"), "o3")
        .unwrap_err();
    let denied = err.denied().unwrap().ctx.clone();

    let all = h.store.events_after(0, 100).unwrap();
    assert!(all.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(
        all.iter().map(|e| &e.ctx).collect::<Vec<_>>(),
        [a.ctx(), b.ctx(), a.ctx(), &denied]
    );

    // The last seq seen is the cursor; the pages concatenate to the log.
    let mut cursor = 0;
    let mut paged = Vec::new();
    loop {
        let page = h.store.events_after(cursor, 3).unwrap();
        let Some(last) = page.last() else { break };
        cursor = last.seq;
        paged.extend(page);
    }
    assert_eq!(paged, all);
    let end = all.last().unwrap().seq;
    assert_eq!(h.store.last_seq().unwrap(), end);
    assert_eq!(h.store.events_after(end, 100).unwrap(), []);
    assert_eq!(h.store.events_after(0, 0).unwrap(), []);
}

#[test]
fn scan_filters_by_mandate_and_state_in_byte_order() {
    let h = harness();
    let m1 = mandate();
    let m2 = mandate_with("mnd-2", scope());
    let a = h
        .ledger
        .authorize(&m1, &cart("bigbasket.com", "10"), "o1")
        .unwrap();
    let b = h
        .ledger
        .authorize(&m1, &cart("bigbasket.com", "20"), "o2")
        .unwrap();
    let c = h
        .ledger
        .authorize(&m2, &cart("bigbasket.com", "30"), "o3")
        .unwrap();
    h.ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay_1", inr("10")))
        .unwrap();

    let ids = |records: Vec<Record>| records.into_iter().map(|r| r.ctx).collect::<Vec<_>>();
    let sorted = |mut ctxs: Vec<ContextId>| {
        ctxs.sort();
        ctxs
    };

    let everything = sorted(vec![a.ctx().clone(), b.ctx().clone(), c.ctx().clone()]);
    let all = RecordFilter::default();
    assert_eq!(ids(h.store.scan(&all, None, 10).unwrap()), everything);
    assert_eq!(ids(h.store.scan(&all, None, 2).unwrap()), &everything[..2]);

    // Keyset paging: the last id seen is the cursor, and a page of one
    // walks the same order as a page of everything.
    let mut walked = Vec::new();
    let mut cursor = None;
    while let Some(record) = h.store.scan(&all, cursor.as_ref(), 1).unwrap().pop() {
        walked.push(record.ctx.clone());
        cursor = Some(record.ctx);
    }
    assert_eq!(walked, everything);

    let under_m1 = RecordFilter {
        mandate: Some(m1.id().clone()),
        ..RecordFilter::default()
    };
    assert_eq!(
        ids(h.store.scan(&under_m1, None, 10).unwrap()),
        sorted(vec![a.ctx().clone(), b.ctx().clone()])
    );

    let paid = RecordFilter {
        state: Some(PaymentState::Paid),
        ..RecordFilter::default()
    };
    assert_eq!(
        ids(h.store.scan(&paid, None, 10).unwrap()),
        [a.ctx().clone()]
    );

    let paid_under_m2 = RecordFilter {
        mandate: Some(m2.id().clone()),
        state: Some(PaymentState::Paid),
    };
    assert_eq!(h.store.scan(&paid_under_m2, None, 10).unwrap(), []);
}

/// Pay, settle and deliver `auth`, spending `nonce`.
fn deliver(h: &Harness, auth: &Authorized, nonce: &str) {
    let proof = MockProof::bound_to(auth.ctx(), nonce, auth.amount().clone());
    let paid = h.ledger.record_payment(auth, &proof).unwrap();
    let Settlement::Settled(settled) = h
        .ledger
        .record_settlement(&paid, &MockFinality::confirmed(nonce, 1))
        .unwrap()
    else {
        panic!("final at one confirmation")
    };
    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    h.ledger.record_delivery(&settled, receipt).unwrap();
}

#[test]
fn prune_forgets_finished_chains_and_keeps_live_ones() {
    let h = harness();
    let m = mandate();
    let done = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "10"), "done")
        .unwrap();
    let live = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "20"), "live")
        .unwrap();
    deliver(&h, &done, "spent");
    // A context that was only ever refused: its refusal is all it has.
    let refused = h
        .ledger
        .authorize(&m, &cart("amazon.in", "1"), "refused")
        .unwrap_err();
    let refused_ctx = refused.denied().unwrap().ctx.clone();
    let last = h.store.last_seq().unwrap();

    h.clock.set(T0 + 60 + 200);
    h.store.prune(Timestamp(T0 + 60 + 100)).unwrap();

    // The delivered chain and the refused one are gone. The delivered
    // record stays — the context is still where it ended — and the live
    // context is untouched.
    assert_eq!(h.store.events(done.ctx()).unwrap(), []);
    assert_eq!(h.ledger.evidence(done.ctx()).unwrap(), None);
    assert_eq!(
        h.store.record(done.ctx()).unwrap().map(|r| r.state),
        Some(PaymentState::Delivered)
    );
    assert_eq!(h.store.events(&refused_ctx).unwrap(), []);
    assert_eq!(h.store.record(&refused_ctx).unwrap(), None);
    assert_eq!(h.store.events(live.ctx()).unwrap().len(), 1);
    assert_eq!(
        h.store.record(live.ctx()).unwrap().map(|r| r.state),
        Some(PaymentState::Authorized)
    );
    let mut both = vec![done.ctx().clone(), live.ctx().clone()];
    both.sort();
    assert_eq!(
        h.store
            .scan(&RecordFilter::default(), None, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.ctx)
            .collect::<Vec<_>>(),
        both
    );
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("30")));

    // A nonce spent on a pruned chain is spent forever.
    assert_eq!(
        reason(
            &h.ledger
                .record_payment(&live, &MockProof::bound_to(live.ctx(), "spent", inr("20")))
                .unwrap_err()
        ),
        DenyReason::NonceAlreadyUsed
    );

    // Sequence numbers carry on from where they were, so a cursor survives:
    // that refusal and this authorization are the next two.
    h.ledger
        .authorize(&m, &cart("bigbasket.com", "30"), "next")
        .unwrap();
    assert_eq!(
        h.store
            .events_after(last, 10)
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        [last + 1, last + 2]
    );
    assert_eq!(h.store.last_seq().unwrap(), last + 2);
}

#[test]
fn a_retry_of_a_pruned_purchase_is_refused_not_authorized_again() {
    let h = harness();
    let m = mandate();
    let cart = cart("bigbasket.com", "10");
    let done = h.ledger.authorize(&m, &cart, "done").unwrap();
    deliver(&h, &done, "spent");

    // With the chain, a retry is the decided purchase: its token, and
    // nothing reserved twice.
    assert_eq!(
        h.ledger.authorize(&m, &cart, "done").unwrap().ctx(),
        done.ctx()
    );
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("10")));

    h.clock.set(T0 + 60 + 200);
    h.store.prune(Timestamp(T0 + 60 + 100)).unwrap();

    // Without it, the mandate the purchase was decided under is gone and
    // the retry cannot be matched, so it is refused — as is a forgery,
    // which without the chain looks exactly the same.
    let err = h.ledger.authorize(&m, &cart, "done").unwrap_err();
    assert_eq!(reason(&err), DenyReason::InvalidState);
    let denied = err.denied().unwrap();
    assert_eq!(denied.ctx, *done.ctx());
    assert!(denied.detail.contains("pruned"), "{}", denied.detail);
    let forged = Mandate::sign(m.body.clone(), &attacker_key()).unwrap();
    assert_eq!(
        reason(&h.ledger.authorize(&forged, &cart, "done").unwrap_err()),
        DenyReason::InvalidState
    );

    // Nothing was reserved again, the record still says delivered, and the
    // chain holds the two refusals and nothing else.
    assert_eq!(h.store.reserved(m.id()).unwrap(), Some(inr("10")));
    assert_eq!(
        h.store.record(done.ctx()).unwrap().map(|r| r.state),
        Some(PaymentState::Delivered)
    );
    let events = h.store.events(done.ctx()).unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|e| matches!(e.body, EventBody::Denied { .. }))
    );
}

#[test]
fn prune_keeps_the_velocity_history_a_window_still_counts() {
    let h = harness();
    let m = mandate_with(
        "mnd-1",
        Scope {
            velocity: Some(Velocity {
                max_count: 2,
                window_secs: 100,
            }),
            ..scope()
        },
    );
    h.ledger
        .authorize(&m, &cart("bigbasket.com", "10"), "one")
        .unwrap();
    h.ledger
        .authorize(&m, &cart("bigbasket.com", "20"), "two")
        .unwrap();
    let third = cart("bigbasket.com", "30");

    // Ninety seconds on, both are still inside the window. Pruning
    // everything before now keeps them, because the window reaches back
    // past `before`: the third purchase is refused exactly as it would be
    // with nothing pruned.
    h.clock.set(T0 + 60 + 90);
    h.store.prune(Timestamp(T0 + 60 + 90)).unwrap();
    assert_eq!(
        reason(&h.ledger.authorize(&m, &third, "three").unwrap_err()),
        DenyReason::VelocityExceeded
    );

    // Once the window has moved past them, it is empty — pruned or not.
    h.clock.set(T0 + 60 + 101);
    h.ledger.authorize(&m, &third, "three").unwrap();
}
