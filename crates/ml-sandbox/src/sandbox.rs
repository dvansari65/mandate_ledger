//! The sandbox's own state, kept in the same database as the ledger so
//! that every `ml` process sees it: the test clock, and the controllable
//! rail's memory of what it has already reported.
//!
//! These tables are the sandbox's, not the ledger's. They are prefixed
//! `ml_sandbox_`, created the first time a feature needs them, and never
//! touched by the ledger's migrations. Creation runs under an advisory lock
//! of its own — concurrent `CREATE TABLE IF NOT EXISTS` races in the system
//! catalogs — and readers never issue DDL, so a command that only reads is
//! never queued behind one that creates.

use ml_core::{Clock as _, SystemClock, Timestamp};
use ml_store_postgres::PgPool;
use postgres::{Client, GenericClient};

/// Advisory-lock key for sandbox DDL. The ledger's own locks use 0–3.
const LOCK_SANDBOX: i32 = 100;

const CLOCK_TABLE: &str = "ml_sandbox_clock";
const CLOCK_DDL: &str = "CREATE TABLE IF NOT EXISTS ml_sandbox_clock (\
    one BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (one), \
    now BIGINT NOT NULL)";

const RAIL_TABLE: &str = "ml_sandbox_rail_checks";
const RAIL_DDL: &str = "CREATE TABLE IF NOT EXISTS ml_sandbox_rail_checks (\
    rail TEXT NOT NULL, \
    reference TEXT NOT NULL, \
    checks INTEGER NOT NULL, \
    PRIMARY KEY (rail, reference))";

/// Errors here are messages: the caller decides whether that means the
/// command could not decide or the rail was unavailable.
type Result<T> = std::result::Result<T, String>;

fn connection(
    pool: &PgPool,
) -> Result<r2d2::PooledConnection<r2d2_postgres::PostgresConnectionManager<postgres::NoTls>>> {
    pool.get()
        .map_err(|e| format!("no database connection: {e}"))
}

fn exists(client: &mut impl GenericClient, table: &str) -> Result<bool> {
    client
        .query_one("SELECT to_regclass($1) IS NOT NULL", &[&table])
        .and_then(|row| row.try_get(0))
        .map_err(|e| e.to_string())
}

/// Create `table` if it is missing, serialized against other creators.
fn ensure(client: &mut Client, table: &str, ddl: &str) -> Result<()> {
    if exists(client, table)? {
        return Ok(());
    }
    let mut tx = client.transaction().map_err(|e| e.to_string())?;
    tx.query("SELECT pg_advisory_xact_lock($1, 0)", &[&LOCK_SANDBOX])
        .map_err(|e| e.to_string())?;
    tx.batch_execute(ddl).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

/// The instant the clock is frozen at, if it is.
pub fn clock(pool: &PgPool) -> Result<Option<Timestamp>> {
    let mut conn = connection(pool)?;
    if !exists(&mut *conn, CLOCK_TABLE)? {
        return Ok(None);
    }
    let row = conn
        .query_opt("SELECT now FROM ml_sandbox_clock", &[])
        .map_err(|e| e.to_string())?;
    row.map(|r| r.try_get(0).map(Timestamp).map_err(|e| e.to_string()))
        .transpose()
}

/// Freeze the clock at `at`.
pub fn set_clock(pool: &PgPool, at: Timestamp) -> Result<()> {
    let mut conn = connection(pool)?;
    ensure(&mut conn, CLOCK_TABLE, CLOCK_DDL)?;
    conn.execute(
        "INSERT INTO ml_sandbox_clock (one, now) VALUES (TRUE, $1) \
         ON CONFLICT (one) DO UPDATE SET now = EXCLUDED.now",
        &[&at.as_secs()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Move the clock forward by `secs`, freezing it at wall time first if it
/// was not frozen. Returns the new instant.
pub fn advance_clock(pool: &PgPool, secs: i64) -> Result<Timestamp> {
    let mut conn = connection(pool)?;
    ensure(&mut conn, CLOCK_TABLE, CLOCK_DDL)?;
    let mut tx = conn.transaction().map_err(|e| e.to_string())?;
    let frozen: Option<i64> = tx
        .query_opt("SELECT now FROM ml_sandbox_clock FOR UPDATE", &[])
        .map_err(|e| e.to_string())?
        .map(|r| r.try_get(0))
        .transpose()
        .map_err(|e| e.to_string())?;
    let base = frozen.unwrap_or_else(|| SystemClock.now().as_secs());
    let next = base.saturating_add(secs);
    tx.execute(
        "INSERT INTO ml_sandbox_clock (one, now) VALUES (TRUE, $1) \
         ON CONFLICT (one) DO UPDATE SET now = EXCLUDED.now",
        &[&next],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(Timestamp(next))
}

/// Back to wall time.
pub fn reset_clock(pool: &PgPool) -> Result<()> {
    let mut conn = connection(pool)?;
    if exists(&mut *conn, CLOCK_TABLE)? {
        conn.execute("DELETE FROM ml_sandbox_clock", &[])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Record one more finality check of `reference` on `rail`, and return how
/// many there have been, this one included.
pub fn count_check(pool: &PgPool, rail: &str, reference: &str) -> Result<u32> {
    let mut conn = connection(pool)?;
    ensure(&mut conn, RAIL_TABLE, RAIL_DDL)?;
    let checks: i32 = conn
        .query_one(
            "INSERT INTO ml_sandbox_rail_checks (rail, reference, checks) VALUES ($1, $2, 1) \
             ON CONFLICT (rail, reference) \
             DO UPDATE SET checks = ml_sandbox_rail_checks.checks + 1 \
             RETURNING checks",
            &[&rail, &reference],
        )
        .and_then(|row| row.try_get(0))
        .map_err(|e| e.to_string())?;
    u32::try_from(checks).map_err(|e| e.to_string())
}
