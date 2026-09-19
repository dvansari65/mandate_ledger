//! Sign a mandate body with a sandbox key.
//!
//! The body file is a `MandateBody` exactly as the engine hashes and signs
//! it — nothing is filled in for you, because a mandate is a statement of
//! authority and every field of it should be deliberate:
//!
//! ```json
//! {
//!   "id": "mnd-1",
//!   "principal": "user:alice",
//!   "agent": "agent:shopper",
//!   "scope": {
//!     "merchants": ["bigbasket.com", "*.zepto.com"],
//!     "categories": ["grocery"],
//!     "currency": "INR",
//!     "max_per_txn": { "amount": "2000.00", "currency": "INR" },
//!     "max_total": { "amount": "8000.00", "currency": "INR" },
//!     "valid_from": 1800000000,
//!     "valid_until": 1802592000,
//!     "velocity": { "max_count": 5, "window_secs": 86400 },
//!     "min_attestation": "merchant_signed"
//!   },
//!   "issued_at": 1800000000,
//!   "parent": null
//! }
//! ```
//!
//! `categories`, `max_per_txn`, `max_total`, `velocity` and `parent` may be
//! omitted. Times are Unix seconds. The output is the signed `Mandate` the
//! engine accepts, with the signer's public key and the signature as hex.

use crate::Failure;
use crate::files;
use crate::keys;
use crate::report::Report;
use clap::Subcommand;
use ml_core::{Mandate, MandateBody};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Command {
    /// Sign a mandate body, producing the mandate the engine accepts.
    Sign {
        #[arg(help = "The MandateBody as JSON")]
        body: PathBuf,
        /// The principal's key file, from `ml keys new`.
        #[arg(long, value_name = "FILE")]
        key: PathBuf,
        /// Where to write the signed mandate.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
}

pub fn run(command: Command) -> Result<Report, Failure> {
    match command {
        Command::Sign { body, key, out } => {
            let body: MandateBody = files::read_json(&body)?;
            let key = keys::load(&key)?;
            let mandate = Mandate::sign(body, &key)
                .map_err(|e| Failure::undecided(format!("cannot sign: {e}")))?;
            files::write_json(&out, &mandate)?;
            Ok(Report::new()
                .with("file", out.display().to_string())
                .with("mandate", mandate.id().as_str())
                .with("principal", mandate.body.principal.as_str())
                .with("signer", hex::encode(mandate.signer)))
        }
    }
}
