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

    /// Start just after this sequence number. Without it, the most recent
    /// events. The report's `next` is the value to pass to read what came
    /// after; `--limit 0` gives `next` alone, which is where "now" is.
    #[arg(long, value_name = "SEQ")]
    after: Option<u64>,

    /// At most this many events.
    #[arg(long, default_value_t = 50, value_name = "N")]
    limit: usize,

    #[command(flatten)]
    db: DbArgs,
}

pub const COLUMNS: &[&str] = &["seq", "at", "context", "event", "code", "detail"];

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let store = engine::store(&cmd.db)?;
    let undecided = |e: ml_core::StoreError| Failure::undecided(e.to_string());

    if let Some(ctx) = &cmd.context {
        let ctx = ContextId::new(ctx.as_str())
            .map_err(|e| Failure::undecided(format!("--context: {e}")))?;
        let events = store.events(&ctx).map_err(undecided)?;
        // No cursor: a chain's last seq says nothing about the global log.
        return Ok(Report::new().table("events", COLUMNS, events.iter().map(row).collect()));
    }

    let after = if let Some(after) = cmd.after {
        after
    } else {
        // The tail. Gaps (rolled-back appends) make this "up to N".
        let window = u64::try_from(cmd.limit).unwrap_or(u64::MAX);
        store.last_seq().map_err(undecided)?.saturating_sub(window)
    };
    let events = store.events_after(after, cmd.limit).map_err(undecided)?;
    let next = events.last().map_or(after, |e| e.seq);
    Ok(Report::new()
        .with("next", next)
        .table("events", COLUMNS, events.iter().map(row).collect()))
}

pub fn row(event: &Event) -> Vec<Value> {
    let (kind, code, detail) = describe(&event.body);
    vec![
        event.seq.into(),
        event.at.as_secs().into(),
        event.ctx.as_str().into(),
        kind.into(),
        code.map_or(Value::Null, Into::into),
        detail.into(),
    ]
}

/// One line per event: what happened, the refusal code if it is one, and
/// the fact that matters most.
fn describe(body: &EventBody) -> (&'static str, Option<&'static str>, String) {
    match body {
        EventBody::Authorized { mandate, cart, .. } => (
            "authorized",
            None,
            format!(
                "{} at {} under {}",
                cart.claims.total, cart.claims.merchant, mandate.body.id
            ),
        ),
        EventBody::Denied {
            stage,
            reason,
            detail,
        } => ("denied", Some(reason.code()), format!("{stage}: {detail}")),
        EventBody::Paid {
            rail,
            reference,
            amount,
            ..
        } => ("paid", None, format!("{amount} via {rail}, {reference}")),
        EventBody::Settled { reference, .. } => ("settled", None, reference.clone()),
        EventBody::SettlementFailed { reason, .. } => ("settlement_failed", None, reason.clone()),
        EventBody::Compensated { reference } => {
            ("compensated", None, reference.clone().unwrap_or_default())
        }
        EventBody::Delivered { receipt } => ("delivered", None, receipt.reference.clone()),
        EventBody::Expired => ("expired", None, String::new()),
    }
}
