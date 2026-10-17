//! Assembling the service from a [`Config`], and running it until told to
//! stop.
//!
//! The store is synchronous and its connections block to close, so the
//! process has to let go of it outside the runtime: `main` builds the
//! service with [`assemble`], runs [`serve`], shuts the runtime down with a
//! deadline, and only then drops the [`App`]. A host that lives inside a
//! runtime uses [`App::close`] instead.

use crate::app::{App, AppOptions, Readiness};
use crate::config::Config;
use ml_adapters::NativeCartAdapter;
use ml_adapters::sandbox::{MemoryChecks, SandboxRail};
use ml_core::{Ledger, SystemClock, TrustedSigners};
use ml_store_postgres::{PgPool, PostgresStore};
use postgres::NoTls;
use r2d2_postgres::PostgresConnectionManager;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::watch;

/// How long to keep trying to reach the database, at boot and at every
/// checkout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How many `…-reorg` references the sandbox rail remembers.
const REORG_MEMORY: usize = 4096;

/// Why the service stopped before it meant to.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database could not be reached or prepared.
    #[error("database: {0}")]
    Database(String),
    /// The listen address could not be bound.
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        /// The address.
        addr: std::net::SocketAddr,
        /// Why.
        source: std::io::Error,
    },
    /// Serving failed.
    #[error("serve: {0}")]
    Serve(std::io::Error),
    /// A startup task panicked.
    #[error("startup: {0}")]
    Startup(String),
}

/// Keeps the last connection error, which r2d2 otherwise reduces to
/// "timed out waiting for connection".
#[derive(Debug)]
struct LastError(Arc<Mutex<Option<String>>>);

impl r2d2::HandleError<postgres::Error> for LastError {
    fn handle_error(&self, error: postgres::Error) {
        let mut message = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(message);
    }
}

/// A pool of at most `size` connections to `database`, keeping `idle` of
/// them open (all of them, when `None`).
fn pool(database: postgres::Config, size: u32, idle: Option<u32>) -> Result<PgPool, Error> {
    let last_error = Arc::new(Mutex::new(None));
    r2d2::Pool::builder()
        .max_size(size)
        .min_idle(idle)
        .connection_timeout(CONNECT_TIMEOUT)
        .error_handler(Box::new(LastError(Arc::clone(&last_error))))
        .build(PostgresConnectionManager::new(database, NoTls))
        .map_err(|e| {
            let cause = last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .unwrap_or_else(|| e.to_string());
            Error::Database(format!("cannot connect: {cause}"))
        })
}

/// The ledger's store over a pool of `pool_size`, its schema brought up to
/// date, and a connection of its own for the readiness probe.
///
/// Every statement and every lock wait is bounded by `request_timeout` on
/// the server side, so a wedged session cannot hold a permit — or a
/// blocking thread — for longer than a request may take.
fn connect(
    mut database: postgres::Config,
    pool_size: u32,
    request_timeout: Duration,
) -> Result<(PostgresStore, PgPool), Error> {
    let ms = request_timeout.as_millis();
    database.options(&format!("-c statement_timeout={ms} -c lock_timeout={ms}"));
    let store = PostgresStore::from_pool(pool(database.clone(), pool_size, None)?);
    store
        .migrate()
        .map_err(|e| Error::Database(format!("cannot prepare the schema: {e}")))?;
    // Opened on demand, so an idle probe costs nothing; separate, so the
    // probe neither competes with the ledger for a connection nor is
    // starved by it.
    let probe = pool(database, 1, Some(0))?;
    Ok((store, probe))
}

/// `/readyz`: one round trip to the database.
fn readiness(probe: PgPool) -> Readiness {
    Box::new(move || {
        let mut conn = probe.get().map_err(|e| e.to_string())?;
        conn.simple_query("SELECT 1").map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// Resolves once the process is asked to stop: SIGTERM or Ctrl-C.
async fn stop_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(term) => term,
                Err(e) => {
                    tracing::warn!(error = %e, "cannot listen for SIGTERM; Ctrl-C only");
                    let _ = ctrl_c.await;
                    return;
                }
            };
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}

/// Connect, prepare the schema, and build the service as `config` says.
///
/// The returned [`App`] owns the store. Drop it off the async thread —
/// after the runtime is gone, or through [`App::close`].
pub async fn assemble(config: &Config) -> Result<App, Error> {
    let database = config.database.expose().clone();
    let (pool_size, request_timeout) = (config.db_pool, config.request_timeout);
    let (store, probe) =
        tokio::task::spawn_blocking(move || connect(database, pool_size, request_timeout))
            .await
            .map_err(|e| Error::Startup(e.to_string()))??;

    let rail = SandboxRail::new(
        config.rail_id.clone(),
        config.rail_finality,
        MemoryChecks::new(REORG_MEMORY),
    );
    let signers = config
        .signers
        .iter()
        .fold(TrustedSigners::new(), |signers, (principal, key)| {
            signers.allow(principal.clone(), key.to_bytes())
        });
    if config.signers.is_empty() {
        tracing::warn!("ML_TRUSTED_SIGNERS is empty: every mandate will be refused as untrusted");
    }
    let adapter = config
        .merchant_keys
        .iter()
        .fold(NativeCartAdapter::new(), |adapter, (id, key)| {
            adapter.with_merchant_key(id.clone(), *key)
        });
    let ledger = Ledger::new(store, rail, signers, SystemClock);
    Ok(App::new(
        ledger,
        adapter,
        config.api_keys.clone(),
        readiness(probe),
        config.evidence_key.as_ref().map(|k| k.expose().clone()),
        AppOptions {
            permits: usize::try_from(config.db_pool).unwrap_or(8),
            request_timeout: config.request_timeout,
            body_limit: config.body_limit,
        },
    ))
}

/// Bind, serve until the stop signal, and give in-flight requests the
/// configured grace.
pub async fn serve(config: &Config, app: &App) -> Result<(), Error> {
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|source| Error::Bind {
            addr: config.listen,
            source,
        })?;
    tracing::info!(
        addr = %config.listen,
        rail = %config.rail_id,
        pool = config.db_pool,
        signers = config.signers.len(),
        merchant_keys = config.merchant_keys.len(),
        signs_evidence = config.evidence_key.is_some(),
        "listening"
    );

    let (stop, stopped) = watch::channel(false);
    tokio::spawn(async move {
        stop_signal().await;
        tracing::info!("stop signal received; finishing in-flight requests");
        let _ = stop.send(true);
    });
    let graceful = {
        let mut stopped = stopped.clone();
        async move {
            let _ = stopped.wait_for(|stop| *stop).await;
        }
    };
    let deadline = {
        let mut stopped = stopped;
        let grace = config.shutdown_grace;
        async move {
            let _ = stopped.wait_for(|stop| *stop).await;
            tokio::time::sleep(grace).await;
        }
    };

    tokio::select! {
        served = axum::serve(listener, app.router()).with_graceful_shutdown(graceful) => {
            served.map_err(Error::Serve)?;
            tracing::info!("stopped");
        }
        () = deadline => {
            tracing::warn!("grace period over with requests still in flight; exiting");
        }
    }
    Ok(())
}
