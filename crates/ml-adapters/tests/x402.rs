//! An x402 cart through the engine: a mandate scoped to the payee on its
//! chain, in the token's currency, authorizes it; the wrong merchant, the
//! same merchant on another chain, or the wrong currency is refused by the
//! engine, with the usual codes.

use ml_adapters::MockRail;
use ml_adapters::x402::{AssetRegistry, X402CartAdapter};
use ml_core::{
    AgentId, AttestationLevel, CartAdapter as _, Currency, DenyReason, FixedClock, Ledger,
    LedgerError, Mandate, MandateBody, MandateId, MemoryStore, MerchantPattern, Money, PrincipalId,
    Scope, SigningKey, Timestamp, TrustedSigners,
};
use serde_json::json;

const T0: i64 = 1_800_000_000;
const PAYEE: &str = "eip155:84532:0x209693bc6afc0c5328ba36faf03c514ef312287c";

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn mandate(merchant: &str, currency: &str) -> Mandate {
    let body = MandateBody {
        id: MandateId::new("mnd-x402").unwrap(),
        principal: PrincipalId::new("user:alice").unwrap(),
        agent: AgentId::new("agent:shopper").unwrap(),
        scope: Scope {
            merchants: vec![MerchantPattern::parse(merchant).unwrap()],
            categories: None,
            currency: Currency::new(currency).unwrap(),
            max_per_txn: Some(Money::parse("5", currency).unwrap()),
            max_total: Some(Money::parse("20", currency).unwrap()),
            valid_from: Timestamp(T0),
            valid_until: Timestamp(T0 + 86_400),
            velocity: None,
            min_attestation: AttestationLevel::AgentReported,
        },
        issued_at: Timestamp(T0),
        parent: None,
    };
    Mandate::sign(body, &key()).unwrap()
}

fn ledger() -> Ledger<MemoryStore, MockRail, TrustedSigners, FixedClock> {
    let signers = TrustedSigners::new().allow(
        PrincipalId::new("user:alice").unwrap(),
        key().verifying_key().to_bytes(),
    );
    Ledger::new(
        MemoryStore::new(),
        MockRail::new("mock", 1),
        signers,
        FixedClock::at(T0 + 60),
    )
}

fn cart() -> ml_core::Cart {
    let document = json!({
        "x402Version": 2,
        "resource": {"url": "https://api.example.com/premium-data"},
        "accepted": {
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": "1500000",
            "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
            "payTo": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C",
            "maxTimeoutSeconds": 60,
            "extra": {"name": "USDC", "version": "2"}
        }
    });
    X402CartAdapter::new(AssetRegistry::base_sepolia())
        .normalize(&serde_json::to_vec(&document).unwrap())
        .unwrap()
}

fn reason(err: &LedgerError) -> DenyReason {
    err.denied().expect("a refusal").reason
}

#[test]
fn a_mandate_for_the_payee_in_usdc_authorizes_the_cart() {
    let auth = ledger()
        .authorize(&mandate(PAYEE, "USDC"), &cart(), "order-1")
        .unwrap();
    assert_eq!(auth.merchant().as_str(), PAYEE);
    assert_eq!(auth.amount().to_string(), "1.500000 USDC");
}

#[test]
fn the_wrong_payee_or_currency_is_refused_by_the_engine() {
    for other in [
        "eip155:84532:0x857b06519e91e3a54538791bdbb0e22373e36b66",
        // The same address on Base mainnet is another merchant.
        "eip155:8453:0x209693bc6afc0c5328ba36faf03c514ef312287c",
    ] {
        let err = ledger()
            .authorize(&mandate(other, "USDC"), &cart(), "order-2")
            .unwrap_err();
        assert_eq!(reason(&err), DenyReason::ScopeMerchantMismatch, "{other}");
    }
    let err = ledger()
        .authorize(&mandate(PAYEE, "INR"), &cart(), "order-3")
        .unwrap_err();
    assert_eq!(reason(&err), DenyReason::CurrencyMismatch);
}
