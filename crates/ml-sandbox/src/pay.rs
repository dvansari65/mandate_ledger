//! `ml pay`: record a payment proof against an authorized context.
//!
//! The proof is the mock rail's, so this command exposes what a real rail
//! would decide for itself — the reference, the single-use nonce, the amount
//! and what the proof is bound to. Those knobs are how the attack scripts
//! replay a nonce, swap a cart or present an unbound proof, and every one of
//! them is refused by the engine, not by this command.

use crate::Failure;
use crate::engine::{self, DbArgs, TrustArgs};
use crate::report::Report;
use clap::Args;
use ml_adapters::MockProof;
use ml_core::{DenyReason, Hash32, Money, Stage};

#[derive(Args)]
pub struct Cmd {
    /// The context, from `ml authorize`.
    #[arg(value_name = "CTX")]
    ctx: String,

    /// The rail's reference for the payment.
    #[arg(long, value_name = "REF")]
    reference: String,

    #[arg(
        long,
        value_name = "AMOUNT",
        value_parser = engine::money,
        help = "The amount the proof is for, as the engine prints money: `128.00 INR`"
    )]
    amount: Money,

    /// The single-use nonce the rail guarantees. Defaults to the reference;
    /// present the same one for a second context to replay a payment.
    #[arg(long, value_name = "NONCE")]
    nonce: Option<String>,

    /// Also bind the proof to this cart hash. The hash of a different cart
    /// is a swap.
    #[arg(long, value_name = "HASH")]
    bound_cart: Option<Hash32>,

    /// Do not bind the proof to the context. Alone, that is a proof bound to
    /// nothing; with --bound-cart, a proof bound to the cart only.
    #[arg(long)]
    unbound: bool,

    /// A proof whose signature does not verify.
    #[arg(long)]
    invalid: bool,

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
            Stage::Payment,
            DenyReason::ContextNotFound,
            "no such context",
        ));
    };

    let proof = MockProof {
        reference: cmd.reference.clone(),
        nonce: cmd.nonce.clone().unwrap_or_else(|| cmd.reference.clone()),
        amount: cmd.amount.clone(),
        bound_ctx: (!cmd.unbound).then(|| ctx.clone()),
        bound_cart: cmd.bound_cart,
        valid: !cmd.invalid,
    };
    // Always through the `Authorized` token: on a context that is already
    // paid, the engine decides whether this is the same payment again or a
    // different one, and records the refusal if it is.
    match ledger.record_payment(&reached.authorized, &proof) {
        Ok(paid) => Ok(Report::new()
            .with("context", paid.ctx().as_str())
            .with("state", "paid")
            .with("rail", paid.rail())
            .with("reference", paid.reference())
            .with("idempotency_key", paid.idempotency_key().to_string())),
        Err(err) => engine::outcome(&err),
    }
}
