//! A request id of the service's own making, on every response and every
//! log line about the request. A client-supplied id is ignored: an id in
//! the logs must be one nobody else chose.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use http::HeaderValue;
use http::header::HeaderName;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::Instrument as _;

static HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Unique for the life of the process: a random prefix drawn once, and a
/// counter. Random so that two processes' ids never read alike; counted so
/// that no request pays for randomness.
fn next_id() -> String {
    static PREFIX: OnceLock<u64> = OnceLock::new();
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let prefix = PREFIX.get_or_init(|| {
        let mut bytes = [0u8; 8];
        if getrandom::fill(&mut bytes).is_err() {
            // No randomness from the OS: the time is unique enough for a
            // log correlator, and nothing secret rides on it.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            bytes.copy_from_slice(&nanos.to_le_bytes()[..8]);
        }
        u64::from_le_bytes(bytes)
    });
    format!(
        "{prefix:016x}{:08x}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Assign the id, run the request inside a span that carries it, log the
/// outcome, and put the id on the response.
pub async fn assign(request: Request, next: Next) -> Response {
    let id = next_id();
    let span = tracing::info_span!(
        "request",
        id = %id,
        method = %request.method(),
        path = %request.uri().path(),
        key = tracing::field::Empty,
    );
    let started = Instant::now();
    let mut response = next.run(request).instrument(span.clone()).await;
    let _entered = span.enter();
    tracing::info!(
        status = response.status().as_u16(),
        latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "handled"
    );
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(HEADER.clone(), value);
    }
    response
}
