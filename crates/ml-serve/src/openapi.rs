//! The OpenAPI document, generated from the handlers and the wire types so
//! that it cannot drift from them. The committed `openapi.json` is checked
//! against it by a test.

use crate::dto::{
    AttestationBody, AuthorizeRequest, AuthorizedResponse, CartBody, CompensatedResponse,
    CompensationRequest, ContextResponse, ContextsResponse, DeliveredResponse, DeliveryRequest,
    EventsResponse, ExpiredResponse, FinalityBody, Health, MoneyBody, PaidResponse, PaymentRequest,
    Problem, ProofBody, ReceiptBody, Refusal, RevokedResponse, SettlementRequest,
    SettlementResponse, StepState,
};
use crate::handlers;
use std::sync::OnceLock;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mandate-ledger",
        description = "The five calls over HTTP. `200`: allowed. `409`: refused, with the code. \
                       `503`: could not decide — read the context back, then retry. \
                       Every call is safe to repeat: a retry is the same purchase, and gets \
                       the same answer.",
        license(name = "Apache-2.0"),
    ),
    paths(
        handlers::authorize,
        handlers::payment,
        handlers::settlement,
        handlers::compensation,
        handlers::delivery,
        handlers::expiry,
        handlers::revocation,
        handlers::context_status,
        handlers::contexts,
        handlers::events,
        handlers::evidence,
        handlers::healthz,
        handlers::readyz,
    ),
    components(schemas(
        MoneyBody,
        AttestationBody,
        CartBody,
        AuthorizeRequest,
        ProofBody,
        PaymentRequest,
        FinalityBody,
        SettlementRequest,
        CompensationRequest,
        ReceiptBody,
        DeliveryRequest,
        StepState,
        AuthorizedResponse,
        PaidResponse,
        SettlementResponse,
        CompensatedResponse,
        DeliveredResponse,
        ExpiredResponse,
        RevokedResponse,
        ContextResponse,
        ContextsResponse,
        EventsResponse,
        Refusal,
        Problem,
        Health,
    )),
    modifiers(&Security),
    tags(
        (name = "lifecycle", description = "The five calls, plus expiry and revocation."),
        (name = "read", description = "Where contexts stand, and what happened, with cursors."),
        (name = "ops", description = "Liveness and readiness. No key needed."),
    )
)]
struct ApiDoc;

/// The one way in: a bearer API key.
struct Security;

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "api_key",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("An API key from ML_API_KEYS"))
                    .build(),
            ),
        );
    }
}

/// The document as pretty JSON, built once.
///
/// # Panics
///
/// If the document cannot be serialized — which the OpenAPI test catches
/// before any build ships.
pub fn spec() -> &'static str {
    static SPEC: OnceLock<String> = OnceLock::new();
    SPEC.get_or_init(|| {
        ApiDoc::openapi()
            .to_pretty_json()
            .expect("the document serializes")
    })
}
