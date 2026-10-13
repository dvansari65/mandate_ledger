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

#[test]
fn prune_forgets_finished_chains_and_old_history_but_never_a_nonce() {
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
    let done = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "10"), "done")
        .unwrap();
    let live = h
        .ledger
        .authorize(&m, &cart("bigbasket.com", "20"), "live")
        .unwrap();
    let paid = h
        .ledger
        .record_payment(&done, &MockProof::bound_to(done.ctx(), "spent", inr("10")))
        .unwrap();
    let Settlement::Settled(settled) = h
        .ledger
        .record_settlement(&paid, &MockFinality::confirmed("spent", 1))
        .unwrap()
    else {
        panic!("final at one confirmation")
    };
    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    h.ledger.record_delivery(&settled, receipt).unwrap();

    // The window is full now. Later, with the history older than the
    // window pruned, it is empty — exactly as it would be with the history
    // kept.
    let third = cart("bigbasket.com", "30");
    assert_eq!(
        reason(&h.ledger.authorize(&m, &third, "third").unwrap_err()),
        DenyReason::VelocityExceeded
    );
    let last = h.store.last_seq().unwrap();
    h.clock.set(T0 + 60 + 200);
    h.store.prune(Timestamp(T0 + 60 + 100)).unwrap();

    // The delivered chain and the refused one are gone; the live one stays.
    assert_eq!(h.store.record(done.ctx()).unwrap(), None);
    assert_eq!(h.store.events(done.ctx()).unwrap(), []);
    assert_eq!(h.ledger.evidence(done.ctx()).unwrap(), None);
    assert_eq!(
        h.store.record(live.ctx()).unwrap().map(|r| r.state),
        Some(PaymentState::Authorized)
    );
    assert_eq!(h.store.events(live.ctx()).unwrap().len(), 1);
    assert_eq!(
        h.store
            .scan(&RecordFilter::default(), None, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.ctx)
            .collect::<Vec<_>>(),
        [live.ctx().clone()]
    );

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
    h.ledger.authorize(&m, &third, "third").unwrap();
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
