//! Tamper-evidence of exported bundles.

#![allow(clippy::many_single_char_names)]

mod common;

use common::*;
use ml_adapters::{MockFinality, MockProof};
use ml_core::*;

fn delivered_bundle() -> (Harness, EvidenceBundle) {
    let h = harness();
    let a = h
        .ledger
        .authorize(&mandate(), &cart("bigbasket.com", "50"), "o")
        .unwrap();
    let p = h
        .ledger
        .record_payment(&a, &MockProof::bound_to(a.ctx(), "pay", inr("50")))
        .unwrap();
    let Settlement::Settled(s) = h
        .ledger
        .record_settlement(&p, &MockFinality::confirmed("pay", 2))
        .unwrap()
    else {
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
    let b = h.ledger.evidence(a.ctx()).unwrap().unwrap();
    (h, b)
}

#[test]
fn intact_bundle_verifies_and_roundtrips_json() {
    let (_h, b) = delivered_bundle();
    b.verify().unwrap();
    let json = serde_json::to_string_pretty(&b).unwrap();
    let back: EvidenceBundle = serde_json::from_str(&json).unwrap();
    back.verify().unwrap();
    assert_eq!(back, b);
}

#[test]
fn altered_amount_is_detected() {
    let (_h, mut b) = delivered_bundle();
    let EventBody::Paid { amount, .. } = &mut b.events[1].body else {
        panic!()
    };
    *amount = inr("5000");
    assert_eq!(
        b.verify().unwrap_err(),
        EvidenceError::HashMismatch { seq: 2 }
    );
}

#[test]
fn removed_event_is_detected() {
    let (_h, mut b) = delivered_bundle();
    b.events.remove(1);
    assert_eq!(
        b.verify().unwrap_err(),
        EvidenceError::BrokenChain { seq: 3 }
    );
}

#[test]
fn reordered_events_are_detected() {
    let (_h, mut b) = delivered_bundle();
    b.events.swap(1, 2);
    assert!(b.verify().is_err());
}

#[test]
fn signed_bundle_binds_exporter() {
    let (_h, b) = delivered_bundle();
    let host = SigningKey::from_bytes(&[3u8; 32]);
    let signed = b.sign(&host).unwrap();
    signed.verify().unwrap();

    let mut forged = signed.clone();
    forged.bundle.generated_at = Timestamp(0);
    assert_eq!(
        forged.verify().unwrap_err(),
        EvidenceError::SignatureInvalid
    );
}
