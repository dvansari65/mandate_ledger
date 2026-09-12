#![allow(dead_code)]

use ml_adapters::{MockRail, NativeCart, NativeCartAdapter};
use ml_core::*;
use std::sync::Arc;

pub const USER_KEY: [u8; 32] = [7u8; 32];
pub const MERCHANT_KEY: [u8; 32] = [9u8; 32];
pub const T0: i64 = 1_800_000_000;

pub type TestLedger = Ledger<Arc<MemoryStore>, MockRail, TrustedSigners, Arc<FixedClock>>;

pub fn user_key() -> SigningKey {
    SigningKey::from_bytes(&USER_KEY)
}

pub fn merchant_key() -> SigningKey {
    SigningKey::from_bytes(&MERCHANT_KEY)
}

pub fn principal() -> PrincipalId {
    PrincipalId::new("user:danish").unwrap()
}

pub fn scope() -> Scope {
    Scope {
        merchants: vec![
            MerchantPattern::parse("bigbasket.com").unwrap(),
            MerchantPattern::parse("*.zepto.com").unwrap(),
        ],
        categories: Some(vec![Category::new("grocery").unwrap()]),
        currency: Currency::new("INR").unwrap(),
        max_per_txn: Some(Money::parse("2000", "INR").unwrap()),
        max_total: Some(Money::parse("8000", "INR").unwrap()),
        valid_from: Timestamp(T0),
        valid_until: Timestamp(T0 + 30 * 86_400),
        velocity: Some(Velocity {
            max_count: 5,
            window_secs: 86_400,
        }),
        min_attestation: AttestationLevel::MerchantSigned,
    }
}

pub fn mandate_with(id: &str, scope: Scope) -> Mandate {
    let body = MandateBody {
        id: MandateId::new(id).unwrap(),
        principal: principal(),
        agent: AgentId::new("agent:shopper").unwrap(),
        scope,
        issued_at: Timestamp(T0),
        parent: None,
    };
    Mandate::sign(body, &user_key()).unwrap()
}

pub fn mandate() -> Mandate {
    mandate_with("mnd-1", scope())
}

pub fn adapter() -> NativeCartAdapter {
    NativeCartAdapter::new().with_merchant_key("bb-2026", merchant_key().verifying_key())
}

pub fn native_cart(merchant: &str, total: &str, category: Option<&str>) -> NativeCart {
    NativeCart {
        merchant: merchant.into(),
        total: Money::parse(total, "INR").unwrap(),
        category: category.map(str::to_owned),
        items: vec![serde_json::json!({ "sku": "milk-1l", "qty": 2 })],
        attestation: None,
    }
}

/// A merchant-signed grocery cart at `merchant` for `total` INR.
pub fn cart(merchant: &str, total: &str) -> Cart {
    let signed = native_cart(merchant, total, Some("grocery"))
        .sign("bb-2026", &merchant_key())
        .unwrap();
    adapter().normalize_cart(&signed).unwrap()
}

pub fn unsigned_cart(merchant: &str, total: &str) -> Cart {
    adapter()
        .normalize_cart(&native_cart(merchant, total, Some("grocery")))
        .unwrap()
}

pub struct Harness {
    pub store: Arc<MemoryStore>,
    pub clock: Arc<FixedClock>,
    pub ledger: TestLedger,
}

pub fn harness() -> Harness {
    harness_with_rail(MockRail::new("mock", 1))
}

pub fn harness_with_rail(rail: MockRail) -> Harness {
    let store = Arc::new(MemoryStore::new());
    let clock = Arc::new(FixedClock::at(T0 + 60));
    let signers = TrustedSigners::new().allow(principal(), user_key().verifying_key().to_bytes());
    let ledger = Ledger::new(Arc::clone(&store), rail, signers, Arc::clone(&clock));
    Harness {
        store,
        clock,
        ledger,
    }
}

pub fn reason(err: &LedgerError) -> DenyReason {
    err.reason()
        .unwrap_or_else(|| panic!("expected a denial, got {err}"))
}

pub fn inr(s: &str) -> Money {
    Money::parse(s, "INR").unwrap()
}
