//! The service, driven in-process over `MemoryStore`: every route, every
//! status code, and the limits.

mod common;

use common::*;
use http::StatusCode;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use ml_core::{AttestationLevel, SignedEvidence};
use ml_serve::{AppOptions, Backend};
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn health_needs_no_key_and_every_response_is_marked() {
    let app = app();
    let reply = call(&app.router, "GET", "/healthz", None, None).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, json!({"status": "ok"}));
    assert_eq!(reply.headers["cache-control"], "no-store");
    assert_eq!(reply.headers["x-content-type-options"], "nosniff");
    assert_eq!(reply.headers["x-request-id"].len(), 24);
    assert_eq!(reply.headers[CONTENT_TYPE], "application/json");
}

#[tokio::test]
async fn request_ids_are_the_services_own() {
    let app = app();
    let request = http::Request::builder()
        .uri("/healthz")
        .header("x-request-id", "chosen-by-the-client")
        .body(axum::body::Body::empty())
        .unwrap();
    let reply = send(&app.router, request).await;
    assert_ne!(reply.headers["x-request-id"], "chosen-by-the-client");
    // Two requests, two ids.
    let other = call(&app.router, "GET", "/healthz", None, None).await;
    assert_ne!(reply.headers["x-request-id"], other.headers["x-request-id"]);
}

#[tokio::test]
async fn readiness_reports_the_store() {
    let (ledger, _) = ledger();
    let router = app_over(ledger, options(), Box::new(|| Err("down".to_owned())));
    let reply = call(&router, "GET", "/readyz", None, None).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.body, json!({"status": "not ready"}));

    let app = app();
    let reply = call(&app.router, "GET", "/readyz", None, None).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, json!({"status": "ok"}));
}

#[tokio::test]
async fn every_v1_route_needs_a_key() {
    let app = app();
    for key in [None, Some("wrong-key-0123456789abcdef0123456789"), Some("")] {
        let reply = call(&app.router, "GET", "/v1/events", None, key).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "key {key:?}");
        assert_eq!(reply.headers["www-authenticate"], "Bearer");
        assert_eq!(reply.body, json!({"error": "missing or invalid API key"}));
    }
    let basic = http::Request::builder()
        .uri("/v1/events")
        .header(AUTHORIZATION, format!("Basic {KEY}"))
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        send(&app.router, basic).await.status,
        StatusCode::UNAUTHORIZED
    );
    let lower = http::Request::builder()
        .uri("/v1/events")
        .header(AUTHORIZATION, format!("bearer {KEY}"))
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(send(&app.router, lower).await.status, StatusCode::OK);
    // The spec needs no key either; an unknown route is a JSON 404.
    assert_eq!(
        call(&app.router, "GET", "/openapi.json", None, None)
            .await
            .status,
        StatusCode::OK
    );
    let missing = call(&app.router, "GET", "/nothing", None, None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.body, json!({"error": "no such route"}));
    let wrong = call(&app.router, "DELETE", "/healthz", None, None).await;
    assert_eq!(wrong.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        wrong.body,
        json!({"error": "method not allowed on this route"})
    );
}

#[tokio::test]
async fn the_lifecycle_end_to_end() {
    let app = app();
    let router = &app.router;
    let m = mandate();

    let auth = post(
        router,
        "/v1/authorizations",
        authorize_body(&m, cart("BigBasket.com", "128.00"), "order-1"),
    )
    .await;
    assert_eq!(auth.status, StatusCode::OK, "{}", auth.body);
    assert_eq!(auth.str("state"), "authorized");
    assert_eq!(auth.str("mandate"), "mnd-1");
    assert_eq!(auth.str("merchant"), "bigbasket.com");
    assert_eq!(
        auth.body["amount"],
        json!({"amount": "128.00", "currency": "INR"})
    );
    assert!(auth.str("cart_hash").starts_with("sha256:"));
    let ctx = auth.str("ctx").to_owned();

    let paid = post(
        router,
        &format!("/v1/contexts/{ctx}/payment"),
        proof("pay-1-ok", "128.00"),
    )
    .await;
    assert_eq!(paid.status, StatusCode::OK, "{}", paid.body);
    assert_eq!(paid.str("state"), "paid");
    assert_eq!(paid.str("rail"), "mock");
    assert_eq!(paid.str("reference"), "pay-1-ok");
    assert!(paid.str("idempotency_key").starts_with("sha256:"));

    // Asked, not told: the reference says it is final.
    let settled = post(
        router,
        &format!("/v1/contexts/{ctx}/settlement"),
        json!({"finality": {}}),
    )
    .await;
    assert_eq!(settled.status, StatusCode::OK, "{}", settled.body);
    assert_eq!(settled.str("state"), "settled");
    assert_eq!(settled.str("reference"), "pay-1-ok@1");

    let delivered = post(
        router,
        &format!("/v1/contexts/{ctx}/delivery"),
        json!({"receipt": {"reference": "order-1/shipped", "signed_by": "bb-2026"}}),
    )
    .await;
    assert_eq!(delivered.status, StatusCode::OK, "{}", delivered.body);
    assert_eq!(delivered.str("state"), "delivered");
    assert_eq!(delivered.str("receipt"), "order-1/shipped");

    let status = get(router, &format!("/v1/contexts/{ctx}")).await;
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(status.str("state"), "delivered");
    assert_eq!(status.str("payment_reference"), "pay-1-ok");
    assert_eq!(status.str("settlement_reference"), "pay-1-ok@1");
    assert_eq!(status.str("receipt_reference"), "order-1/shipped");

    // Evidence: signed by the host, verifiable without the service.
    let evidence = get(router, &format!("/v1/contexts/{ctx}/evidence")).await;
    assert_eq!(evidence.status, StatusCode::OK);
    let signed: SignedEvidence = serde_json::from_value(evidence.body).unwrap();
    signed.verify().unwrap();
    assert_eq!(signed.signer, host_key().verifying_key().to_bytes());
    assert_eq!(signed.bundle.events.len(), 4);
    assert_eq!(signed.bundle.ctx.as_str(), ctx);
}

#[tokio::test]
async fn a_refusal_is_409_with_the_code() {
    let app = app();
    let reply = post(
        &app.router,
        "/v1/authorizations",
        authorize_body(&mandate(), cart("amazon.in", "10.00"), "order-2"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT, "{}", reply.body);
    assert_eq!(reply.str("code"), "SCOPE_MERCHANT_MISMATCH");
    assert_eq!(reply.str("stage"), "authorize");
    assert_eq!(reply.body["recorded"], true);
    assert_ne!(reply.str("ctx"), "");
    assert_ne!(reply.str("detail"), "");
    // The refusal is in the log.
    let events = get(&app.router, "/v1/events").await;
    assert_eq!(events.body["events"][0]["body"]["type"], "denied");
    assert_eq!(
        events.body["events"][0]["body"]["reason"],
        "SCOPE_MERCHANT_MISMATCH"
    );
}

#[tokio::test]
async fn a_retry_is_the_same_purchase_and_a_forgery_is_not() {
    let app = app();
    let m = mandate();
    let first = authorize(&app.router, &m, cart("bigbasket.com", "10.00"), "order-3").await;
    let again = authorize(&app.router, &m, cart("bigbasket.com", "10.00"), "order-3").await;
    assert_eq!(first, again);
    let other = authorize(&app.router, &m, cart("bigbasket.com", "11.00"), "order-3").await;
    assert_ne!(first, other);

    // The same purchase under the same mandate signed by another key is
    // a forgery of the context: refused as such, and recorded on it.
    let forged = mandate_signed_by("mnd-1", scope(), &attacker_key());
    let reply = post(
        &app.router,
        "/v1/authorizations",
        authorize_body(&forged, cart("bigbasket.com", "10.00"), "order-3"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.str("code"), "CONTEXT_MISMATCH");
    assert_eq!(reply.str("ctx"), first);
    // A new purchase under it never gets a context: the signer is untrusted.
    let reply = post(
        &app.router,
        "/v1/authorizations",
        authorize_body(&forged, cart("bigbasket.com", "10.00"), "order-3b"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.str("code"), "MANDATE_SIGNER_UNTRUSTED");
}

#[tokio::test]
async fn malformed_requests_are_client_errors_not_refusals() {
    let app = app();
    let router = &app.router;
    let m = mandate();

    // Not JSON.
    let request = http::Request::builder()
        .method("POST")
        .uri("/v1/authorizations")
        .header(AUTHORIZATION, format!("Bearer {KEY}"))
        .header(CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from("{not json"))
        .unwrap();
    let reply = send(router, request).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.body["error"].is_string());

    // A misspelt field is an error, not a different request.
    let mut body = authorize_body(&m, cart("bigbasket.com", "10.00"), "order-4");
    body["reqest_key"] = json!("typo");
    let reply = post(router, "/v1/authorizations", body).await;
    assert_eq!(
        reply.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        reply.body
    );
    assert!(reply.str("error").contains("reqest_key"));

    // Wrong content type.
    let request = http::Request::builder()
        .method("POST")
        .uri("/v1/authorizations")
        .header(AUTHORIZATION, format!("Bearer {KEY}"))
        .header(CONTENT_TYPE, "text/plain")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    assert_eq!(
        send(router, request).await.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    // Too big.
    let mut big = authorize_body(&m, cart("bigbasket.com", "10.00"), "order-4");
    big["cart"]["items"] = json!(vec!["x".repeat(1000); 100]);
    let reply = post(router, "/v1/authorizations", big).await;
    assert_eq!(
        reply.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        reply.body
    );

    // Bounds the engine would otherwise store verbatim.
    let reply = post(
        router,
        "/v1/authorizations",
        authorize_body(&m, cart("bigbasket.com", "10.00"), &"k".repeat(257)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.str("error").starts_with("request_key:"));

    // A mandate that is not one, and a context id that is not one.
    let mut bad = authorize_body(&m, cart("bigbasket.com", "10.00"), "order-4");
    bad["mandate"] = json!({"body": {}});
    let reply = post(router, "/v1/authorizations", bad).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.str("error").starts_with("mandate:"));
    let reply = get(router, &format!("/v1/contexts/{}", "c".repeat(300))).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.str("error").starts_with("ctx:"));

    // Query parameters are checked too.
    for query in ["limit=501", "state=bogus", "after=", "bogus=1"] {
        let reply = get(router, &format!("/v1/contexts?{query}")).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{query}: {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn steps_a_context_never_earned_are_refused_unrecorded() {
    let app = app();
    let router = &app.router;
    let ctx = authorize(
        router,
        &mandate(),
        cart("bigbasket.com", "10.00"),
        "order-5",
    )
    .await;

    let reply = post(
        router,
        &format!("/v1/contexts/{ctx}/settlement"),
        json!({"finality": {"confirmations": 1}}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.str("code"), "INVALID_STATE");
    assert_eq!(reply.str("stage"), "settlement");
    assert_eq!(reply.body["recorded"], false);
    assert!(reply.str("detail").contains("authorized"));

    let reply = post(
        router,
        "/v1/contexts/no-such-context/delivery",
        json!({"receipt": {"reference": "r"}}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.str("code"), "CONTEXT_NOT_FOUND");
    assert_eq!(reply.body["recorded"], false);

    // A read of a context that does not exist is a 404, not a refusal.
    assert_eq!(
        get(router, "/v1/contexts/no-such-context").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(router, "/v1/contexts/no-such-context/evidence")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn settlement_pending_failed_and_compensation() {
    let app = app();
    let router = &app.router;
    let m = mandate();

    // Told: zero confirmations is pending, one is final.
    let ctx = authorize(router, &m, cart("bigbasket.com", "10.00"), "order-6").await;
    let paid = post(
        router,
        &format!("/v1/contexts/{ctx}/payment"),
        proof("pay-6", "10.00"),
    )
    .await;
    assert_eq!(paid.status, StatusCode::OK, "{}", paid.body);
    let pending = post(
        router,
        &format!("/v1/contexts/{ctx}/settlement"),
        json!({"finality": {"confirmations": 0}}),
    )
    .await;
    assert_eq!(pending.status, StatusCode::OK, "{}", pending.body);
    assert_eq!(pending.str("state"), "pending");
    assert!(pending.str("reason").contains("need 1"));
    assert_eq!(
        get(router, &format!("/v1/contexts/{ctx}"))
            .await
            .str("state"),
        "paid"
    );
    let settled = post(
        router,
        &format!("/v1/contexts/{ctx}/settlement"),
        json!({"finality": {"confirmations": 1}}),
    )
    .await;
    assert_eq!(settled.str("state"), "settled");

    // Asked: a reference that fails, then compensation.
    let ctx = authorize(router, &m, cart("bigbasket.com", "20.00"), "order-7").await;
    post(
        router,
        &format!("/v1/contexts/{ctx}/payment"),
        proof("pay-7-fail", "20.00"),
    )
    .await;
    let failed = post(
        router,
        &format!("/v1/contexts/{ctx}/settlement"),
        json!({"finality": {}}),
    )
    .await;
    assert_eq!(failed.status, StatusCode::OK, "{}", failed.body);
    assert_eq!(failed.str("state"), "settlement_failed");
    assert_eq!(failed.str("reason"), "declined by the rail");
    let compensated = post(
        router,
        &format!("/v1/contexts/{ctx}/compensation"),
        json!({"reference": "refund-7"}),
    )
    .await;
    assert_eq!(compensated.status, StatusCode::OK, "{}", compensated.body);
    assert_eq!(compensated.str("state"), "compensated");
    // Again is the same answer.
    let again = post(
        router,
        &format!("/v1/contexts/{ctx}/compensation"),
        json!({}),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK);

    // Telling the rail two things at once is a bad request.
    let reply = post(
        router,
        &format!("/v1/contexts/{ctx}/settlement"),
        json!({"finality": {"confirmations": 1, "failed": "x"}}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn expiry_and_revocation() {
    let app = app();
    let router = &app.router;
    let m = mandate();
    let ctx = authorize(router, &m, cart("bigbasket.com", "10.00"), "order-8").await;
    let expired = post_empty(router, &format!("/v1/contexts/{ctx}/expiry")).await;
    assert_eq!(expired.status, StatusCode::OK, "{}", expired.body);
    assert_eq!(expired.str("state"), "expired");
    assert_eq!(
        get(router, &format!("/v1/contexts/{ctx}"))
            .await
            .str("state"),
        "expired"
    );
    // Paying an expired context is refused by the engine, and recorded.
    let reply = post(
        router,
        &format!("/v1/contexts/{ctx}/payment"),
        proof("pay-8", "10.00"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.str("code"), "INVALID_STATE");
    assert_eq!(reply.body["recorded"], true);

    let revoked = post_empty(router, "/v1/mandates/mnd-1/revocation").await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.body);
    assert_eq!(revoked.body, json!({"mandate": "mnd-1", "revoked": true}));
    let reply = post(
        router,
        "/v1/authorizations",
        authorize_body(&m, cart("bigbasket.com", "10.00"), "order-9"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.str("code"), "MANDATE_REVOKED");
}

#[tokio::test]
async fn a_signed_cart_is_checked_before_the_ledger_sees_it() {
    let app = app();
    let router = &app.router;
    let m = mandate_with(
        "mnd-signed",
        ml_core::Scope {
            min_attestation: AttestationLevel::MerchantSigned,
            ..scope()
        },
    );
    // Unsigned: the engine refuses, and records it.
    let reply = post(
        router,
        "/v1/authorizations",
        authorize_body(&m, cart("bigbasket.com", "10.00"), "order-10"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.str("code"), "ATTESTATION_INSUFFICIENT");
    // Signed: allowed.
    authorize(
        router,
        &m,
        signed_cart("bigbasket.com", "10.00"),
        "order-10",
    )
    .await;
    // Tampered after signing: the adapter rejects it, so there is nothing to refuse.
    let mut tampered = signed_cart("bigbasket.com", "10.00");
    tampered["total"]["amount"] = json!("1.00");
    let reply = post(
        router,
        "/v1/authorizations",
        authorize_body(&m, tampered, "order-11"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
    assert!(reply.str("error").starts_with("cart:"));
}

#[tokio::test]
async fn contexts_and_events_page() {
    let app = app();
    let router = &app.router;
    let m = mandate();
    let mut ctxs = Vec::new();
    for (i, total) in ["10.00", "20.00", "30.00"].iter().enumerate() {
        ctxs.push(
            authorize(
                router,
                &m,
                cart("bigbasket.com", total),
                &format!("order-{i}"),
            )
            .await,
        );
    }
    ctxs.sort();
    post(
        router,
        &format!("/v1/contexts/{}/payment", ctxs[0]),
        proof("pay-a", "10.00"),
    )
    .await;

    let page = get(router, "/v1/contexts?limit=2").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let listed: Vec<&str> = page.body["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["ctx"].as_str().unwrap())
        .collect();
    assert_eq!(listed, &ctxs[..2]);
    assert_eq!(page.str("next"), ctxs[1]);
    let rest = get(router, &format!("/v1/contexts?limit=2&after={}", ctxs[1])).await;
    assert_eq!(rest.body["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(rest.body["contexts"][0]["ctx"], json!(ctxs[2]));
    let empty = get(router, &format!("/v1/contexts?after={}", ctxs[2])).await;
    assert_eq!(empty.body["contexts"], json!([]));
    assert_eq!(empty.str("next"), ctxs[2]);

    let paid = get(router, "/v1/contexts?state=paid&mandate=mnd-1").await;
    assert_eq!(paid.body["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(paid.body["contexts"][0]["state"], "paid");
    let none = get(router, "/v1/contexts?mandate=mnd-9").await;
    assert_eq!(none.body["contexts"], json!([]));
    assert_eq!(none.body["next"], Value::Null);

    // The log: four events; pages concatenate; limit=0 is where now is.
    let all = get(router, "/v1/events").await;
    assert_eq!(all.body["events"].as_array().unwrap().len(), 4);
    assert_eq!(all.body["next"], 4);
    let first = get(router, "/v1/events?after=0&limit=3").await;
    assert_eq!(first.body["events"].as_array().unwrap().len(), 3);
    assert_eq!(first.body["next"], 3);
    let last = get(router, "/v1/events?after=3&limit=3").await;
    assert_eq!(last.body["events"].as_array().unwrap().len(), 1);
    assert_eq!(last.body["events"][0]["body"]["type"], "paid");
    let now = get(router, "/v1/events?limit=0").await;
    assert_eq!(now.body, json!({"events": [], "next": 4}));
}

/// A backend that takes its time over one read, so that permits and the
/// request timeout can be seen to work.
struct Slow<B> {
    inner: B,
    delay: Duration,
}

impl<B: Backend> Backend for Slow<B> {
    fn authorize(
        &self,
        m: &ml_core::Mandate,
        c: &ml_core::Cart,
        k: &str,
    ) -> Result<ml_core::Authorized, ml_core::LedgerError> {
        self.inner.authorize(m, c, k)
    }
    fn record_payment(
        &self,
        a: &ml_core::Authorized,
        p: &ml_adapters::MockProof,
    ) -> Result<ml_core::Paid, ml_core::LedgerError> {
        self.inner.record_payment(a, p)
    }
    fn record_settlement(
        &self,
        p: &ml_core::Paid,
        f: &ml_adapters::sandbox::Finality,
    ) -> Result<ml_core::Settlement, ml_core::LedgerError> {
        self.inner.record_settlement(p, f)
    }
    fn compensate(
        &self,
        f: &ml_core::SettlementFailed,
        r: Option<&str>,
    ) -> Result<ml_core::Compensated, ml_core::LedgerError> {
        self.inner.compensate(f, r)
    }
    fn record_delivery(
        &self,
        s: &ml_core::Settled,
        r: ml_core::DeliveryReceipt,
    ) -> Result<ml_core::Delivered, ml_core::LedgerError> {
        self.inner.record_delivery(s, r)
    }
    fn expire(&self, a: &ml_core::Authorized) -> Result<(), ml_core::LedgerError> {
        self.inner.expire(a)
    }
    fn revoke(&self, m: &ml_core::MandateId) -> Result<(), ml_core::LedgerError> {
        self.inner.revoke(m)
    }
    fn reached(
        &self,
        c: &ml_core::ContextId,
    ) -> Result<Option<ml_core::Reached>, ml_core::LedgerError> {
        self.inner.reached(c)
    }
    fn evidence(
        &self,
        c: &ml_core::ContextId,
    ) -> Result<Option<ml_core::EvidenceBundle>, ml_core::LedgerError> {
        self.inner.evidence(c)
    }
    fn record(
        &self,
        c: &ml_core::ContextId,
    ) -> Result<Option<ml_core::Record>, ml_core::StoreError> {
        std::thread::sleep(self.delay);
        self.inner.record(c)
    }
    fn scan(
        &self,
        f: &ml_core::RecordFilter,
        a: Option<&ml_core::ContextId>,
        l: usize,
    ) -> Result<Vec<ml_core::Record>, ml_core::StoreError> {
        self.inner.scan(f, a, l)
    }
    fn events_after(&self, a: u64, l: usize) -> Result<Vec<ml_core::Event>, ml_core::StoreError> {
        self.inner.events_after(a, l)
    }
    fn last_seq(&self) -> Result<u64, ml_core::StoreError> {
        self.inner.last_seq()
    }
}

#[tokio::test]
async fn load_past_the_permits_is_shed_at_once() {
    let (ledger, _) = ledger();
    let router = app_over(
        Slow {
            inner: ledger,
            delay: Duration::from_millis(300),
        },
        AppOptions {
            permits: 1,
            ..options()
        },
        Box::new(|| Ok(())),
    );
    let (first, second) = tokio::join!(get(&router, "/v1/contexts/ctx-a"), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        get(&router, "/v1/contexts/ctx-b").await
    });
    assert_eq!(first.status, StatusCode::NOT_FOUND);
    assert_eq!(
        second.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        second.body
    );
    assert_eq!(second.headers["retry-after"], "1");
    assert!(second.str("error").contains("permit"));
    // Once the slow call is done, the permit is back.
    let later = get(&router, "/v1/contexts/ctx-c").await;
    assert_eq!(later.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_call_that_outlives_the_timeout_is_503() {
    let (ledger, _) = ledger();
    let router = app_over(
        Slow {
            inner: ledger,
            delay: Duration::from_millis(300),
        },
        AppOptions {
            request_timeout: Duration::from_millis(50),
            ..options()
        },
        Box::new(|| Ok(())),
    );
    let reply = get(&router, "/v1/contexts/ctx-a").await;
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.body
    );
    assert!(reply.str("error").contains("read the context back"));
}
