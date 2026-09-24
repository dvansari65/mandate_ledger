//! `ml authorize`: check a cart against a mandate and reserve its total.
//!
//! The first command that decides something. On success the context id it
//! prints is the handle every later step takes; on refusal the report says
//! which check failed, and the refusal is in the log.

use crate::engine::{self, DbArgs, RailArgs, TrustArgs};
use crate::report::Report;
use crate::{Failure, files};
use clap::Args;
use ml_adapters::NativeCart;
use ml_core::Mandate;
use std::path::PathBuf;

#[derive(Args)]
pub struct Cmd {
    /// The signed mandate, from `ml mandate sign`.
    #[arg(long, value_name = "FILE")]
    mandate: PathBuf,

    /// The cart, signed or not.
    #[arg(long, value_name = "FILE")]
    cart: PathBuf,

    /// Idempotency key for this purchase attempt. The same mandate, cart and
    /// key always land on the same context and reserve nothing twice.
    #[arg(long, value_name = "KEY")]
    request_key: String,

    #[command(flatten)]
    db: DbArgs,

    #[command(flatten)]
    trust: TrustArgs,

    #[command(flatten)]
    rail: RailArgs,
}

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let mandate: Mandate = files::read_json(&cmd.mandate)?;
    let raw: NativeCart = files::read_json(&cmd.cart)?;
    // A cart the adapter rejects — a bad or unknown merchant signature —
    // never reaches the ledger, so that is an error, not a recorded refusal.
    let cart = engine::adapter(&cmd.trust)?
        .normalize_cart(&raw)
        .map_err(|e| Failure::undecided(format!("{}: {e}", cmd.cart.display())))?;

    let ledger = engine::ledger(&cmd.db, &cmd.trust, &cmd.rail)?;
    match ledger.authorize(&mandate, &cart, &cmd.request_key) {
        Ok(auth) => Ok(Report::new()
            .with("context", auth.ctx().as_str())
            .with("state", "authorized")
            .with("mandate", auth.mandate_id().as_str())
            .with("merchant", auth.merchant().as_str())
            .with("amount", auth.amount().to_string())
            .with("cart_hash", auth.cart_hash().to_string())),
        Err(err) => engine::outcome(&err),
    }
}
