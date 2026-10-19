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

mod rail {
    //! The x402 rail through the engine, with a payment foundry's `cast`
    //! signed — an EIP-712 implementation independent of this crate's:
    //!
    //! ```text
    //! cast keccak "mandate-ledger:x402-nonce:v1:ctx_9b16b8b344ac38c22944eed7e8b3345e"
    //! cast wallet sign --private-key 0xac09…ff80 --data --from-file lifecycle.json
    //! ```
    //!
    //! The key is Anvil's first well-known test account.

    use super::{T0, cart, key, mandate, reason};
    use ml_adapters::x402::{
        AssetRegistry, FinalityPolicy, Log, PaymentPayload, TransactionReceipt, X402Finality,
        X402Rail, binding_nonce,
    };
    use ml_core::{
        DenyReason, FixedClock, Ledger, MemoryStore, PaymentState, PrincipalId, Settlement,
        TrustedSigners,
    };
    use serde_json::json;
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Duration;

    const PAYEE: &str = "eip155:84532:0x209693bc6afc0c5328ba36faf03c514ef312287c";
    const CTX: &str = "ctx_9b16b8b344ac38c22944eed7e8b3345e";
    const PAYER: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
    const NONCE: &str = "0xbecd8aef3f1d147d770965a8839c467864de74060d9213ed41191672520f4a14";
    const SIGNATURE: &str = "0xa785f0ea8468d3d8688f1be2d9c124229f13bff2967d9a3ee92284a8ed23a43a363e3fa8a5338f6cf946bd8f015fd5c7e91a958c4a2eb66e3ca11f0677528eb71c";
    const TX: &str = "0x5e1f6a0000000000000000000000000000000000000000000000000000000001";

    type X402Ledger =
        Ledger<MemoryStore, X402Rail<Arc<FixedClock>>, TrustedSigners, Arc<FixedClock>>;

    fn ledger() -> X402Ledger {
        ledger_with(Arc::new(FixedClock::at(T0 + 60)))
    }

    /// The ledger and its rail on one clock, as a host wires them.
    fn ledger_with(clock: Arc<FixedClock>) -> X402Ledger {
        let signers = TrustedSigners::new().allow(
            PrincipalId::new("user:alice").unwrap(),
            key().verifying_key().to_bytes(),
        );
        let rail = X402Rail::new(
            "x402",
            AssetRegistry::base_sepolia(),
            FinalityPolicy {
                confirmations: NonZeroU32::new(2).unwrap(),
                trust_facilitator: false,
                expiry_grace: Duration::from_secs(60),
            },
            Arc::clone(&clock),
        );
        Ledger::new(MemoryStore::new(), rail, signers, clock)
    }

    fn payment() -> PaymentPayload {
        serde_json::from_value(json!({
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
            },
            "payload": {
                "signature": SIGNATURE,
                "authorization": {
                    "from": PAYER,
                    "to": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C",
                    "value": "1500000",
                    "validAfter": "0",
                    "validBefore": "1800003600",
                    "nonce": NONCE
                }
            }
        }))
        .unwrap()
    }

    fn topic(address: &str) -> String {
        format!("0x{:0>64}", address[2..].to_ascii_lowercase())
    }

    /// The receipt of the transaction that settled `payment()`, in block
    /// 100, read at `head`.
    fn receipt(head: u64) -> X402Finality {
        let usdc = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
        X402Finality::Receipt {
            network: "eip155:84532".to_owned(),
            head,
            receipt: TransactionReceipt {
                transaction_hash: TX.to_owned(),
                block_number: "0x64".to_owned(),
                status: "0x1".to_owned(),
                logs: vec![
                    Log {
                        address: usdc.to_owned(),
                        topics: vec![
                            "0x98de503528ee59b575ef0c0a2576a82497bfc029a5685b209e9ec333479b10a5"
                                .to_owned(),
                            topic(PAYER),
                            NONCE.to_owned(),
                        ],
                        data: "0x".to_owned(),
                    },
                    Log {
                        address: usdc.to_owned(),
                        topics: vec![
                            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
                                .to_owned(),
                            topic(PAYER),
                            topic("0x209693Bc6afc0C5328bA36FaF03C514EF312287C"),
                        ],
                        data: format!("0x{:064x}", 1_500_000),
                    },
                ],
            },
        }
    }

    #[test]
    fn a_payment_signed_elsewhere_pays_and_settles_its_context() {
        let ledger = ledger();
        let auth = ledger
            .authorize(&mandate(PAYEE, "USDC"), &cart(), "order-1")
            .unwrap();
        assert_eq!(auth.ctx().as_str(), CTX);
        assert_eq!(
            format!("0x{}", hex::encode(binding_nonce(auth.ctx()))),
            NONCE
        );

        let paid = ledger.record_payment(&auth, &payment()).unwrap();
        assert_eq!(paid.rail(), "x402");
        assert_eq!(
            paid.reference(),
            format!(
                "eip155:84532:0x036cbd53842c5426634e7929541ec2318f3dcf7e:{}:{NONCE}:1800003600",
                PAYER.to_ascii_lowercase()
            )
        );

        // One confirmation of two: pending, nothing changes.
        let Settlement::Pending { reason } =
            ledger.record_settlement(&paid, &receipt(100)).unwrap()
        else {
            panic!("one confirmation is not final at two")
        };
        assert_eq!(reason, "1 of 2 confirmations");
        let Settlement::Settled(settled) = ledger.record_settlement(&paid, &receipt(101)).unwrap()
        else {
            panic!("two confirmations are final at two")
        };
        assert_eq!(settled.reference(), TX);
        assert_eq!(
            ledger.state(auth.ctx()).unwrap(),
            Some(PaymentState::Settled)
        );
    }

    #[test]
    fn a_retry_after_the_authorization_expires_is_the_same_payment() {
        let clock = Arc::new(FixedClock::at(T0 + 60));
        let ledger = ledger_with(Arc::clone(&clock));
        let auth = ledger
            .authorize(&mandate(PAYEE, "USDC"), &cart(), "order-1")
            .unwrap();
        let paid = ledger.record_payment(&auth, &payment()).unwrap();
        // The host retries the call after the authorization's window: it
        // gets the payment's own answer, and nothing is written.
        clock.set(1_800_003_600);
        let again = ledger.record_payment(&auth, &payment()).unwrap();
        assert_eq!(again.reference(), paid.reference());
        assert_eq!(again.idempotency_key(), paid.idempotency_key());
        assert_eq!(
            ledger.evidence(auth.ctx()).unwrap().unwrap().events.len(),
            2
        );
    }

    #[test]
    fn the_same_payment_binds_no_other_purchase() {
        let ledger = ledger();
        let m = mandate(PAYEE, "USDC");
        // The same cart under another request key: another context, whose
        // binding nonce this payment does not carry.
        let other = ledger.authorize(&m, &cart(), "order-2").unwrap();
        let err = ledger.record_payment(&other, &payment()).unwrap_err();
        assert_eq!(reason(&err), DenyReason::UnboundProof);
        // Its own context takes it, once.
        let own = ledger.authorize(&m, &cart(), "order-1").unwrap();
        ledger.record_payment(&own, &payment()).unwrap();
    }

    #[test]
    fn a_tampered_payment_is_refused_by_the_rail() {
        let ledger = ledger();
        let auth = ledger
            .authorize(&mandate(PAYEE, "USDC"), &cart(), "order-1")
            .unwrap();
        let mut tampered = payment();
        tampered.payload.authorization.valid_before = "1800007200".to_owned();
        let err = ledger.record_payment(&auth, &tampered).unwrap_err();
        assert_eq!(reason(&err), DenyReason::ProofInvalid);
    }
}
