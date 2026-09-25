//! `ml compensate`: record that the host undid its side effects after a
//! settlement failed.

use crate::Failure;
use crate::engine::{self, DbArgs, RailArgs, TrustArgs};
use crate::report::Report;
use clap::Args;
use ml_core::{DenyReason, Stage};

#[derive(Args)]
pub struct Cmd {
    /// The context, from `ml authorize`.
    #[arg(value_name = "CTX")]
    ctx: String,

    /// A reference to the compensating action — a refund id, a reversal.
    #[arg(long, value_name = "REF")]
    reference: Option<String>,

    #[command(flatten)]
    db: DbArgs,

    #[command(flatten)]
    trust: TrustArgs,

    #[command(flatten)]
    rail: RailArgs,
}

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let ctx = engine::context(&cmd.ctx)?;
    let ledger = engine::ledger(&cmd.db, &cmd.trust, &cmd.rail)?;
    let Some(reached) = engine::reached(&ledger, &ctx)? else {
        return Ok(engine::unreachable(
            &ctx,
            Stage::Compensation,
            DenyReason::ContextNotFound,
            "no such context",
        ));
    };
    let Some(failed) = reached.settlement_failed else {
        return Ok(engine::unreachable(
            &ctx,
            Stage::Compensation,
            DenyReason::InvalidState,
            format!(
                "the context is {}; no settlement failed",
                engine::state_name(reached.state)
            ),
        ));
    };
    match ledger.compensate(&failed, cmd.reference.as_deref()) {
        Ok(compensated) => Ok(Report::new()
            .with("context", compensated.ctx().as_str())
            .with("state", "compensated")),
        Err(err) => engine::outcome(&err),
    }
}
