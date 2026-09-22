//! Assembling the engine for one command: the store, the rail, the clock,
//! and the operator's trust configuration.
//!
//! Every command is one process, so all of this is built from scratch each
//! time. That is cheap — one connection and a version check on the schema —
//! and it is the point: nothing survives between two commands except what
//! the database holds.

use crate::report::Report;
use crate::{Failure, keys};
use clap::Args;
use ml_adapters::{MockRail, NativeCartAdapter};
use ml_core::{Denied, Ledger, LedgerError, PrincipalId, SystemClock, TrustedSigners};
use ml_store_postgres::PostgresStore;
use postgres::{Config, NoTls};
use r2d2_postgres::PostgresConnectionManager;
use std::path::PathBuf;

/// The engine as the sandbox assembles it: PostgreSQL, the mock rail, wall
/// time, and whichever signers the operator named.
pub type Engine = Ledger<PostgresStore, MockRail, TrustedSigners, SystemClock>;

/// Where the ledger lives.
#[derive(Args)]
pub struct DbArgs {
    #[arg(
        long,
        env = "ML_DATABASE_URL",
        value_name = "URL",
        hide_env_values = true,
        help = "PostgreSQL URL; read from ML_DATABASE_URL when not given"
    )]
    database_url: Option<String>,
}

/// Whose keys the operator trusts. Without any, every mandate is refused as
/// untrusted and every signed cart is rejected — as in production, where a
/// mandate cannot vouch for its own key.
#[derive(Args)]
pub struct TrustArgs {
    /// Trust the key in KEYFILE to sign mandates for PRINCIPAL. Repeatable.
    #[arg(long = "trust", value_name = "PRINCIPAL=KEYFILE", value_parser = pair)]
    trust: Vec<(String, PathBuf)>,

    /// Accept carts signed by the key in KEYFILE under merchant key id ID.
    /// Repeatable.
    #[arg(long = "merchant-key", value_name = "ID=KEYFILE", value_parser = pair)]
    merchant_keys: Vec<(String, PathBuf)>,
}

/// `NAME=FILE`, split at the last `=` so the name may contain one.
fn pair(s: &str) -> Result<(String, PathBuf), String> {
    match s.rsplit_once('=') {
        Some((name, file)) if !name.is_empty() && !file.is_empty() => {
            Ok((name.to_owned(), PathBuf::from(file)))
        }
        _ => Err(format!("expected NAME=FILE, got `{s}`")),
    }
}

/// Connect, and bring the schema up to date.
pub fn store(db: &DbArgs) -> Result<PostgresStore, Failure> {
    let url = db.database_url.as_deref().ok_or_else(|| {
        Failure::undecided("no database: set ML_DATABASE_URL or pass --database-url")
    })?;
    let config: Config = url
        .parse()
        .map_err(|e| Failure::undecided(format!("bad database URL: {e}")))?;
    // One process, one command, one connection. The store's own `connect`
    // sizes its pool for a service; ten sessions to run one transaction
    // would be the wrong shape here.
    let pool = r2d2::Pool::builder()
        .max_size(1)
        .build(PostgresConnectionManager::new(config, NoTls))
        .map_err(|e| Failure::undecided(format!("cannot connect to the database: {e}")))?;
    let store = PostgresStore::from_pool(pool);
    store
        .migrate()
        .map_err(|e| Failure::undecided(format!("cannot prepare the database: {e}")))?;
    Ok(store)
}

/// The signer policy named by `--trust`.
pub fn signers(trust: &TrustArgs) -> Result<TrustedSigners, Failure> {
    let mut signers = TrustedSigners::new();
    for (principal, file) in &trust.trust {
        let principal = PrincipalId::new(principal.as_str())
            .map_err(|e| Failure::undecided(format!("--trust: {e}")))?;
        signers = signers.allow(principal, keys::load(file)?.verifying_key().to_bytes());
    }
    Ok(signers)
}

/// The cart adapter, knowing the merchant keys named by `--merchant-key`.
pub fn adapter(trust: &TrustArgs) -> Result<NativeCartAdapter, Failure> {
    let mut adapter = NativeCartAdapter::new();
    for (id, file) in &trust.merchant_keys {
        adapter = adapter.with_merchant_key(id.as_str(), keys::load(file)?.verifying_key());
    }
    Ok(adapter)
}

/// The whole engine, for a command that decides something.
pub fn ledger(db: &DbArgs, trust: &TrustArgs) -> Result<Engine, Failure> {
    Ok(Ledger::new(
        store(db)?,
        MockRail::new("mock", 1),
        signers(trust)?,
        SystemClock,
    ))
}

/// What an engine error means to the shell. A refusal is a report — the
/// ledger decided — and exits 2. Anything else means it could not decide:
/// exit 1, and the message says why.
pub fn outcome(err: &LedgerError) -> Result<Report, Failure> {
    match err.denied() {
        Some(Denied {
            ctx,
            stage,
            reason,
            detail,
        }) => Ok(Report::refused()
            .with("context", ctx.as_str())
            .with("stage", stage.to_string())
            .with("refused", reason.code())
            .with("detail", detail.as_str())),
        None => Err(Failure::undecided(err.to_string())),
    }
}
