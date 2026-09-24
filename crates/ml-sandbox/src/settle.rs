//! `ml settle`: ask the rail whether a recorded payment is final.
//!
//! Told with `--confirmations` or `--failed`, the sandbox rail reports that.
//! Asked with neither, it answers from the payment reference: `…-ok` is
//! final, `…-fail` failed, `…-reorg` gains a confirmation per check and is
//! then dropped. Pending changes nothing and can be asked again; failure
//! releases the reservation and leads to `ml compensate`.

use crate::Failure;
use crate::engine::{self, DbArgs, RailArgs, TrustArgs};
use crate::rail::{Finality, Reported};
use crate::report::Report;
use clap::Args;
use ml_core::{DenyReason, Settlement, Stage};

#[derive(Args)]
pub struct Cmd {
    /// The context, from `ml authorize`.
    #[arg(value_name = "CTX")]
    ctx: String,

    /// Speak for the rail: it reports this many confirmations.
    #[arg(long, value_name = "N", conflicts_with = "failed")]
    confirmations: Option<u32>,

    /// Speak for the rail: it reports the payment failed, for this reason.
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

    #[command(flatten)]
    rail: RailArgs,
}

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let ctx = engine::context(&cmd.ctx)?;
    let ledger = engine::ledger(&cmd.db, &cmd.trust, &cmd.rail)?;
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
    let finality = Finality {
        reference,
        reported: match (cmd.confirmations, &cmd.failed) {
            (Some(n), _) => Some(Reported::Confirmations(n)),
            (None, Some(reason)) => Some(Reported::Failed(reason.clone())),
            (None, None) => None,
        },
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
