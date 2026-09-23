//! `ml expire`: release an authorization that will not be paid.

use crate::Failure;
use crate::engine::{self, DbArgs, TrustArgs};
use crate::report::Report;
use clap::Args;
use ml_core::{DenyReason, Stage};

#[derive(Args)]
pub struct Cmd {
    /// The context, from `ml authorize`.
    #[arg(value_name = "CTX")]
    ctx: String,

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
            Stage::Expiry,
            DenyReason::ContextNotFound,
            "no such context",
        ));
    };
    // Every context holds an `Authorized`; the engine decides whether it can
    // still lapse, and records the refusal if it has moved on.
    match ledger.expire(&reached.authorized) {
        Ok(()) => Ok(Report::new()
            .with("context", ctx.as_str())
            .with("state", "expired")),
        Err(err) => engine::outcome(&err),
    }
}
