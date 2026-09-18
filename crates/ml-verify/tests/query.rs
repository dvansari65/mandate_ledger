//! The read surface an operator or a script needs: the whole log, across
//! contexts, and the current position of every context.

mod common;

use common::*;
use ml_adapters::MockProof;
use ml_core::*;

#[test]
fn the_log_spans_contexts_and_pages_by_seq() {
    let h = harness();
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
    assert!(h.store.events_after(end, 100).unwrap().is_empty());
    assert!(h.store.events_after(0, 0).unwrap().is_empty());
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
    assert_eq!(
        ids(h.store.scan(&RecordFilter::default(), 10).unwrap()),
        everything
    );
    assert_eq!(
        ids(h.store.scan(&RecordFilter::default(), 2).unwrap()),
        &everything[..2]
    );

    let under_m1 = RecordFilter {
        mandate: Some(m1.id().clone()),
        ..RecordFilter::default()
    };
    assert_eq!(
        ids(h.store.scan(&under_m1, 10).unwrap()),
        sorted(vec![a.ctx().clone(), b.ctx().clone()])
    );

    let paid = RecordFilter {
        state: Some(PaymentState::Paid),
        ..RecordFilter::default()
    };
    assert_eq!(ids(h.store.scan(&paid, 10).unwrap()), [a.ctx().clone()]);

    let paid_under_m2 = RecordFilter {
        mandate: Some(m2.id().clone()),
        state: Some(PaymentState::Paid),
    };
    assert!(h.store.scan(&paid_under_m2, 10).unwrap().is_empty());
}
