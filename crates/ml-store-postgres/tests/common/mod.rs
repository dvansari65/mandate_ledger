//! Shared fixtures. Every test namespaces its own ids, so tests are isolated
//! from one another without truncating a shared database and can run in
//! parallel — including against the Postgres service in CI.

// Fixture builders stay methods for a consistent call shape, even where they
// happen not to read `self`.
#![allow(dead_code, clippy::unused_self)]

use ml_adapters::{MockRail, NativeCart, NativeCartAdapter};
use ml_core::{
    AgentId, AttestationLevel, Cart, Currency, FixedClock, Ledger, Mandate, MandateBody, MandateId,
    MerchantPattern, Money, PrincipalId, Scope, SigningKey, Timestamp, TrustedSigners,
};
use std::sync::Arc;

pub const T0: i64 = 1_800_000_000;

/// The database to test against, or `None` to skip.
///
/// Skipping is for a developer machine without Postgres, and for the
/// workspace test job, which is deliberately database-independent. But a
/// silent skip in the job that exists to run these would be a false green,
/// so that job sets `ML_REQUIRE_DATABASE` and a missing URL is fatal there.
///
/// The requirement is stated explicitly rather than inferred from `CI`:
/// every CI job sets `CI`, including the ones that are supposed to skip.
pub fn url() -> Option<String> {
    if let Some(url) = std::env::var_os("ML_TEST_DATABASE_URL") {
        return Some(url.to_string_lossy().into_owned());
    }
    assert!(
        std::env::var_os("ML_REQUIRE_DATABASE").is_none(),
        "ML_REQUIRE_DATABASE is set but ML_TEST_DATABASE_URL is not; \
         these tests must not silently skip in the job that exists to run them"
    );
    None
}

/// Binds a fixture, or returns from the test with a note when no database is
/// configured. Keeps `cargo test` green on a machine without Postgres.
#[macro_export]
macro_rules! pg_or_skip {
    ($tag:expr) => {
        match common::fixture($tag) {
            Some(f) => f,
            None => {
                eprintln!("skipping {}: set ML_TEST_DATABASE_URL to run", $tag);
                return;
            }
        }
    };
}

pub fn user_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

pub fn merchant_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}

pub type TestLedger =
    Ledger<Arc<ml_store_postgres::PostgresStore>, MockRail, TrustedSigners, Arc<FixedClock>>;

pub struct Fixture {
    pub store: Arc<ml_store_postgres::PostgresStore>,
    pub clock: Arc<FixedClock>,
    /// Unique per test, so budgets and nonces never collide across tests.
    pub tag: String,
}

/// A namespace unique to this test run, stable for its whole duration.
///
/// Context ids are derived deterministically from `(mandate, cart hash,
/// request key)`, and the store is now durable — so without this, a second
/// run re-derives the ids of the first, finds those contexts already
/// advanced, and the engine's idempotent-replay path reports success for
/// work it never did. Fresh ids per run keep each run a clean experiment
/// while still exercising real persistence.
fn run_id() -> &'static str {
    static RUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{:x}", nanos ^ u128::from(std::process::id()))
    })
}

/// Connect, migrate, and namespace everything under `tag`.
pub fn fixture(tag: &str) -> Option<Fixture> {
    let store = ml_store_postgres::PostgresStore::connect(&url()?).expect("connect");
    store.migrate().expect("migrate");
    Some(Fixture {
        store: Arc::new(store),
        clock: Arc::new(FixedClock::at(T0 + 60)),
        tag: format!("{tag}-{}", run_id()),
    })
}

impl Fixture {
    /// A second store over the same database — what a restarted process sees.
    pub fn reconnect(&self) -> Arc<ml_store_postgres::PostgresStore> {
        let store = ml_store_postgres::PostgresStore::connect(&url().unwrap()).expect("reconnect");
        Arc::new(store)
    }

    pub fn principal(&self) -> PrincipalId {
        PrincipalId::new(format!("user:{}", self.tag)).unwrap()
    }

    pub fn mandate_id(&self) -> MandateId {
        MandateId::new(format!("mnd-{}", self.tag)).unwrap()
    }

    /// A rail name unique to this test, so nonces never collide.
    pub fn rail_name(&self) -> String {
        format!("mock-{}", self.tag)
    }

    pub fn scope(&self) -> Scope {
        Scope {
            merchants: vec![MerchantPattern::parse("bigbasket.com").unwrap()],
            categories: None,
            currency: Currency::new("INR").unwrap(),
            max_per_txn: Some(Money::parse("3000.00", "INR").unwrap()),
            max_total: Some(Money::parse("8000.00", "INR").unwrap()),
            valid_from: Timestamp(T0),
            valid_until: Timestamp(T0 + 30 * 86_400),
            velocity: None,
            min_attestation: AttestationLevel::MerchantSigned,
        }
    }

    pub fn mandate_with(&self, scope: Scope) -> Mandate {
        Mandate::sign(
            MandateBody {
                id: self.mandate_id(),
                principal: self.principal(),
                agent: AgentId::new("agent:shopper").unwrap(),
                scope,
                issued_at: Timestamp(T0),
                parent: None,
            },
            &user_key(),
        )
        .unwrap()
    }

    pub fn mandate(&self) -> Mandate {
        self.mandate_with(self.scope())
    }

    pub fn adapter(&self) -> NativeCartAdapter {
        NativeCartAdapter::new().with_merchant_key("bb", merchant_key().verifying_key())
    }

    /// A merchant-signed cart at `bigbasket.com` for `total` INR.
    pub fn cart(&self, total: &str) -> Cart {
        let doc = NativeCart {
            merchant: "bigbasket.com".into(),
            total: Money::parse(total, "INR").unwrap(),
            category: None,
            items: vec![serde_json::json!({ "sku": "milk-1l", "qty": 2 })],
            attestation: None,
        }
        .sign("bb", &merchant_key())
        .unwrap();
        self.adapter().normalize_cart(&doc).unwrap()
    }

    pub fn signers(&self) -> TrustedSigners {
        TrustedSigners::new().allow(self.principal(), user_key().verifying_key().to_bytes())
    }

    /// A ledger sharing this fixture's store, with a rail unique to the test.
    pub fn ledger(&self) -> TestLedger {
        self.ledger_on(Arc::clone(&self.store))
    }

    /// A ledger over a specific store handle — used to simulate a restart.
    pub fn ledger_on(&self, store: Arc<ml_store_postgres::PostgresStore>) -> TestLedger {
        Ledger::new(
            store,
            MockRail::new(self.rail_name(), 1),
            self.signers(),
            Arc::clone(&self.clock),
        )
    }
}

pub fn inr(s: &str) -> Money {
    Money::parse(s, "INR").unwrap()
}
