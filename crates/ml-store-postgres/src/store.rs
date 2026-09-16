//! The PostgreSQL [`Store`] implementation.

use crate::convert::{
    EVENT_COLUMNS, RECORD_COLUMNS, backend, corrupt, event_from_row, money_from_sql, pg,
    record_from_row, state_to_sql,
};
use ml_core::{
    AppendOutcome, ContextId, Event, EventBody, Hash32, MandateId, Money, PaymentState, Record,
    Store, StoreError, Timestamp,
};
use postgres::{Config, GenericClient, NoTls};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use rust_decimal::Decimal;
use std::cmp::Ordering;

/// The connection pool this store runs on.
///
/// Construct it yourself with [`PostgresStore::from_pool`] when you need TLS
/// or non-default pool sizing; [`PostgresStore::connect`] builds a plaintext
/// one for local development.
pub type PgPool = Pool<PostgresConnectionManager<NoTls>>;

/// Held for the whole of `migrate`, so concurrent boots serialize.
const LOCK_MIGRATE: i32 = 0;

/// The schema, as an ordered list of migrations. `ml_schema` records which
/// of them a database has applied, and [`PostgresStore::migrate`] runs only
/// the rest — on a database that is already current, nothing at all.
const MIGRATIONS: [&str; 1] = [include_str!("migrations/0001_initial.sql")];
/// Serializes appends to one context, so the chain and the state advance
/// one step at a time.
const LOCK_CONTEXT: i32 = 1;
/// Serializes authorizations against one mandate, so two contexts cannot
/// both pass the same budget check. Always taken *after* [`LOCK_CONTEXT`],
/// never before, so no deadlock cycle can form.
const LOCK_MANDATE: i32 = 2;

/// A durable [`Store`] backed by PostgreSQL.
///
/// Every side effect of an append — the reservation, the velocity row, the
/// nonce, the state change and the event itself — is applied in a single
/// transaction, guarded by an advisory lock on the context (and, for an
/// authorization, on the mandate). Two concurrent requests against the same
/// budget therefore serialize, exactly as they do in
/// [`ml_core::MemoryStore`], and the same tests hold against both.
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Connect with no TLS. For local development, tests and the sandbox.
    ///
    /// `url` is a libpq connection string, e.g.
    /// `postgres://user@localhost:5432/mandate_ledger`.
    pub fn connect(url: &str) -> Result<Self, StoreError> {
        let config: Config = url.parse().map_err(backend)?;
        let manager = PostgresConnectionManager::new(config, NoTls);
        let pool = Pool::builder().build(manager).map_err(backend)?;
        Ok(Self::from_pool(pool))
    }

    /// Use a pool you built — the way to supply TLS or your own sizing.
    #[must_use]
    pub const fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The underlying pool.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Bring the schema up to date. Safe to call on every boot.
    ///
    /// Runs under an advisory lock, so replicas booting together serialize:
    /// `CREATE TABLE` is not safe to run concurrently, and two callers would
    /// otherwise race in the system catalogs.
    ///
    /// On a database that is already current this issues no DDL, and that
    /// matters. `CREATE INDEX IF NOT EXISTS` takes a `SHARE` lock on its
    /// table before it looks for the index, and a transaction keeps every
    /// lock until it ends — so re-running the whole schema would hold
    /// `ml_events` while waiting on `ml_contexts`, the mirror image of an
    /// `append` in flight, which holds `ml_contexts` while waiting on
    /// `ml_events`. PostgreSQL resolves that deadlock by killing one side:
    /// either the boot fails or a live payment does. Reading the version
    /// first takes only `ACCESS SHARE`, which conflicts with nothing an
    /// append does.
    ///
    /// Fails if the database is at a version newer than this build knows,
    /// rather than run against a schema it has never seen.
    pub fn migrate(&self) -> Result<(), StoreError> {
        let mut conn = self.pool.get().map_err(backend)?;
        let mut tx = conn.transaction().map_err(pg)?;
        tx.query("SELECT pg_advisory_xact_lock($1, 0)", &[&LOCK_MIGRATE])
            .map_err(pg)?;

        let applied = Self::applied_migrations(&mut tx)?;
        if applied > MIGRATIONS.len() {
            return Err(backend(format!(
                "database schema is at version {applied}, newer than this build's {}",
                MIGRATIONS.len()
            )));
        }
        for (version, sql) in (1_i32..).zip(MIGRATIONS).skip(applied) {
            tx.batch_execute(sql).map_err(pg)?;
            tx.execute("INSERT INTO ml_schema (version) VALUES ($1)", &[&version])
                .map_err(pg)?;
        }
        tx.commit().map_err(pg)
    }

    /// How many migrations this database has applied. A fresh database has
    /// none, and gets the version table itself — the one piece of DDL that
    /// is not a migration.
    fn applied_migrations(client: &mut impl GenericClient) -> Result<usize, StoreError> {
        let exists: bool = client
            .query_one("SELECT to_regclass('ml_schema') IS NOT NULL", &[])
            .map_err(pg)?
            .try_get(0)
            .map_err(pg)?;
        if !exists {
            client
                .batch_execute(
                    "CREATE TABLE ml_schema (\
                         version    INTEGER PRIMARY KEY, \
                         applied_at TIMESTAMPTZ NOT NULL DEFAULT now())",
                )
                .map_err(pg)?;
            return Ok(0);
        }
        let version: i32 = client
            .query_one("SELECT coalesce(max(version), 0) FROM ml_schema", &[])
            .map_err(pg)?
            .try_get(0)
            .map_err(pg)?;
        usize::try_from(version).map_err(corrupt)
    }

    /// Delete every row this store owns.
    ///
    /// **Destructive.** Intended for tests and for resetting a sandbox; it
    /// also restarts the event sequence, so any evidence bundle exported
    /// beforehand will no longer match the chain in the database.
    pub fn truncate_all(&self) -> Result<(), StoreError> {
        let mut conn = self.pool.get().map_err(backend)?;
        conn.batch_execute(
            "TRUNCATE ml_events, ml_contexts, ml_reservations, ml_authorizations, ml_nonces, \
             ml_revocations; ALTER SEQUENCE ml_event_seq RESTART WITH 1;",
        )
        .map_err(pg)
    }

    /// Read one context's record using any client or transaction.
    fn read_record(
        client: &mut impl GenericClient,
        ctx: &ContextId,
    ) -> Result<Option<Record>, StoreError> {
        let sql = format!("SELECT {RECORD_COLUMNS} FROM ml_contexts WHERE ctx = $1");
        let row = client.query_opt(&sql, &[&ctx.as_str()]).map_err(pg)?;
        row.as_ref().map(record_from_row).transpose()
    }

    /// Read a mandate's outstanding reservation using any client or transaction.
    fn read_reserved(
        client: &mut impl GenericClient,
        mandate: &MandateId,
    ) -> Result<Option<Money>, StoreError> {
        let row = client
            .query_opt(
                "SELECT amount, currency FROM ml_reservations WHERE mandate_id = $1",
                &[&mandate.as_str()],
            )
            .map_err(pg)?;
        match row {
            None => Ok(None),
            Some(row) => {
                let amount: Decimal = row.try_get(0).map_err(pg)?;
                let currency: String = row.try_get(1).map_err(pg)?;
                Ok(Some(money_from_sql(amount, &currency)?))
            }
        }
    }

    /// Mirror of `MemoryStore`'s transition guard.
    fn check_transition(
        existing: Option<&Record>,
        ctx: &ContextId,
        to: PaymentState,
    ) -> Result<(), StoreError> {
        let legal = match existing {
            None => to == PaymentState::Authorized,
            Some(r) => r.state.can_transition_to(to),
        };
        if legal {
            Ok(())
        } else {
            Err(StoreError::IllegalTransition {
                ctx: ctx.clone(),
                from: existing.map(|r| r.state),
                to,
            })
        }
    }
}

/// What phase 1 decided to do, applied only once every check has passed.
#[derive(Default)]
struct Effects {
    /// `(mandate, new running total)` — the value replaces the reservation.
    reserve: Option<(MandateId, Money)>,
    /// `(mandate, amount to subtract)`.
    release: Option<(MandateId, Money)>,
    /// A brand-new context row: `(mandate, cart hash, merchant, amount)`.
    create: Option<(MandateId, Hash32, String, Money)>,
}

impl Store for PostgresStore {
    fn record(&self, ctx: &ContextId) -> Result<Option<Record>, StoreError> {
        let mut conn = self.pool.get().map_err(backend)?;
        Self::read_record(&mut *conn, ctx)
    }

    fn events(&self, ctx: &ContextId) -> Result<Vec<Event>, StoreError> {
        let mut conn = self.pool.get().map_err(backend)?;
        let sql = format!("SELECT {EVENT_COLUMNS} FROM ml_events WHERE ctx = $1 ORDER BY seq");
        conn.query(&sql, &[&ctx.as_str()])
            .map_err(pg)?
            .iter()
            .map(event_from_row)
            .collect()
    }

    #[allow(clippy::too_many_lines)] // One transaction; splitting it would hide the ordering.
    fn append(
        &self,
        ctx: &ContextId,
        at: Timestamp,
        body: EventBody,
    ) -> Result<AppendOutcome, StoreError> {
        let mut conn = self.pool.get().map_err(backend)?;
        let mut tx = conn.transaction().map_err(pg)?;

        // Serialize every append for this context. This also covers a `Denied`
        // on a context that does not exist yet, where there is no row to lock.
        tx.query(
            "SELECT pg_advisory_xact_lock($1, hashtext($2))",
            &[&LOCK_CONTEXT, &ctx.as_str()],
        )
        .map_err(pg)?;

        // An authorization also contends on the mandate: two different
        // contexts must not both pass the same budget check.
        if let EventBody::Authorized { mandate, .. } = &body {
            tx.query(
                "SELECT pg_advisory_xact_lock($1, hashtext($2))",
                &[&LOCK_MANDATE, &mandate.body.id.as_str()],
            )
            .map_err(pg)?;
        }

        let existing = Self::read_record(&mut tx, ctx)?;

        // ── Phase 1: decide. Nothing is written until every check passes. ──
        let mut fx = Effects::default();

        match &body {
            EventBody::Authorized { mandate, cart, .. } => {
                Self::check_transition(existing.as_ref(), ctx, PaymentState::Authorized)?;
                let scope = &mandate.body.scope;
                let mandate_id = &mandate.body.id;

                if let Some(v) = scope.velocity {
                    let since = at.saturating_sub_secs(v.window_secs);
                    let count: i64 = tx
                        .query_one(
                            "SELECT count(*) FROM ml_authorizations \
                             WHERE mandate_id = $1 AND at >= $2",
                            &[&mandate_id.as_str(), &since.as_secs()],
                        )
                        .map_err(pg)?
                        .try_get(0)
                        .map_err(pg)?;
                    let count = u32::try_from(count).unwrap_or(u32::MAX);
                    if count >= v.max_count {
                        return Ok(AppendOutcome::VelocityExceeded { count });
                    }
                }

                let amount = &cart.claims.total;
                let current = Self::read_reserved(&mut tx, mandate_id)?
                    .unwrap_or_else(|| Money::zero(amount.currency().clone()));
                let next = current.checked_add(amount)?;
                if let Some(cap) = &scope.max_total {
                    if next.cmp_same_currency(cap)? == Ordering::Greater {
                        return Ok(AppendOutcome::BudgetExceeded {
                            remaining: cap.checked_sub(&current)?,
                        });
                    }
                }
                fx.reserve = Some((mandate_id.clone(), next));
                fx.create = Some((
                    mandate_id.clone(),
                    cart.hash,
                    cart.claims.merchant.as_str().to_owned(),
                    amount.clone(),
                ));
            }
            EventBody::Paid { rail, nonce, .. } => {
                Self::check_transition(existing.as_ref(), ctx, PaymentState::Paid)?;
                // Claim the nonce by writing it, and let the unique index be
                // the arbiter. Reading first and inserting afterwards is not
                // enough: concurrent contexts all observe a free nonce, then
                // all insert, and every loser silently no-ops on conflict —
                // so one payment proof would pay many contexts.
                //
                // On conflict with a row another transaction has not yet
                // committed, this blocks until that transaction resolves,
                // which is exactly the serialization required. This write
                // happens during the decision phase because it *is* the
                // decision; any later failure rolls the whole thing back.
                let claimed = tx
                    .query_opt(
                        "INSERT INTO ml_nonces (rail, nonce, ctx) VALUES ($1, $2, $3) \
                         ON CONFLICT (rail, nonce) DO NOTHING RETURNING ctx",
                        &[&rail.as_str(), &nonce.as_str(), &ctx.as_str()],
                    )
                    .map_err(pg)?;
                if claimed.is_none() {
                    // Somebody holds it. Our own earlier payment is an
                    // idempotent replay; any other context is a reused proof.
                    let owner: String = tx
                        .query_one(
                            "SELECT ctx FROM ml_nonces WHERE rail = $1 AND nonce = $2",
                            &[&rail.as_str(), &nonce.as_str()],
                        )
                        .map_err(pg)?
                        .try_get(0)
                        .map_err(pg)?;
                    if owner != ctx.as_str() {
                        return Ok(AppendOutcome::NonceAlreadyUsed);
                    }
                }
            }
            EventBody::Settled { .. } => {
                Self::check_transition(existing.as_ref(), ctx, PaymentState::Settled)?;
            }
            EventBody::SettlementFailed { .. } => {
                Self::check_transition(existing.as_ref(), ctx, PaymentState::SettlementFailed)?;
                let r = existing
                    .as_ref()
                    .ok_or_else(|| StoreError::Corrupt("no record".to_owned()))?;
                fx.release = Some((r.mandate_id.clone(), r.amount.clone()));
            }
            EventBody::Compensated { .. } => {
                Self::check_transition(existing.as_ref(), ctx, PaymentState::Compensated)?;
            }
            EventBody::Delivered { .. } => {
                Self::check_transition(existing.as_ref(), ctx, PaymentState::Delivered)?;
            }
            EventBody::Expired => {
                Self::check_transition(existing.as_ref(), ctx, PaymentState::Expired)?;
                let r = existing
                    .as_ref()
                    .ok_or_else(|| StoreError::Corrupt("no record".to_owned()))?;
                fx.release = Some((r.mandate_id.clone(), r.amount.clone()));
            }
            EventBody::Denied { .. } => {}
        }

        // The sequence number is part of the hash preimage, so it has to be
        // drawn before the row is built. nextval is non-transactional: a
        // rollback leaves a gap, which evidence verification tolerates.
        let seq: i64 = tx
            .query_one("SELECT nextval('ml_event_seq')", &[])
            .map_err(pg)?
            .try_get(0)
            .map_err(pg)?;
        let seq = u64::try_from(seq).map_err(corrupt)?;

        let prev_hash = tx
            .query_opt(
                "SELECT hash FROM ml_events WHERE ctx = $1 ORDER BY seq DESC LIMIT 1",
                &[&ctx.as_str()],
            )
            .map_err(pg)?
            .map(|row| -> Result<Hash32, StoreError> {
                let hash: String = row.try_get(0).map_err(pg)?;
                crate::convert::hash_from_sql(&hash)
            })
            .transpose()?
            .unwrap_or(Hash32::ZERO);

        let event = Event::new(prev_hash, seq, ctx.clone(), at, body)?;

        // ── Phase 2: write. ──
        if let Some((mandate, next)) = fx.reserve {
            tx.execute(
                "INSERT INTO ml_reservations (mandate_id, amount, currency) VALUES ($1, $2, $3) \
                 ON CONFLICT (mandate_id) \
                 DO UPDATE SET amount = EXCLUDED.amount, currency = EXCLUDED.currency",
                &[&mandate.as_str(), &next.amount(), &next.currency().as_str()],
            )
            .map_err(pg)?;
            tx.execute(
                "INSERT INTO ml_authorizations (mandate_id, ctx, at) VALUES ($1, $2, $3)",
                &[&mandate.as_str(), &ctx.as_str(), &at.as_secs()],
            )
            .map_err(pg)?;
        }

        if let Some((mandate, amount)) = fx.release {
            let current = Self::read_reserved(&mut tx, &mandate)?
                .ok_or_else(|| corrupt(format!("no reservation for {mandate}")))?;
            let next = current.checked_sub(&amount)?;
            if next.is_negative() {
                return Err(corrupt(format!(
                    "releasing {amount} from {current} on {mandate} goes negative"
                )));
            }
            tx.execute(
                "UPDATE ml_reservations SET amount = $2 WHERE mandate_id = $1",
                &[&mandate.as_str(), &next.amount()],
            )
            .map_err(pg)?;
        }

        if let Some((mandate, cart_hash, merchant, amount)) = fx.create {
            tx.execute(
                "INSERT INTO ml_contexts \
                 (ctx, state, mandate_id, cart_hash, merchant, amount, currency) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    &ctx.as_str(),
                    &state_to_sql(PaymentState::Authorized),
                    &mandate.as_str(),
                    &cart_hash.to_string(),
                    &merchant,
                    &amount.amount(),
                    &amount.currency().as_str(),
                ],
            )
            .map_err(pg)?;
        }

        // A `Denied` never changes state, and may target a context that does
        // not exist — in which case there is nothing to update.
        match &event.body {
            EventBody::Paid {
                rail,
                reference,
                idempotency_key,
                ..
            } => {
                tx.execute(
                    "UPDATE ml_contexts SET state = $2, rail = $3, payment_reference = $4, \
                     idempotency_key = $5 WHERE ctx = $1",
                    &[
                        &ctx.as_str(),
                        &state_to_sql(PaymentState::Paid),
                        &rail.as_str(),
                        &reference.as_str(),
                        &idempotency_key.to_string(),
                    ],
                )
                .map_err(pg)?;
            }
            EventBody::Settled { reference, .. } => {
                tx.execute(
                    "UPDATE ml_contexts SET state = $2, settlement_reference = $3 WHERE ctx = $1",
                    &[
                        &ctx.as_str(),
                        &state_to_sql(PaymentState::Settled),
                        &reference.as_str(),
                    ],
                )
                .map_err(pg)?;
            }
            EventBody::SettlementFailed { reason, .. } => {
                tx.execute(
                    "UPDATE ml_contexts SET state = $2, failure_reason = $3 WHERE ctx = $1",
                    &[
                        &ctx.as_str(),
                        &state_to_sql(PaymentState::SettlementFailed),
                        &reason.as_str(),
                    ],
                )
                .map_err(pg)?;
            }
            EventBody::Compensated { .. } => {
                tx.execute(
                    "UPDATE ml_contexts SET state = $2 WHERE ctx = $1",
                    &[&ctx.as_str(), &state_to_sql(PaymentState::Compensated)],
                )
                .map_err(pg)?;
            }
            EventBody::Delivered { receipt } => {
                tx.execute(
                    "UPDATE ml_contexts SET state = $2, receipt_reference = $3 WHERE ctx = $1",
                    &[
                        &ctx.as_str(),
                        &state_to_sql(PaymentState::Delivered),
                        &receipt.reference.as_str(),
                    ],
                )
                .map_err(pg)?;
            }
            EventBody::Expired => {
                tx.execute(
                    "UPDATE ml_contexts SET state = $2 WHERE ctx = $1",
                    &[&ctx.as_str(), &state_to_sql(PaymentState::Expired)],
                )
                .map_err(pg)?;
            }
            EventBody::Authorized { .. } | EventBody::Denied { .. } => {}
        }

        let body_json = serde_json::to_value(&event.body).map_err(corrupt)?;
        tx.execute(
            "INSERT INTO ml_events (seq, ctx, at, prev_hash, hash, body) \
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[
                &i64::try_from(event.seq).map_err(corrupt)?,
                &ctx.as_str(),
                &at.as_secs(),
                &event.prev_hash.to_string(),
                &event.hash.to_string(),
                &body_json,
            ],
        )
        .map_err(pg)?;

        tx.commit().map_err(pg)?;
        Ok(AppendOutcome::Appended(event))
    }

    fn reserved(&self, mandate: &MandateId) -> Result<Option<Money>, StoreError> {
        let mut conn = self.pool.get().map_err(backend)?;
        Self::read_reserved(&mut *conn, mandate)
    }

    fn is_revoked(&self, mandate: &MandateId) -> Result<bool, StoreError> {
        let mut conn = self.pool.get().map_err(backend)?;
        let row = conn
            .query_opt(
                "SELECT 1 FROM ml_revocations WHERE mandate_id = $1",
                &[&mandate.as_str()],
            )
            .map_err(pg)?;
        Ok(row.is_some())
    }

    fn revoke(&self, mandate: &MandateId) -> Result<(), StoreError> {
        let mut conn = self.pool.get().map_err(backend)?;
        conn.execute(
            "INSERT INTO ml_revocations (mandate_id) VALUES ($1) ON CONFLICT DO NOTHING",
            &[&mandate.as_str()],
        )
        .map_err(pg)?;
        Ok(())
    }
}
