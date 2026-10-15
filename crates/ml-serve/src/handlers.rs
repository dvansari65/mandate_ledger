//! One handler per route. Each validates what it can without the ledger,
//! then asks the ledger exactly one thing, on a blocking thread.

use crate::app::App;
use crate::blocking::decide;
use crate::dto::{
    AuthorizeRequest, AuthorizedResponse, CompensatedResponse, CompensationRequest,
    ContextResponse, ContextsQuery, ContextsResponse, DeliveredResponse, DeliveryRequest,
    EventsQuery, EventsResponse, ExpiredResponse, Health, PaidResponse, PaymentRequest, Problem,
    Refusal, RevokedResponse, SettlementRequest, SettlementResponse, StepState,
};
use crate::error::ApiError;
use axum::Json;
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request, State};
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use http::header::CONTENT_TYPE;
use http::request::Parts;
use ml_adapters::sandbox::Finality;
use ml_core::{
    ContextId, Denied, DenyReason, MandateId, PaymentState, RecordFilter, Settlement, Stage,
};
use std::any::Any;
use std::time::Duration;

/// The page size when none is asked for, and the most that may be.
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

/// How long readiness waits for the store.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// A JSON body, with the extractor's rejection in the service's own shape.
pub struct Body<T>(pub T);

impl<T, S> FromRequest<S> for Body<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::Rejected {
                status: rejection.status(),
                message: rejection.body_text(),
            }),
        }
    }
}

/// A query string, with the extractor's rejection in the service's own shape.
pub struct Params<T>(pub T);

impl<T, S> FromRequestParts<S> for Params<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::Rejected {
                status: rejection.status(),
                message: rejection.body_text(),
            }),
        }
    }
}

type Reply<T> = Result<Json<T>, ApiError>;

fn context(ctx: &str) -> Result<ContextId, ApiError> {
    ContextId::new(ctx).map_err(|e| ApiError::bad_request(format!("ctx: {e}")))
}

fn mandate_id(id: &str) -> Result<MandateId, ApiError> {
    MandateId::new(id).map_err(|e| ApiError::bad_request(format!("mandate: {e}")))
}

/// A step the engine cannot even be asked about: the context does not
/// exist, or never earned the token the step needs. Refused with the
/// engine's own code, and marked as not recorded, because the type system
/// said no before the ledger could.
fn unreachable(
    ctx: &ContextId,
    stage: Stage,
    reason: DenyReason,
    detail: impl Into<String>,
) -> ApiError {
    ApiError::Refused(Denied {
        ctx: ctx.clone(),
        stage,
        reason,
        detail: detail.into(),
        recorded: false,
    })
}

fn no_such_context(ctx: &ContextId, stage: Stage) -> ApiError {
    unreachable(ctx, stage, DenyReason::ContextNotFound, "no such context")
}

/// The engine's own spelling of a state: its wire form.
fn state_name(state: PaymentState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn page_limit(limit: Option<usize>) -> Result<usize, ApiError> {
    match limit {
        None => Ok(DEFAULT_LIMIT),
        Some(n) if n <= MAX_LIMIT => Ok(n),
        Some(_) => Err(ApiError::bad_request(format!("limit: at most {MAX_LIMIT}"))),
    }
}

/// Check a cart against a mandate and reserve its total.
#[utoipa::path(
    post, path = "/v1/authorizations", tag = "lifecycle",
    request_body = AuthorizeRequest,
    responses(
        (status = 200, description = "Authorized; the context id is the handle for every later step", body = AuthorizedResponse),
        (status = 409, description = "Refused", body = Refusal),
        (status = 400, description = "Not a request the ledger can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "Could not decide; read the context back, then retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn authorize(
    State(app): State<App>,
    Body(request): Body<AuthorizeRequest>,
) -> Reply<AuthorizedResponse> {
    request.validate()?;
    let mandate = serde_json::from_value(request.mandate)
        .map_err(|e| ApiError::bad_request(format!("mandate: {e}")))?;
    let cart = app
        .adapter()
        .normalize_cart(&request.cart.into_native()?)
        .map_err(|e| {
            // A cart the adapter rejects never reached the ledger, so there
            // is no context to refuse it under; but a bad merchant
            // signature is worth a line in the log.
            tracing::warn!(error = %e, "cart rejected");
            ApiError::bad_request(format!("cart: {e}"))
        })?;
    let request_key = request.request_key;
    let auth = decide(&app, move |ledger| {
        Ok(ledger.authorize(&mandate, &cart, &request_key)?)
    })
    .await?;
    Ok(Json(AuthorizedResponse::from(&auth)))
}

/// Record a payment proof against an authorized context.
#[utoipa::path(
    post, path = "/v1/contexts/{ctx}/payment", tag = "lifecycle",
    params(("ctx" = String, Path, description = "The context, from authorize")),
    request_body = PaymentRequest,
    responses(
        (status = 200, description = "Paid", body = PaidResponse),
        (status = 409, description = "Refused", body = Refusal),
        (status = 400, description = "Not a request the ledger can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "Could not decide; read the context back, then retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn payment(
    State(app): State<App>,
    Path(ctx): Path<String>,
    Body(request): Body<PaymentRequest>,
) -> Reply<PaidResponse> {
    let ctx = context(&ctx)?;
    let proof = request.proof.into_proof(&ctx)?;
    let paid = decide(&app, move |ledger| {
        let reached = ledger
            .reached(&ctx)?
            .ok_or_else(|| no_such_context(&ctx, Stage::Payment))?;
        // Always through the `Authorized` token: on a context that is
        // already paid, the engine decides whether this is the same payment
        // again or a different one, and records the refusal if it is.
        Ok(ledger.record_payment(&reached.authorized, &proof)?)
    })
    .await?;
    Ok(Json(PaidResponse::from(&paid)))
}

/// Ask the rail whether a recorded payment is final.
#[utoipa::path(
    post, path = "/v1/contexts/{ctx}/settlement", tag = "lifecycle",
    params(("ctx" = String, Path, description = "The context, from authorize")),
    request_body = SettlementRequest,
    responses(
        (status = 200, description = "Settled, pending, or failed — each a decision", body = SettlementResponse),
        (status = 409, description = "Refused", body = Refusal),
        (status = 400, description = "Not a request the ledger can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "Could not decide; read the context back, then retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn settlement(
    State(app): State<App>,
    Path(ctx): Path<String>,
    Body(request): Body<SettlementRequest>,
) -> Reply<SettlementResponse> {
    let ctx = context(&ctx)?;
    let (reference, reported) = request.finality.validate()?;
    let response = decide(&app, move |ledger| {
        let reached = ledger
            .reached(&ctx)?
            .ok_or_else(|| no_such_context(&ctx, Stage::Settlement))?;
        let paid = reached.paid.ok_or_else(|| {
            unreachable(
                &ctx,
                Stage::Settlement,
                DenyReason::InvalidState,
                format!(
                    "the context is {}; it was never paid",
                    state_name(reached.state)
                ),
            )
        })?;
        let finality = Finality {
            reference: reference.unwrap_or_else(|| paid.reference().to_owned()),
            reported,
        };
        let ctx = ctx.as_str().to_owned();
        Ok(match ledger.record_settlement(&paid, &finality)? {
            Settlement::Settled(settled) => SettlementResponse {
                ctx,
                state: StepState::Settled,
                reference: Some(settled.reference().to_owned()),
                reason: None,
            },
            Settlement::Pending { reason } => SettlementResponse {
                ctx,
                state: StepState::Pending,
                reference: None,
                reason: Some(reason),
            },
            Settlement::Failed(failed) => SettlementResponse {
                ctx,
                state: StepState::SettlementFailed,
                reference: None,
                reason: Some(failed.reason().to_owned()),
            },
        })
    })
    .await?;
    Ok(Json(response))
}

/// Record that the host undid its side effects after a failed settlement.
#[utoipa::path(
    post, path = "/v1/contexts/{ctx}/compensation", tag = "lifecycle",
    params(("ctx" = String, Path, description = "The context, from authorize")),
    request_body = CompensationRequest,
    responses(
        (status = 200, description = "Compensated", body = CompensatedResponse),
        (status = 409, description = "Refused", body = Refusal),
        (status = 400, description = "Not a request the ledger can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "Could not decide; read the context back, then retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn compensation(
    State(app): State<App>,
    Path(ctx): Path<String>,
    Body(request): Body<CompensationRequest>,
) -> Reply<CompensatedResponse> {
    let ctx = context(&ctx)?;
    request.validate()?;
    let compensated = decide(&app, move |ledger| {
        let reached = ledger
            .reached(&ctx)?
            .ok_or_else(|| no_such_context(&ctx, Stage::Compensation))?;
        let failed = reached.settlement_failed.ok_or_else(|| {
            unreachable(
                &ctx,
                Stage::Compensation,
                DenyReason::InvalidState,
                format!(
                    "the context is {}; compensation needs settlement_failed",
                    state_name(reached.state)
                ),
            )
        })?;
        Ok(ledger.compensate(&failed, request.reference.as_deref())?)
    })
    .await?;
    Ok(Json(CompensatedResponse {
        ctx: compensated.ctx().as_str().to_owned(),
        state: StepState::Compensated,
    }))
}

/// Record fulfilment of a settled context.
#[utoipa::path(
    post, path = "/v1/contexts/{ctx}/delivery", tag = "lifecycle",
    params(("ctx" = String, Path, description = "The context, from authorize")),
    request_body = DeliveryRequest,
    responses(
        (status = 200, description = "Delivered", body = DeliveredResponse),
        (status = 409, description = "Refused", body = Refusal),
        (status = 400, description = "Not a request the ledger can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "Could not decide; read the context back, then retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn delivery(
    State(app): State<App>,
    Path(ctx): Path<String>,
    Body(request): Body<DeliveryRequest>,
) -> Reply<DeliveredResponse> {
    let ctx = context(&ctx)?;
    let receipt = request.receipt.into_receipt()?;
    let delivered = decide(&app, move |ledger| {
        let reached = ledger
            .reached(&ctx)?
            .ok_or_else(|| no_such_context(&ctx, Stage::Delivery))?;
        let settled = reached.settled.ok_or_else(|| {
            unreachable(
                &ctx,
                Stage::Delivery,
                DenyReason::InvalidState,
                format!(
                    "the context is {}; delivery needs settled",
                    state_name(reached.state)
                ),
            )
        })?;
        Ok(ledger.record_delivery(&settled, receipt)?)
    })
    .await?;
    Ok(Json(DeliveredResponse::from(&delivered)))
}

/// Release an authorization that will not be paid.
#[utoipa::path(
    post, path = "/v1/contexts/{ctx}/expiry", tag = "lifecycle",
    params(("ctx" = String, Path, description = "The context, from authorize")),
    responses(
        (status = 200, description = "Expired; the reservation is released", body = ExpiredResponse),
        (status = 409, description = "Refused", body = Refusal),
        (status = 400, description = "Not a request the ledger can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "Could not decide; read the context back, then retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn expiry(State(app): State<App>, Path(ctx): Path<String>) -> Reply<ExpiredResponse> {
    let ctx = context(&ctx)?;
    let ctx = decide(&app, move |ledger| {
        let reached = ledger
            .reached(&ctx)?
            .ok_or_else(|| no_such_context(&ctx, Stage::Expiry))?;
        ledger.expire(&reached.authorized)?;
        Ok(ctx)
    })
    .await?;
    Ok(Json(ExpiredResponse {
        ctx: ctx.as_str().to_owned(),
        state: StepState::Expired,
    }))
}

/// Withdraw a mandate: every later authorization under it is refused.
/// Contexts already authorized are unaffected.
#[utoipa::path(
    post, path = "/v1/mandates/{mandate}/revocation", tag = "lifecycle",
    params(("mandate" = String, Path, description = "The mandate id")),
    responses(
        (status = 200, description = "Revoked", body = RevokedResponse),
        (status = 400, description = "Not a request the ledger can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "Could not decide; retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn revocation(
    State(app): State<App>,
    Path(mandate): Path<String>,
) -> Reply<RevokedResponse> {
    let mandate = mandate_id(&mandate)?;
    let mandate = decide(&app, move |ledger| {
        ledger.revoke(&mandate)?;
        Ok(mandate)
    })
    .await?;
    Ok(Json(RevokedResponse {
        mandate: mandate.as_str().to_owned(),
        revoked: true,
    }))
}

/// Where a context stands now.
#[utoipa::path(
    get, path = "/v1/contexts/{ctx}", tag = "read",
    params(("ctx" = String, Path, description = "The context, from authorize")),
    responses(
        (status = 200, description = "The context", body = ContextResponse),
        (status = 404, description = "No such context", body = Problem),
        (status = 400, description = "Not a context id", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "The store was unavailable; retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn context_status(
    State(app): State<App>,
    Path(ctx): Path<String>,
) -> Reply<ContextResponse> {
    let ctx = context(&ctx)?;
    let record = decide(&app, move |ledger| Ok(ledger.record(&ctx)?))
        .await?
        .ok_or(ApiError::NotFound("no such context"))?;
    Ok(Json(ContextResponse::from(record)))
}

/// Where every context stands, a page at a time.
#[utoipa::path(
    get, path = "/v1/contexts", tag = "read",
    params(ContextsQuery),
    responses(
        (status = 200, description = "A page of contexts", body = ContextsResponse),
        (status = 400, description = "Not a query the store can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "The store was unavailable; retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn contexts(
    State(app): State<App>,
    Params(query): Params<ContextsQuery>,
) -> Reply<ContextsResponse> {
    let filter = RecordFilter {
        mandate: query.mandate.as_deref().map(mandate_id).transpose()?,
        state: query
            .state
            .as_deref()
            .map(|s| {
                serde_json::from_value(serde_json::Value::String(s.to_owned())).map_err(|_| {
                    ApiError::bad_request(format!(
                        "state: unknown state `{s}`; one of authorized, paid, settled, \
                         settlement_failed, compensated, delivered, expired"
                    ))
                })
            })
            .transpose()?,
    };
    let after = query
        .after
        .as_deref()
        .map(|s| ContextId::new(s).map_err(|e| ApiError::bad_request(format!("after: {e}"))))
        .transpose()?;
    let limit = page_limit(query.limit)?;
    let records = decide(&app, move |ledger| {
        Ok(ledger.scan(&filter, after.as_ref(), limit)?)
    })
    .await?;
    let next = records
        .last()
        .map(|r| r.ctx.as_str().to_owned())
        .or(query.after);
    Ok(Json(ContextsResponse {
        contexts: records.into_iter().map(ContextResponse::from).collect(),
        next,
    }))
}

/// What happened: every event in every context, in commit order,
/// refusals included.
#[utoipa::path(
    get, path = "/v1/events", tag = "read",
    params(EventsQuery),
    responses(
        (status = 200, description = "A page of the log", body = EventsResponse),
        (status = 400, description = "Not a query the store can be asked", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "The store was unavailable; retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn events(
    State(app): State<App>,
    Params(query): Params<EventsQuery>,
) -> Reply<EventsResponse> {
    let limit = page_limit(query.limit)?;
    let response = decide(&app, move |ledger| {
        let after = match query.after {
            Some(after) => after,
            // The tail. Gaps (rolled-back appends) make this "up to N".
            None => ledger
                .last_seq()?
                .saturating_sub(u64::try_from(limit).unwrap_or(u64::MAX)),
        };
        let events = ledger.events_after(after, limit)?;
        let next = events.last().map_or(after, |e| e.seq);
        Ok(EventsResponse { events, next })
    })
    .await?;
    Ok(Json(response))
}

/// A context's chain as a bundle anyone can verify without this service:
/// signed by the service's evidence key when it has one, bare otherwise.
/// Either form is what `ml verify` reads.
#[utoipa::path(
    get, path = "/v1/contexts/{ctx}/evidence", tag = "read",
    params(("ctx" = String, Path, description = "The context, from authorize")),
    responses(
        (status = 200, description = "The bundle, or the signed bundle", body = Object),
        (status = 404, description = "No such context", body = Problem),
        (status = 400, description = "Not a context id", body = Problem),
        (status = 401, description = "No valid API key", body = Problem),
        (status = 503, description = "The store was unavailable; retry", body = Problem),
    ),
    security(("api_key" = []))
)]
pub async fn evidence(State(app): State<App>, Path(ctx): Path<String>) -> Reply<serde_json::Value> {
    let ctx = context(&ctx)?;
    let bundle = decide(&app, move |ledger| Ok(ledger.evidence(&ctx)?))
        .await?
        .ok_or(ApiError::NotFound("no such context"))?;
    let value = match app.evidence_key() {
        Some(key) => {
            let signed = bundle
                .sign(key)
                .map_err(|e| ApiError::Internal(format!("cannot sign evidence: {e}")))?;
            serde_json::to_value(signed)
        }
        None => serde_json::to_value(bundle),
    }
    .map_err(|e| ApiError::Internal(format!("cannot serialize evidence: {e}")))?;
    Ok(Json(value))
}

/// The process is up.
#[utoipa::path(
    get, path = "/healthz", tag = "ops",
    responses((status = 200, description = "Up", body = Health))
)]
pub async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok".to_owned(),
    })
}

/// The process can reach its store. One check runs at a time; a second
/// caller while one is in flight is told to come back, so that nobody can
/// drain the pool through this door.
#[utoipa::path(
    get, path = "/readyz", tag = "ops",
    responses(
        (status = 200, description = "Ready", body = Health),
        (status = 503, description = "Not ready", body = Health),
    )
)]
pub async fn readyz(State(app): State<App>) -> Response {
    let not_ready = || {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(Health {
                status: "not ready".to_owned(),
            }),
        )
            .into_response()
    };
    let Ok(permit) = app.ready_permit().clone().try_acquire_owned() else {
        return not_ready();
    };
    let checked = app.clone();
    let check = tokio::task::spawn_blocking(move || {
        let _held = permit;
        checked.ready()
    });
    match tokio::time::timeout(READY_TIMEOUT, check).await {
        Ok(Ok(Ok(()))) => Json(Health {
            status: "ok".to_owned(),
        })
        .into_response(),
        Ok(Ok(Err(cause))) => {
            tracing::warn!(%cause, "not ready");
            not_ready()
        }
        Ok(Err(join)) => {
            tracing::warn!(cause = %join, "not ready");
            not_ready()
        }
        Err(_elapsed) => {
            tracing::warn!("not ready: the store did not answer in time");
            not_ready()
        }
    }
}

/// The OpenAPI document.
pub async fn openapi() -> Response {
    ([(CONTENT_TYPE, "application/json")], crate::openapi::spec()).into_response()
}

/// Nothing is served here.
pub async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(Problem::new("no such route"))).into_response()
}

/// The route exists; the method does not.
pub async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(Problem::new("method not allowed on this route")),
    )
        .into_response()
}

/// A handler panicked. The cause goes to the log, not to the caller.
// The signature is `CatchPanicLayer`'s.
#[allow(clippy::needless_pass_by_value)]
pub fn panicked(cause: Box<dyn Any + Send + 'static>) -> Response {
    let cause = cause
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| cause.downcast_ref::<&str>().copied())
        .unwrap_or("unknown");
    tracing::error!(cause, "handler panicked");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(Problem::new("internal error")),
    )
        .into_response()
}
