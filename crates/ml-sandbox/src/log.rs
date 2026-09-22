//! `ml log`: what happened — every event in every context, refusals
//! included, in the order the store committed them.

use crate::Failure;
use crate::engine::{self, DbArgs};
use crate::report::Report;
use clap::Args;
use ml_core::{ContextId, Event, EventBody, Store as _};
use serde_json::Value;

#[derive(Args)]
pub struct Cmd {
    /// One context's chain, oldest first, instead of the global log.
    #[arg(long, value_name = "CTX", conflicts_with_all = ["after", "limit"])]
    context: Option<String>,

    /// Start after this sequence number. The report's `next` is the value to
    /// pass to read what came after.
    #[arg(long, default_value_t = 0, value_name = "SEQ")]
    after: u64,

    /// At most this many events.
    #[arg(long, default_value_t = 50, value_name = "N")]
    limit: usize,

    #[command(flatten)]
    db: DbArgs,
}

pub const COLUMNS: &[&str] = &["seq", "at", "context", "event", "detail"];

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let store = engine::store(&cmd.db)?;
    let events = match &cmd.context {
        Some(ctx) => {
            let ctx = ContextId::new(ctx.as_str())
                .map_err(|e| Failure::undecided(format!("--context: {e}")))?;
            store.events(&ctx)
        }
        None => store.events_after(cmd.after, cmd.limit),
    }
    .map_err(|e| Failure::undecided(e.to_string()))?;

    let next = events.last().map_or(cmd.after, |e| e.seq);
    let rows = events.iter().map(row).collect();
    Ok(Report::new()
        .with("next", next)
        .table("events", COLUMNS, rows))
}

pub fn row(event: &Event) -> Vec<Value> {
    let (kind, detail) = describe(&event.body);
    vec![
        event.seq.into(),
        event.at.as_secs().into(),
        event.ctx.as_str().into(),
        kind.into(),
        detail.into(),
    ]
}

/// One line per event: what happened, and the fact that matters most.
fn describe(body: &EventBody) -> (&'static str, String) {
    match body {
        EventBody::Authorized { mandate, cart, .. } => (
            "authorized",
            format!(
                "{} at {} under {}",
                cart.claims.total, cart.claims.merchant, mandate.body.id
            ),
        ),
        EventBody::Denied {
            stage,
            reason,
            detail,
        } => ("denied", format!("{stage}: {} — {detail}", reason.code())),
        EventBody::Paid {
            rail,
            reference,
            amount,
            ..
        } => ("paid", format!("{amount} via {rail}, {reference}")),
        EventBody::Settled { reference, .. } => ("settled", reference.clone()),
        EventBody::SettlementFailed { reason, .. } => ("settlement_failed", reason.clone()),
        EventBody::Compensated { reference } => {
            ("compensated", reference.clone().unwrap_or_default())
        }
        EventBody::Delivered { receipt } => ("delivered", receipt.reference.clone()),
        EventBody::Expired => ("expired", String::new()),
    }
}
