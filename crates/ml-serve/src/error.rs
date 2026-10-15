//! What a request gets back when it does not get a token.

use crate::dto::{Problem, Refusal};
use axum::Json;
use axum::response::{IntoResponse, Response};
use http::header::{RETRY_AFTER, WWW_AUTHENTICATE};
use http::{HeaderValue, StatusCode};
use ml_core::{Denied, LedgerError, StoreError};

/// Every way a request ends other than with a token.
#[derive(Debug)]
pub enum ApiError {
    /// The ledger evaluated the step and refused it. `409`.
    Refused(Denied),
    /// No accepted API key. `401`.
    Unauthorized,
    /// The thing asked about does not exist. `404`.
    NotFound(&'static str),
    /// The request is not something the ledger can be asked: malformed
    /// JSON, an invalid id, a field out of bounds. `400`, or the status the
    /// JSON extractor chose.
    Rejected {
        /// The status to answer with.
        status: StatusCode,
        /// What was wrong. Never echoes the request.
        message: String,
    },
    /// Every permit is in use. `503`, right away.
    Busy,
    /// The ledger did not answer within the request timeout. The work may
    /// still complete and be recorded. `503`.
    Timeout,
    /// The store or the rail was unavailable; the step was not evaluated.
    /// Logged with its cause, answered without it. `503`.
    Undecided(String),
    /// The service itself failed. Logged with its cause. `500`.
    Internal(String),
}

impl ApiError {
    /// A `400` with this message.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::Rejected {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl From<LedgerError> for ApiError {
    fn from(err: LedgerError) -> Self {
        match err {
            LedgerError::Denied(denied) => Self::Refused(denied),
            LedgerError::Store(e) => Self::Undecided(e.to_string()),
            LedgerError::RailUnavailable(e) => Self::Undecided(format!("rail unavailable: {e}")),
            LedgerError::Canonicalize(e) => Self::Internal(e.to_string()),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(err: StoreError) -> Self {
        Self::Undecided(err.to_string())
    }
}

fn problem(status: StatusCode, error: impl Into<String>) -> Response {
    (status, Json(Problem::new(error))).into_response()
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Refused(denied) => {
                tracing::info!(
                    ctx = %denied.ctx,
                    stage = %denied.stage,
                    code = denied.reason.code(),
                    recorded = denied.recorded,
                    "refused"
                );
                (StatusCode::CONFLICT, Json(Refusal::from(denied))).into_response()
            }
            Self::Unauthorized => {
                let mut response = problem(StatusCode::UNAUTHORIZED, "missing or invalid API key");
                response
                    .headers_mut()
                    .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
                response
            }
            Self::NotFound(what) => problem(StatusCode::NOT_FOUND, what),
            Self::Rejected { status, message } => problem(status, message),
            Self::Busy => retry_later("every permit is in use; retry shortly"),
            Self::Timeout => {
                tracing::warn!("the ledger did not answer within the request timeout");
                retry_later("the ledger did not answer in time; read the context back, then retry")
            }
            Self::Undecided(cause) => {
                tracing::error!(%cause, "could not decide");
                retry_later("the ledger could not decide; read the context back, then retry")
            }
            Self::Internal(cause) => {
                tracing::error!(%cause, "internal error");
                problem(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        }
    }
}

/// A `503` the client should retry.
fn retry_later(error: &str) -> Response {
    let mut response = problem(StatusCode::SERVICE_UNAVAILABLE, error);
    response
        .headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from_static("1"));
    response
}
