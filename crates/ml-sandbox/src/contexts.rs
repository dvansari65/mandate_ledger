//! `ml contexts`: where every context stands now.

use crate::Failure;
use crate::engine::{self, DbArgs};
use crate::report::Report;
use clap::Args;
use ml_core::{MandateId, PaymentState, Record, RecordFilter, Store as _};
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
    let records = engine::store(&cmd.db)?
        .scan(&filter, cmd.limit)
        .map_err(|e| Failure::undecided(e.to_string()))?;
    Ok(Report::new().table("contexts", COLUMNS, records.iter().map(row).collect()))
}

fn row(record: &Record) -> Vec<Value> {
    vec![
        record.ctx.as_str().into(),
        serde_json::to_value(record.state).expect("a state serializes"),
        record.mandate_id.as_str().into(),
        record.merchant.as_str().into(),
        record.amount.to_string().into(),
    ]
}
