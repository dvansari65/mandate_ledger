//! `ml evidence`: export a context's chain as a bundle anyone can verify
//! without this database — and, signed with the host's key, prove who
//! exported it.

use crate::Failure;
use crate::engine::{self, DbArgs, TrustArgs};
use crate::report::Report;
use crate::{files, keys};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct Cmd {
    /// The context, from `ml authorize`.
    #[arg(value_name = "CTX")]
    ctx: String,

    /// Where to write the bundle.
    #[arg(long, value_name = "FILE")]
    out: PathBuf,

    /// Sign the bundle as the exporting host, with this private key. A
    /// third party then checks not only that the chain is intact but who
    /// vouched for it.
    #[arg(long, value_name = "KEYFILE")]
    sign: Option<PathBuf>,

    #[command(flatten)]
    db: DbArgs,
}

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let ctx = engine::context(&cmd.ctx)?;
    let host = cmd.sign.as_deref().map(keys::load).transpose()?;
    let ledger = engine::ledger(&cmd.db, &TrustArgs::default())?;
    // Exporting is not a payment step, so an unknown context is an error,
    // not a refusal: there is no decision here to record.
    let bundle = ledger
        .evidence(&ctx)
        .map_err(|e| Failure::undecided(e.to_string()))?
        .ok_or_else(|| Failure::undecided(format!("no such context: {ctx}")))?;

    let report = Report::new()
        .with("file", cmd.out.display().to_string())
        .with("context", ctx.as_str())
        .with("events", bundle.events.len())
        .with("final_state", bundle.final_state().map(engine::state_name));
    if let Some(key) = host {
        let signed = bundle
            .sign(&key)
            .map_err(|e| Failure::undecided(format!("cannot sign: {e}")))?;
        files::write_json(&cmd.out, &signed)?;
        Ok(report.with("signed_by", hex::encode(signed.signer)))
    } else {
        files::write_json(&cmd.out, &bundle)?;
        Ok(report)
    }
}
