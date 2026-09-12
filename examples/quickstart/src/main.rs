//! End-to-end: a user mandate, a merchant-signed cart, a mock rail — then
//! the full lifecycle, followed by the denials a host will see in practice.
//!
//! Run: `cargo run -p quickstart`

// An example reads best as one top-to-bottom story with the crate prelude in scope.
#![allow(clippy::wildcard_imports, clippy::too_many_lines)]

use ml_adapters::{MockFinality, MockProof, MockRail, NativeCart, NativeCartAdapter};
use ml_core::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Keys. In production: the user's wallet key and the merchant's signing key. ──
    let user_key = SigningKey::from_bytes(&[7u8; 32]);
    let merchant_key = SigningKey::from_bytes(&[9u8; 32]);
    let principal = PrincipalId::new("user:danish")?;

    // ── 1. The user signs a mandate: what the agent may spend. ──
    let now = SystemClock.now();
    let scope = Scope {
        merchants: vec![
            MerchantPattern::parse("bigbasket.com")?,
            MerchantPattern::parse("*.zepto.com")?,
        ],
        categories: Some(vec![Category::new("grocery")?]),
        currency: Currency::new("INR")?,
        max_per_txn: Some(Money::parse("2000.00", "INR")?),
        max_total: Some(Money::parse("8000.00", "INR")?),
        valid_from: now,
        valid_until: Timestamp(now.0 + 30 * 86_400),
        velocity: Some(Velocity {
            max_count: 5,
            window_secs: 86_400,
        }),
        min_attestation: AttestationLevel::MerchantSigned,
    };
    let mandate = Mandate::sign(
        MandateBody {
            id: MandateId::new("mnd_8f2a")?,
            principal: principal.clone(),
            agent: AgentId::new("agent:shopper-v3")?,
            scope,
            issued_at: now,
            parent: None,
        },
        &user_key,
    )?;

    // ── 2. Wire the engine: store + rail + who may sign for whom + clock. ──
    let signers = TrustedSigners::new().allow(principal, user_key.verifying_key().to_bytes());
    let ledger = Ledger::new(
        MemoryStore::new(),
        MockRail::new("mock-upi", 1),
        signers,
        SystemClock,
    );
    let adapter =
        NativeCartAdapter::new().with_merchant_key("bb-2026", merchant_key.verifying_key());

    // ── 3. The merchant produces a signed cart; the adapter normalizes it. ──
    let cart_doc = NativeCart {
        merchant: "bigbasket.com".into(),
        total: Money::parse("128.00", "INR")?,
        category: Some("grocery".into()),
        items: vec![serde_json::json!({ "sku": "milk-1l", "qty": 2, "price": "64.00" })],
        attestation: None,
    }
    .sign("bb-2026", &merchant_key)?;
    let cart = adapter.normalize_cart(&cart_doc)?;
    println!("cart hash      {}", cart.hash());

    // ── 4. The lifecycle. Each call returns a token the next call requires. ──
    let auth = ledger.authorize(&mandate, &cart, "order-1")?;
    println!("authorized     {}  reserved {}", auth.ctx(), auth.amount());

    let proof = MockProof::bound_to(auth.ctx(), "pay_Nx7", Money::parse("128.00", "INR")?);
    let paid = ledger.record_payment(&auth, &proof)?;
    println!(
        "paid           ref={} idem={}",
        paid.reference(),
        paid.idempotency_key()
    );

    let settled = match ledger.record_settlement(&paid, &MockFinality::confirmed("pay_Nx7", 0))? {
        Settlement::Pending { reason } => {
            println!("settlement     pending: {reason}");
            match ledger.record_settlement(&paid, &MockFinality::confirmed("pay_Nx7", 3))? {
                Settlement::Settled(s) => s,
                other => return Err(format!("unexpected {other:?}").into()),
            }
        }
        Settlement::Settled(s) => s,
        Settlement::Failed(f) => return Err(format!("settlement failed: {}", f.reason()).into()),
    };
    println!("settled        {}", settled.reference());

    // `record_delivery` only accepts a `Settled` — passing `paid` would not compile.
    let receipt = DeliveryReceipt {
        reference: "BB-88121".into(),
        attestation: Attestation::MerchantSigned {
            key_id: "bb-2026".into(),
        },
    };
    let delivered = ledger.record_delivery(&settled, receipt)?;
    println!("delivered      {}", delivered.receipt_reference());

    // ── 5. Evidence: self-verifying, exportable, signable by the host. ──
    let bundle = ledger.evidence(auth.ctx())?.expect("context exists");
    bundle.verify()?;
    println!(
        "evidence       {} events, chain verified, final state {:?}",
        bundle.events.len(),
        bundle.final_state()
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&bundle.events.last().unwrap())?
    );

    // ── 6. What denials look like. Every one is also written to the ledger. ──
    println!("\n── denials ──");
    let show = |label: &str, r: Result<Authorized, LedgerError>| match r {
        Ok(_) => println!("{label:<34} ALLOWED (unexpected)"),
        Err(e) => println!(
            "{label:<34} {}",
            e.reason()
                .map_or_else(|| e.to_string(), |r| r.code().to_owned())
        ),
    };

    let other_merchant = adapter.normalize_cart(
        &NativeCart {
            merchant: "amazon.in".into(),
            ..cart_doc.clone()
        }
        .sign("bb-2026", &merchant_key)?,
    )?;
    show(
        "merchant outside scope",
        ledger.authorize(&mandate, &other_merchant, "o2"),
    );

    let unsigned = adapter.normalize_cart(&NativeCart {
        attestation: None,
        ..cart_doc.clone()
    })?;
    show(
        "unsigned cart, mandate wants signed",
        ledger.authorize(&mandate, &unsigned, "o3"),
    );

    let too_big = adapter.normalize_cart(
        &NativeCart {
            total: Money::parse("2500.00", "INR")?,
            ..cart_doc.clone()
        }
        .sign("bb-2026", &merchant_key)?,
    )?;
    show(
        "over per-transaction cap",
        ledger.authorize(&mandate, &too_big, "o4"),
    );

    let mut forged = mandate.clone();
    forged.body.scope.max_total = None;
    show("tampered mandate", ledger.authorize(&forged, &cart, "o5"));

    let second = ledger.authorize(&mandate, &cart, "o6")?;
    let replayed_nonce =
        MockProof::bound_to(second.ctx(), "pay_Nx7", Money::parse("128.00", "INR")?);
    match ledger.record_payment(&second, &replayed_nonce) {
        Err(e) => println!(
            "{:<34} {}",
            "nonce reused on a second context",
            e.reason().unwrap().code()
        ),
        Ok(_) => println!("nonce reuse ALLOWED (unexpected)"),
    }

    ledger.revoke(mandate.id())?;
    show("after revocation", ledger.authorize(&mandate, &cart, "o7"));

    Ok(())
}
