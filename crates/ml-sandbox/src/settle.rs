//! `ml settle`: ask the rail whether a recorded payment is final.
//!
//! The mock rail reports what it is told: a number of confirmations, final
//! at one, or a failure. Pending changes nothing and can be asked again;
//! failure releases the reservation and leads to `ml compensate`.

use crate::Failure;
use crate::engine::{self, DbArgs, TrustArgs};
use crate::report::Report;
use clap::{ArgGroup, Args};
use ml_adapters::MockFinality;
use ml_core::{DenyReason, Settlement, Stage};

#[derive(Args)]
#[command(group = ArgGroup::new("evidence").required(true).args(["confirmations", "failed"]))]
pub struct Cmd {
    /// The context, from `ml authorize`.
    #[arg(value_name = "CTX")]
    ctx: String,

    /// What the rail reports: this many confirmations. Final at one.
    #[arg(long, value_name = "N")]
    confirmations: Option<u32>,

    /// What the rail reports: the payment failed, for this reason.
    #[arg(long, value_name = "REASON")]
    failed: Option<String>,

    /// The payment the evidence is about. Defaults to the one recorded at
    /// `ml pay`; evidence about another payment is refused.
    #[arg(long, value_name = "REF")]
    reference: Option<String>,

    #[command(flatten)]
    db: DbArgs,

    #[command(flatten)]
    trust: TrustArgs,
}

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let ctx = engine::context(&cmd.ctx)?;
    let ledger = engine::ledger(&cmd.db, &cmd.trust)?;
    let Some(reached) = engine::reached(&ledger, &ctx)? else {
        return Ok(engine::unreachable(
            &ctx,
            Stage::Settlement,
            DenyReason::ContextNotFound,
            "no such context",
        ));
    };
    let Some(paid) = reached.paid else {
        return Ok(engine::unreachable(
            &ctx,
            Stage::Settlement,
            DenyReason::InvalidState,
            format!(
                "the context is {}; it was never paid",
                engine::state_name(reached.state)
            ),
        ));
    };

    let reference = cmd
        .reference
        .clone()
        .unwrap_or_else(|| paid.reference().to_owned());
    let finality = match (cmd.confirmations, &cmd.failed) {
        (Some(n), None) => MockFinality::confirmed(reference, n),
        (None, Some(reason)) => MockFinality::failed(reference, reason.as_str()),
        _ => {
            return Err(Failure::undecided(
                "give --confirmations or --failed, not both",
            ));
        }
    };
    let report = Report::new().with("context", ctx.as_str());
    match ledger.record_settlement(&paid, &finality) {
        Ok(Settlement::Settled(settled)) => Ok(report
            .with("state", "settled")
            .with("reference", settled.reference())),
        Ok(Settlement::Pending { reason }) => {
            Ok(report.with("state", "pending").with("reason", reason))
        }
        Ok(Settlement::Failed(failed)) => Ok(report
            .with("state", "settlement_failed")
            .with("reason", failed.reason())),
        Err(err) => engine::outcome(&err),
    }
}
