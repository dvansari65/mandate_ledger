//! `ml deliver`: record fulfilment. Only a settled context can be delivered
//! — the engine's `record_delivery` takes a `Settled` token and nothing
//! else — so on any other context this command has nothing to present, and
//! says so without troubling the ledger.

use crate::Failure;
use crate::engine::{self, DbArgs, TrustArgs};
use crate::report::Report;
use clap::Args;
use ml_core::{Attestation, DeliveryReceipt, DenyReason, Stage};

#[derive(Args)]
pub struct Cmd {
    /// The context, from `ml authorize`.
    #[arg(value_name = "CTX")]
    ctx: String,

    /// The merchant's fulfilment reference — an order id, a tracking number.
    #[arg(long, value_name = "REF")]
    receipt: String,

    /// The merchant key id that vouches for the receipt. Without it, the
    /// receipt is agent-reported.
    #[arg(long, value_name = "KEY_ID")]
    signed_by: Option<String>,

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
            Stage::Delivery,
            DenyReason::ContextNotFound,
            "no such context",
        ));
    };
    let Some(settled) = reached.settled else {
        return Ok(engine::unreachable(
            &ctx,
            Stage::Delivery,
            DenyReason::InvalidState,
            format!(
                "the context is {}; delivery needs settled",
                engine::state_name(reached.state)
            ),
        ));
    };
    let receipt = DeliveryReceipt {
        reference: cmd.receipt.clone(),
        attestation: match &cmd.signed_by {
            Some(key_id) => Attestation::MerchantSigned {
                key_id: key_id.clone(),
            },
            None => Attestation::AgentReported,
        },
    };
    match ledger.record_delivery(&settled, receipt) {
        Ok(delivered) => Ok(Report::new()
            .with("context", delivered.ctx().as_str())
            .with("state", "delivered")
            .with("receipt", delivered.receipt_reference())),
        Err(err) => engine::outcome(&err),
    }
}
