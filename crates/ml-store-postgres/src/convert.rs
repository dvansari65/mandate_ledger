//! Mapping between `ml-core` types and their SQL columns.
//!
//! Every conversion here is part of the database contract, so each one is
//! written out explicitly rather than derived. The `PaymentState` mapping in
//! particular is an exhaustive match on purpose: if `ml-core` gains a state,
//! this crate stops compiling until the column value is decided.

use ml_core::{
    Currency, Event, EventBody, Hash32, MerchantId, Money, PaymentState, Record, StoreError,
    Timestamp,
};
use postgres::Row;
use rust_decimal::Decimal;

/// Wraps any error as [`StoreError::Backend`].
pub fn backend<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Backend(e.to_string())
}

/// Wraps a `postgres` error as [`StoreError::Backend`], keeping the detail.
///
/// `postgres::Error` renders as just `"db error"`; everything useful — the
/// SQLSTATE, the server message, the constraint that was violated — hangs off
/// its source. Reporting only the `Display` form leaves an operator with
/// nothing to act on, so this digs the detail out.
// Taken by value because this is used as `map_err(pg)`, which hands over
// ownership; a reference-taking version would need a closure at every site.
#[allow(clippy::needless_pass_by_value)]
pub fn pg(e: postgres::Error) -> StoreError {
    if let Some(db) = e.as_db_error() {
        let mut msg = format!("{}: {}", db.code().code(), db.message());
        if let Some(detail) = db.detail() {
            msg.push_str(" — ");
            msg.push_str(detail);
        }
        if let Some(constraint) = db.constraint() {
            msg.push_str(" (constraint ");
            msg.push_str(constraint);
            msg.push(')');
        }
        return StoreError::Backend(msg);
    }
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(cause) = source {
        msg.push_str(": ");
        msg.push_str(&cause.to_string());
        source = cause.source();
    }
    StoreError::Backend(msg)
}

/// Wraps any error as [`StoreError::Corrupt`] — stored data the engine cannot use.
pub fn corrupt<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Corrupt(e.to_string())
}

/// The column value for a state.
pub const fn state_to_sql(state: PaymentState) -> &'static str {
    match state {
        PaymentState::Authorized => "authorized",
        PaymentState::Paid => "paid",
        PaymentState::Settled => "settled",
        PaymentState::SettlementFailed => "settlement_failed",
        PaymentState::Compensated => "compensated",
        PaymentState::Delivered => "delivered",
        PaymentState::Expired => "expired",
    }
}

/// Parse a state column value.
pub fn state_from_sql(value: &str) -> Result<PaymentState, StoreError> {
    match value {
        "authorized" => Ok(PaymentState::Authorized),
        "paid" => Ok(PaymentState::Paid),
        "settled" => Ok(PaymentState::Settled),
        "settlement_failed" => Ok(PaymentState::SettlementFailed),
        "compensated" => Ok(PaymentState::Compensated),
        "delivered" => Ok(PaymentState::Delivered),
        "expired" => Ok(PaymentState::Expired),
        other => Err(corrupt(format!("unknown payment state `{other}`"))),
    }
}

/// Parse a hash column value (`sha256:<hex>`).
pub fn hash_from_sql(value: &str) -> Result<Hash32, StoreError> {
    value
        .parse()
        .map_err(|_| corrupt(format!("unparseable hash `{value}`")))
}

/// Rebuild a [`Money`] from its amount and currency columns.
pub fn money_from_sql(amount: Decimal, currency: &str) -> Result<Money, StoreError> {
    Ok(Money::new(
        amount,
        Currency::new(currency).map_err(corrupt)?,
    ))
}

/// Rebuild a [`Record`] from a `ml_contexts` row.
///
/// Column order is fixed by [`RECORD_COLUMNS`].
pub fn record_from_row(row: &Row) -> Result<Record, StoreError> {
    let ctx: String = row.try_get(0).map_err(backend)?;
    let state: String = row.try_get(1).map_err(backend)?;
    let mandate_id: String = row.try_get(2).map_err(backend)?;
    let cart_hash: String = row.try_get(3).map_err(backend)?;
    let merchant: String = row.try_get(4).map_err(backend)?;
    let amount: Decimal = row.try_get(5).map_err(backend)?;
    let currency: String = row.try_get(6).map_err(backend)?;
    let idempotency_key: Option<String> = row.try_get(9).map_err(backend)?;

    Ok(Record {
        ctx: ctx.try_into().map_err(corrupt)?,
        state: state_from_sql(&state)?,
        mandate_id: mandate_id.try_into().map_err(corrupt)?,
        cart_hash: hash_from_sql(&cart_hash)?,
        merchant: MerchantId::new(merchant).map_err(corrupt)?,
        amount: money_from_sql(amount, &currency)?,
        rail: row.try_get(7).map_err(backend)?,
        payment_reference: row.try_get(8).map_err(backend)?,
        idempotency_key: idempotency_key.as_deref().map(hash_from_sql).transpose()?,
        settlement_reference: row.try_get(10).map_err(backend)?,
        failure_reason: row.try_get(11).map_err(backend)?,
        receipt_reference: row.try_get(12).map_err(backend)?,
    })
}

/// The `ml_contexts` columns, in the order [`record_from_row`] expects.
pub const RECORD_COLUMNS: &str = "ctx, state, mandate_id, cart_hash, merchant, amount, currency, \
     rail, payment_reference, idempotency_key, settlement_reference, failure_reason, \
     receipt_reference";

/// Rebuild an [`Event`] from a `ml_events` row: `seq, ctx, at, prev_hash, hash, body`.
pub fn event_from_row(row: &Row) -> Result<Event, StoreError> {
    let seq: i64 = row.try_get(0).map_err(backend)?;
    let ctx: String = row.try_get(1).map_err(backend)?;
    let at: i64 = row.try_get(2).map_err(backend)?;
    let prev_hash: String = row.try_get(3).map_err(backend)?;
    let hash: String = row.try_get(4).map_err(backend)?;
    let body: serde_json::Value = row.try_get(5).map_err(backend)?;
    let body: EventBody = serde_json::from_value(body).map_err(corrupt)?;

    Ok(Event {
        seq: u64::try_from(seq).map_err(corrupt)?,
        ctx: ctx.try_into().map_err(corrupt)?,
        at: Timestamp(at),
        prev_hash: hash_from_sql(&prev_hash)?,
        hash: hash_from_sql(&hash)?,
        body,
    })
}

/// The `ml_events` columns, in the order [`event_from_row`] expects.
pub const EVENT_COLUMNS: &str = "seq, ctx, at, prev_hash, hash, body";
