//! `ml revoke`: withdraw a mandate. Every later authorization under it is
//! refused, decided inside the store's own transaction; contexts already
//! authorized are unaffected.

use crate::Failure;
use crate::engine::{self, DbArgs};
use crate::report::Report;
use clap::Args;
use ml_core::{MandateId, Store as _};

#[derive(Args)]
pub struct Cmd {
    /// The mandate id, as in the mandate file.
    #[arg(value_name = "MANDATE")]
    mandate: String,

    #[command(flatten)]
    db: DbArgs,
}

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let mandate = MandateId::new(cmd.mandate.as_str())
        .map_err(|e| Failure::undecided(format!("mandate: {e}")))?;
    engine::store(&cmd.db)?
        .revoke(&mandate)
        .map_err(|e| Failure::undecided(e.to_string()))?;
    Ok(Report::new()
        .with("mandate", mandate.as_str())
        .with("revoked", true))
}
