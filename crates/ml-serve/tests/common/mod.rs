//! Shared fixtures: an in-process service over `MemoryStore`, a trusted
//! wallet key, a merchant key, and request helpers.

#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt as _;
use ml_adapters::sandbox::{MemoryChecks, SandboxRail};
use ml_adapters::{NativeAttestation, NativeCart, NativeCartAdapter};
use ml_core::{
    AgentId, AttestationLevel, Currency, FixedClock, Ledger, Mandate, MandateBody, MandateId,
    MemoryStore, MerchantPattern, Money, PrincipalId, Scope, SigningKey, Timestamp, TrustedSigners,
};
use ml_serve::{ApiKeys, App, AppOptions, Backend, Readiness};
use serde_json::{Value, json};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt as _;

pub const KEY: &str = "test-api-key-0123456789abcdef0123456789";
pub const T0: i64 = 1_800_000_000;

pub fn user_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

pub fn merchant_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}

pub fn host_key() -> SigningKey {
    SigningKey::from_bytes(&[11u8; 32])
}

pub fn attacker_key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

pub fn principal() -> PrincipalId {
    PrincipalId::new("user:alice").unwrap()
}

pub fn scope() -> Scope {
    Scope {
        merchants: vec![MerchantPattern::parse("bigbasket.com").unwrap()],
        categories: None,
        currency: Currency::new("INR").unwrap(),
        max_per_txn: Some(Money::parse("3000.00", "INR").unwrap()),
        max_total: Some(Money::parse("8000.00", "INR").unwrap()),
        valid_from: Timestamp(T0),
        valid_until: Timestamp(T0 + 30 * 86_400),
        velocity: None,
        min_attestation: AttestationLevel::AgentReported,
    }
}

pub fn mandate_signed_by(id: &str, scope: Scope, key: &SigningKey) -> Mandate {
    Mandate::sign(
        MandateBody {
            id: MandateId::new(id).unwrap(),
            principal: principal(),
            agent: AgentId::new("agent:shopper").unwrap(),
            scope,
            issued_at: Timestamp(T0),
            parent: None,
        },
        key,
    )
    .unwrap()
}

pub fn mandate_with(id: &str, scope: Scope) -> Mandate {
    mandate_signed_by(id, scope, &user_key())
}

pub fn mandate() -> Mandate {
    mandate_with("mnd-1", scope())
}

pub fn cart(merchant: &str, total: &str) -> Value {
    json!({
        "merchant": merchant,
        "total": {"amount": total, "currency": "INR"},
        "items": [{"sku": "x"}],
    })
}

/// A cart the merchant signed under key id `bb-2026`.
pub fn signed_cart(merchant: &str, total: &str) -> Value {
    let mut native: NativeCart = serde_json::from_value(cart(merchant, total)).unwrap();
    let signature = ed25519_dalek::Signer::sign(&merchant_key(), &native.signing_bytes().unwrap());
    native.attestation = Some(NativeAttestation {
        key_id: "bb-2026".to_owned(),
        signature: signature.to_bytes(),
    });
    serde_json::to_value(native).unwrap()
}

pub fn authorize_body(mandate: &Mandate, cart: Value, request_key: &str) -> Value {
    let mut body = json!({
        "mandate": serde_json::to_value(mandate).unwrap(),
        "request_key": request_key,
    });
    body["cart"] = cart;
    body
}

pub fn proof(reference: &str, amount: &str) -> Value {
    json!({"proof": {"reference": reference, "amount": {"amount": amount, "currency": "INR"}}})
}

pub type TestLedger = Ledger<MemoryStore, SandboxRail, TrustedSigners, Arc<FixedClock>>;

pub struct TestApp {
    pub router: Router,
    pub clock: Arc<FixedClock>,
}

pub fn options() -> AppOptions {
    AppOptions {
        permits: 4,
        request_timeout: Duration::from_secs(5),
        body_limit: 64 * 1024,
    }
}

pub fn ledger() -> (TestLedger, Arc<FixedClock>) {
    let rail = SandboxRail::new("mock", NonZeroU32::MIN, MemoryChecks::new(16));
    let signers = TrustedSigners::new().allow(principal(), user_key().verifying_key().to_bytes());
    let clock = Arc::new(FixedClock::at(T0 + 60));
    (
        Ledger::new(MemoryStore::new(), rail, signers, Arc::clone(&clock)),
        clock,
    )
}

pub fn adapter() -> NativeCartAdapter {
    NativeCartAdapter::new().with_merchant_key("bb-2026", merchant_key().verifying_key())
}

/// The service over any backend, with the test key and the host key.
pub fn app_over(backend: impl Backend, options: AppOptions, readiness: Readiness) -> Router {
    App::new(
        backend,
        adapter(),
        ApiKeys::new([KEY]).unwrap(),
        readiness,
        Some(host_key()),
        options,
    )
    .router()
}

pub fn app() -> TestApp {
    let (ledger, clock) = ledger();
    TestApp {
        router: app_over(ledger, options(), Box::new(|| Ok(()))),
        clock,
    }
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
}

impl Reply {
    pub fn str(&self, field: &str) -> &str {
        self.body[field]
            .as_str()
            .unwrap_or_else(|| panic!("no string `{field}` in {}", self.body))
    }
}

pub async fn call(
    router: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    key: Option<&str>,
) -> Reply {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(key) = key {
        request = request.header(AUTHORIZATION, format!("Bearer {key}"));
    }
    let request = match body {
        Some(body) => request
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    send(router, request).await
}

pub async fn send(router: &Router, request: Request<Body>) -> Reply {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{status}: not JSON ({e}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Reply {
        status,
        headers,
        body,
    }
}

pub async fn post(router: &Router, path: &str, body: Value) -> Reply {
    call(router, "POST", path, Some(body), Some(KEY)).await
}

pub async fn post_empty(router: &Router, path: &str) -> Reply {
    call(router, "POST", path, None, Some(KEY)).await
}

pub async fn get(router: &Router, path: &str) -> Reply {
    call(router, "GET", path, None, Some(KEY)).await
}

/// Authorize `cart` under `mandate`, expecting success; the context id.
pub async fn authorize(router: &Router, mandate: &Mandate, cart: Value, key: &str) -> String {
    let reply = post(
        router,
        "/v1/authorizations",
        authorize_body(mandate, cart, key),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    reply.str("ctx").to_owned()
}

/// Authorize, pay and settle `cart`; the context id.
pub async fn settle(router: &Router, mandate: &Mandate, cart: Value, key: &str) -> String {
    let ctx = authorize(router, mandate, cart, key).await;
    let paid = post(
        router,
        &format!("/v1/contexts/{ctx}/payment"),
        proof(&format!("pay-{key}-ok"), "10.00"),
    )
    .await;
    assert_eq!(paid.status, StatusCode::OK, "{}", paid.body);
    let settled = post(
        router,
        &format!("/v1/contexts/{ctx}/settlement"),
        json!({"finality": {}}),
    )
    .await;
    assert_eq!(settled.status, StatusCode::OK, "{}", settled.body);
    assert_eq!(settled.str("state"), "settled");
    ctx
}
