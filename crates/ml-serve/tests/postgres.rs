//! The service over the real store. Skipped without `ML_TEST_DATABASE_URL`;
//! fatal if `ML_REQUIRE_DATABASE` is set and the URL is not.

mod common;

use common::*;
use http::StatusCode;
use ml_adapters::sandbox::{MemoryChecks, SandboxRail};
use ml_core::{Ledger, MandateId, SignedEvidence, SystemClock, TrustedSigners};
use ml_serve::App;
use ml_store_postgres::PostgresStore;
use serde_json::json;
use std::num::NonZeroU32;

fn url() -> Option<String> {
    if let Some(url) = std::env::var_os("ML_TEST_DATABASE_URL") {
        return Some(url.to_string_lossy().into_owned());
    }
    assert!(
        std::env::var_os("ML_REQUIRE_DATABASE").is_none(),
        "ML_REQUIRE_DATABASE is set but ML_TEST_DATABASE_URL is not"
    );
    None
}

/// Ids unique to this run: contexts derive from them, and the store keeps
/// what earlier runs left behind.
fn tag() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:x}", nanos ^ u128::from(std::process::id()))
}

#[tokio::test]
async fn the_lifecycle_over_postgres() {
    let Some(url) = url() else {
        eprintln!("skipping: set ML_TEST_DATABASE_URL to run");
        return;
    };
    // The sync client drives its own runtime, so it is never used on a
    // thread that is driving this one — the service keeps the same rule.
    let (store, pool) = tokio::task::spawn_blocking(move || {
        let config: postgres::Config = url.parse().unwrap();
        let pool = r2d2::Pool::builder()
            .max_size(2)
            .min_idle(Some(0))
            .build(r2d2_postgres::PostgresConnectionManager::new(
                config,
                postgres::NoTls,
            ))
            .unwrap();
        let store = PostgresStore::from_pool(pool.clone());
        store.migrate().unwrap();
        (store, pool)
    })
    .await
    .unwrap();
    let tag = tag();
    let rail = SandboxRail::new(
        format!("mock-{tag}"),
        NonZeroU32::MIN,
        MemoryChecks::new(16),
    );
    let signers = TrustedSigners::new().allow(principal(), user_key().verifying_key().to_bytes());
    let ledger = Ledger::new(store, rail, signers, SystemClock);
    let app = App::new(
        ledger,
        adapter(),
        ml_serve::ApiKeys::new([KEY]).unwrap(),
        Box::new(move || {
            let mut conn = pool.get().map_err(|e| e.to_string())?;
            conn.simple_query("SELECT 1").map_err(|e| e.to_string())?;
            Ok(())
        }),
        Some(host_key()),
        options(),
    );
    let router = app.router();

    assert_eq!(
        call(&router, "GET", "/readyz", None, None).await.status,
        StatusCode::OK
    );

    let mandate_id = format!("mnd-{tag}");
    let m = mandate_with(
        &mandate_id,
        ml_core::Scope {
            valid_from: ml_core::Timestamp(0),
            valid_until: ml_core::Timestamp(i64::MAX / 2),
            ..scope()
        },
    );
    let ctx = settle(&router, &m, cart("bigbasket.com", "10.00"), &tag).await;
    let delivered = post(
        &router,
        &format!("/v1/contexts/{ctx}/delivery"),
        json!({"receipt": {"reference": "shipped"}}),
    )
    .await;
    assert_eq!(delivered.status, StatusCode::OK, "{}", delivered.body);

    let listed = get(&router, &format!("/v1/contexts?mandate={mandate_id}")).await;
    assert_eq!(listed.body["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(listed.body["contexts"][0]["state"], "delivered");

    let evidence = get(&router, &format!("/v1/contexts/{ctx}/evidence")).await;
    let signed: SignedEvidence = serde_json::from_value(evidence.body).unwrap();
    signed.verify().unwrap();
    assert_eq!(signed.bundle.events.len(), 4);
    assert_eq!(
        signed.bundle.mandate().unwrap().body.id,
        MandateId::new(&mandate_id).unwrap()
    );

    // The store's connections block to close, and the service holds the
    // last reference to them: it is let go of off this thread.
    drop(router);
    app.close().await;
}
