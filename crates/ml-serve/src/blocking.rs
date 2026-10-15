//! The bridge from async handlers to the synchronous engine.
//!
//! Every ledger call runs on a blocking thread holding one of a fixed
//! number of permits, sized to the database pool: a request that finds no
//! permit free is answered `503` at once rather than queued, so load past
//! the pool's capacity is shed instead of piled up. The permit travels
//! into the blocking task and is released when the work ends — not when
//! the request gives up on it — so a timed-out call still counts against
//! the pool for as long as it really runs.

use crate::app::App;
use crate::backend::Backend;
use crate::error::ApiError;

/// Run `work` against the engine, within the request timeout.
///
/// On timeout the work is not cancelled — a blocking thread cannot be —
/// and its result is dropped; the caller is told to read the context back
/// before retrying.
pub async fn decide<T>(
    app: &App,
    work: impl FnOnce(&dyn Backend) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError>
where
    T: Send + 'static,
{
    let permit = app
        .permits()
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Busy)?;
    let backend = app.backend();
    let task = tokio::task::spawn_blocking(move || {
        let _held = permit;
        work(backend.as_ref())
    });
    match tokio::time::timeout(app.options().request_timeout, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(join)) => Err(ApiError::Internal(format!("ledger task failed: {join}"))),
        Err(_elapsed) => Err(ApiError::Timeout),
    }
}
