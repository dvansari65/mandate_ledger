//! `ml contexts`: where every context stands now.

use crate::Failure;
use crate::engine::{self, DbArgs};
use crate::report::Report;
use clap::Args;
use ml_core::{ContextId, MandateId, PaymentState, Record, RecordFilter, Store as _};
use serde_json::Value;

#[derive(Args)]
pub struct Cmd {
    /// Only contexts authorized under this mandate.
    #[arg(long, value_name = "ID")]
    mandate: Option<String>,

    #[arg(
        long,
        value_name = "STATE",
        value_parser = state,
        help = "Only contexts in this state: authorized, paid, settled, settlement_failed, \
                compensated, delivered or expired"
    )]
    state: Option<PaymentState>,

    /// Start just after this context id. The report's `next` is the last id
    /// listed: pass it back to read the contexts after it.
    #[arg(long, value_name = "CTX")]
    after: Option<String>,

    /// At most this many contexts, in byte order of their ids.
    #[arg(long, default_value_t = 50, value_name = "N")]
    limit: usize,

    #[command(flatten)]
    db: DbArgs,
}

pub const COLUMNS: &[&str] = &["context", "state", "mandate", "merchant", "amount"];

/// The engine's own spelling of a state is its wire form.
fn state(s: &str) -> Result<PaymentState, String> {
    serde_json::from_value(Value::String(s.to_owned())).map_err(|_| {
        format!(
            "unknown state `{s}`; one of authorized, paid, settled, settlement_failed, \
             compensated, delivered, expired"
        )
    })
}

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let filter = RecordFilter {
        mandate: cmd
            .mandate
            .as_deref()
            .map(MandateId::new)
            .transpose()
            .map_err(|e| Failure::undecided(format!("--mandate: {e}")))?,
        state: cmd.state,
    };
    let after = cmd
        .after
        .as_deref()
        .map(ContextId::new)
        .transpose()
        .map_err(|e| Failure::undecided(format!("--after: {e}")))?;
    let records = engine::store(&cmd.db)?
        .scan(&filter, after.as_ref(), cmd.limit)
        .map_err(|e| Failure::undecided(e.to_string()))?;
    // The cursor for the page after this one: the last id listed, or the
    // one given when nothing was.
    let next = records.last().map(|r| r.ctx.clone()).or(after);
    Ok(Report::new()
        .with("next", next.map(|ctx| ctx.as_str().to_owned()))
        .table("contexts", COLUMNS, records.iter().map(row).collect()))
}

fn row(record: &Record) -> Vec<Value> {
    vec![
        record.ctx.as_str().into(),
        engine::state_name(record.state).into(),
        record.mandate_id.as_str().into(),
        record.merchant.as_str().into(),
        record.amount.to_string().into(),
    ]
}
