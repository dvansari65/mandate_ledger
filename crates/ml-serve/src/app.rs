//! The service's state and its router.

use crate::auth::{self, ApiKeys};
use crate::backend::Backend;
use crate::{handlers, request_id};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post};
use http::HeaderValue;
use http::header::{CACHE_CONTROL, X_CONTENT_TYPE_OPTIONS};
use ml_adapters::NativeCartAdapter;
use ml_core::SigningKey;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::set_header::SetResponseHeaderLayer;

/// Whether the store can be reached right now.
pub type Readiness = Box<dyn Fn() -> Result<(), String> + Send + Sync>;

/// How long a readiness check may take, and how long its answer is reused.
const READY_TIMEOUT: Duration = Duration::from_secs(2);
const READY_CACHE: Duration = Duration::from_secs(1);

/// The limits a deployment chooses.
#[derive(Clone, Copy, Debug)]
pub struct AppOptions {
    /// Ledger calls in flight at once: size it to the database pool.
    pub permits: usize,
    /// How long one ledger call may take before the request is answered
    /// `503`.
    pub request_timeout: Duration,
    /// The largest request body accepted, in bytes.
    pub body_limit: usize,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            permits: 8,
            request_timeout: Duration::from_secs(30),
            body_limit: 256 * 1024,
        }
    }
}

/// `/readyz`, coalesced: one check in flight at a time, its answer shared
/// with everyone asking meanwhile and for a moment after. A stream of
/// probes, honest or not, costs one database round trip a second, and no
/// probe fails for lack of a turn.
struct Probe {
    check: Readiness,
    permit: Semaphore,
    last: Mutex<Option<(Instant, Result<(), String>)>>,
}

impl Probe {
    /// The last answer, if it is recent enough to stand.
    fn cached(&self) -> Option<Result<(), String>> {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|(at, _)| at.elapsed() < READY_CACHE)
            .map(|(_, answer)| answer.clone())
    }

    fn remember(&self, answer: &Result<(), String>) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((Instant::now(), answer.clone()));
    }
}

struct Inner {
    backend: Arc<dyn Backend>,
    adapter: NativeCartAdapter,
    keys: ApiKeys,
    evidence_key: Option<SigningKey>,
    permits: Arc<Semaphore>,
    probe: Probe,
    options: AppOptions,
}

/// Everything a handler needs, cheap to clone.
#[derive(Clone)]
pub struct App(Arc<Inner>);

impl App {
    /// Assemble the service over `backend`.
    ///
    /// `evidence_key` signs exported bundles as this host; without it they
    /// are exported bare. `readiness` is what `/readyz` asks.
    pub fn new(
        backend: impl Backend,
        adapter: NativeCartAdapter,
        keys: ApiKeys,
        readiness: Readiness,
        evidence_key: Option<SigningKey>,
        options: AppOptions,
    ) -> Self {
        Self(Arc::new(Inner {
            backend: Arc::new(backend),
            adapter,
            keys,
            evidence_key,
            permits: Arc::new(Semaphore::new(options.permits.max(1))),
            probe: Probe {
                check: readiness,
                permit: Semaphore::new(1),
                last: Mutex::new(None),
            },
            options,
        }))
    }

    pub(crate) fn backend(&self) -> Arc<dyn Backend> {
        Arc::clone(&self.0.backend)
    }

    pub(crate) fn adapter(&self) -> &NativeCartAdapter {
        &self.0.adapter
    }

    pub(crate) fn keys(&self) -> &ApiKeys {
        &self.0.keys
    }

    pub(crate) fn evidence_key(&self) -> Option<&SigningKey> {
        self.0.evidence_key.as_ref()
    }

    pub(crate) fn permits(&self) -> &Arc<Semaphore> {
        &self.0.permits
    }

    pub(crate) fn options(&self) -> &AppOptions {
        &self.0.options
    }

    /// Whether the store can be reached: the recent answer if there is
    /// one, else one check, shared with every probe that arrives while it
    /// runs.
    pub(crate) async fn readiness(&self) -> Result<(), String> {
        let probe = &self.0.probe;
        if let Some(answer) = probe.cached() {
            return answer;
        }
        let permit = match tokio::time::timeout(READY_TIMEOUT, probe.permit.acquire()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(_closed)) => return Err("the probe is closed".to_owned()),
            Err(_elapsed) => return Err("a check is still running".to_owned()),
        };
        // The check waited for may have answered meanwhile.
        if let Some(answer) = probe.cached() {
            drop(permit);
            return answer;
        }
        let app = self.clone();
        let check = tokio::task::spawn_blocking(move || (app.0.probe.check)());
        let answer = match tokio::time::timeout(READY_TIMEOUT, check).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(join)) => Err(format!("the check failed: {join}")),
            Err(_elapsed) => Err("the store did not answer in time".to_owned()),
        };
        probe.remember(&answer);
        drop(permit);
        answer
    }

    /// Let go of the service off the async thread.
    ///
    /// The last `App` to drop takes the store's connections with it, and a
    /// synchronous store blocks to close them — which, on a thread that is
    /// driving the runtime, is a panic. Drop the last one here instead.
    /// Any clone still held elsewhere makes this a no-op.
    pub async fn close(self) {
        let _ = tokio::task::spawn_blocking(move || drop(self)).await;
    }

    /// The whole service as a router: every route, every layer.
    pub fn router(&self) -> Router {
        let api = Router::new()
            .route("/v1/authorizations", post(handlers::authorize))
            .route("/v1/contexts", get(handlers::contexts))
            .route("/v1/contexts/{ctx}", get(handlers::context_status))
            .route("/v1/contexts/{ctx}/payment", post(handlers::payment))
            .route("/v1/contexts/{ctx}/settlement", post(handlers::settlement))
            .route(
                "/v1/contexts/{ctx}/compensation",
                post(handlers::compensation),
            )
            .route("/v1/contexts/{ctx}/delivery", post(handlers::delivery))
            .route("/v1/contexts/{ctx}/expiry", post(handlers::expiry))
            .route("/v1/contexts/{ctx}/evidence", get(handlers::evidence))
            .route(
                "/v1/mandates/{mandate}/revocation",
                post(handlers::revocation),
            )
            .route("/v1/events", get(handlers::events))
            // Inside the key check: a wrong method on a protected route must
            // not tell anyone without a key that the route exists.
            .method_not_allowed_fallback(handlers::method_not_allowed)
            .route_layer(middleware::from_fn_with_state(
                self.clone(),
                auth::require_key,
            ));
        Router::new()
            .route("/healthz", get(handlers::healthz))
            .route("/readyz", get(handlers::readyz))
            .route("/openapi.json", get(handlers::openapi))
            .merge(api)
            .fallback(handlers::not_found)
            .method_not_allowed_fallback(handlers::method_not_allowed)
            .layer(
                ServiceBuilder::new()
                    .layer(middleware::from_fn(request_id::assign))
                    .layer(SetResponseHeaderLayer::overriding(
                        CACHE_CONTROL,
                        HeaderValue::from_static("no-store"),
                    ))
                    .layer(SetResponseHeaderLayer::overriding(
                        X_CONTENT_TYPE_OPTIONS,
                        HeaderValue::from_static("nosniff"),
                    ))
                    .layer(CatchPanicLayer::custom(handlers::panicked))
                    .layer(DefaultBodyLimit::max(self.0.options.body_limit)),
            )
            .with_state(self.clone())
    }
}
