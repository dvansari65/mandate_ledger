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
use ml_core::{
    ContextId, Denied, DenyReason, Ledger, LedgerError, Money, PaymentState, PrincipalId, Reached,
    Stage, SystemClock, TrustedSigners,
};
use ml_store_postgres::PostgresStore;
use postgres::{Config, NoTls};
use r2d2_postgres::PostgresConnectionManager;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long to keep trying to reach the database. Long enough for a slow
/// network handshake, short enough that a typo in the URL is not a
/// thirty-second wait.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

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
/// mandate cannot vouch for its own key. Only public keys are read, so the
/// `.pub` files are enough; the private files are accepted too.
#[derive(Args, Default)]
pub struct TrustArgs {
    /// Trust the key in KEYFILE to sign mandates for PRINCIPAL. Repeatable.
    #[arg(long = "trust", value_name = "PRINCIPAL=KEYFILE", value_parser = pair)]
    trust: Vec<(String, PathBuf)>,

    /// Accept carts signed by the key in KEYFILE under merchant key id ID.
    /// Repeatable.
    #[arg(long = "merchant-key", value_name = "ID=KEYFILE", value_parser = pair)]
    merchant_keys: Vec<(String, PathBuf)>,
}

/// Keeps the last connection error, which r2d2 otherwise reduces to
/// "timed out waiting for connection".
#[derive(Debug)]
struct LastError(Arc<Mutex<Option<String>>>);

impl r2d2::HandleError<postgres::Error> for LastError {
    fn handle_error(&self, error: postgres::Error) {
        // `postgres::Error` displays as "error connecting to server"; the
        // reason — refused, unknown host, bad password — is in its source.
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
    let last_error = Arc::new(Mutex::new(None));
    let pool = r2d2::Pool::builder()
        .max_size(1)
        .connection_timeout(CONNECT_TIMEOUT)
        .error_handler(Box::new(LastError(Arc::clone(&last_error))))
        .build(PostgresConnectionManager::new(config, NoTls))
        .map_err(|e| {
            let cause = last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .unwrap_or_else(|| e.to_string());
            Failure::undecided(format!("cannot connect to the database: {cause}"))
        })?;
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
        signers = signers.allow(principal, keys::public(file)?.to_bytes());
    }
    Ok(signers)
}

/// The cart adapter, knowing the merchant keys named by `--merchant-key`.
pub fn adapter(trust: &TrustArgs) -> Result<NativeCartAdapter, Failure> {
    let mut adapter = NativeCartAdapter::new();
    for (id, file) in &trust.merchant_keys {
        adapter = adapter.with_merchant_key(id.as_str(), keys::public(file)?);
    }
    Ok(adapter)
}

/// The whole engine, for a command that decides something. Local
/// configuration is checked before the database is touched.
pub fn ledger(db: &DbArgs, trust: &TrustArgs) -> Result<Engine, Failure> {
    let signers = signers(trust)?;
    Ok(Ledger::new(
        store(db)?,
        MockRail::new("mock", 1),
        signers,
        SystemClock,
    ))
}

/// What an engine error means to the shell. A refusal is a report — the
/// ledger decided, and wrote the decision down — and exits 2. Anything else
/// means it could not decide: exit 1, and the message says why.
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
            .with("detail", detail.as_str())
            .with("recorded", true)),
        None => Err(Failure::undecided(err.to_string())),
    }
}

/// A step the engine cannot even be asked about: the context does not
/// exist, or never earned the token the step needs — a delivery on a
/// context that was never settled has no `Settled` to present. Refused
/// here with the engine's own code for it, and marked as not recorded,
/// because the type system said no before the ledger could.
pub fn unreachable(
    ctx: &ContextId,
    stage: Stage,
    reason: DenyReason,
    detail: impl Into<String>,
) -> Report {
    Report::refused()
        .with("context", ctx.as_str())
        .with("stage", stage.to_string())
        .with("refused", reason.code())
        .with("detail", detail.into())
        .with("recorded", false)
}

/// A context id from the command line.
pub fn context(s: &str) -> Result<ContextId, Failure> {
    ContextId::new(s).map_err(|e| Failure::undecided(format!("context: {e}")))
}

/// The tokens `ctx` has earned, if it exists.
pub fn reached(ledger: &Engine, ctx: &ContextId) -> Result<Option<Reached>, Failure> {
    ledger
        .reached(ctx)
        .map_err(|e| Failure::undecided(e.to_string()))
}

/// The engine's own spelling of a state — its wire form.
pub fn state_name(state: PaymentState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `AMOUNT CURRENCY`, as the engine prints money: `128.00 INR`.
pub fn money(s: &str) -> Result<Money, String> {
    let (amount, currency) = s
        .split_once(' ')
        .ok_or_else(|| format!("expected `AMOUNT CURRENCY`, e.g. `128.00 INR`, got `{s}`"))?;
    Money::parse(amount, currency).map_err(|e| e.to_string())
}
