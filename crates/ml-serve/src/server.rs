//! Assembling the service from a [`Config`] and running it until told to
//! stop.

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

/// How long to keep trying to reach the database at boot.
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

/// Connect, size the pool, and bring the schema up to date.
fn connect(url: &str, pool_size: u32) -> Result<PostgresStore, Error> {
    let config: postgres::Config = url
        .parse()
        .map_err(|e| Error::Database(format!("bad URL: {e}")))?;
    let last_error = Arc::new(Mutex::new(None));
    let pool = r2d2::Pool::builder()
        .max_size(pool_size)
        .connection_timeout(CONNECT_TIMEOUT)
        .error_handler(Box::new(LastError(Arc::clone(&last_error))))
        .build(PostgresConnectionManager::new(config, NoTls))
        .map_err(|e| {
            let cause = last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .unwrap_or_else(|| e.to_string());
            Error::Database(format!("cannot connect: {cause}"))
        })?;
    let store = PostgresStore::from_pool(pool);
    store
        .migrate()
        .map_err(|e| Error::Database(format!("cannot prepare the schema: {e}")))?;
    Ok(store)
}

/// `/readyz`: one round trip to the database.
fn readiness(pool: PgPool) -> Readiness {
    Box::new(move || {
        let mut conn = pool.get().map_err(|e| e.to_string())?;
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

/// Run the service until a stop signal, then give in-flight requests the
/// configured grace and exit.
pub async fn run(config: Config) -> Result<(), Error> {
    let url = config.database_url.expose().clone();
    let pool_size = config.db_pool;
    let store = tokio::task::spawn_blocking(move || connect(&url, pool_size))
        .await
        .map_err(|e| Error::Startup(e.to_string()))??;
    let app = assemble(&config, store);
    // Whatever happens from here on, the store is let go of off the async
    // thread: its connections block to close.
    let served = serve(&config, &app).await;
    app.close().await;
    served
}

/// The service over `store`, as the configuration says.
fn assemble(config: &Config, store: PostgresStore) -> App {
    let ready = readiness(store.pool().clone());

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
    App::new(
        ledger,
        adapter,
        config.api_keys.clone(),
        ready,
        config.evidence_key.as_ref().map(|k| k.expose().clone()),
        AppOptions {
            permits: usize::try_from(config.db_pool).unwrap_or(8),
            request_timeout: config.request_timeout,
            body_limit: config.body_limit,
        },
    )
}

/// Bind, serve until the stop signal, and give in-flight requests the
/// configured grace.
async fn serve(config: &Config, app: &App) -> Result<(), Error> {
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
