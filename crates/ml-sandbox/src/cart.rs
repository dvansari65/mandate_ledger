//! Carts: what the agent wants to buy, in the native JSON format.
//!
//! ```json
//! {
//!   "merchant": "bigbasket.com",
//!   "total": { "amount": "128.00", "currency": "INR" },
//!   "category": "grocery",
//!   "items": [ { "sku": "milk-1l", "qty": 2 } ]
//! }
//! ```
//!
//! `items` is opaque to the engine: it is hashed, never read. A mandate that
//! requires `merchant_signed` carts needs the merchant's signature over the
//! cart, which `ml cart sign` adds under a key id — the same id the operator
//! then names with `--merchant-key` when authorizing.

use crate::report::Report;
use crate::{Failure, files, keys};
use clap::Subcommand;
use ml_adapters::NativeCart;
use ml_core::Hash32;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Command {
    /// Sign a cart as the merchant, under a key id.
    Sign {
        #[arg(help = "The cart as JSON")]
        cart: PathBuf,
        /// The merchant's key file, from `ml keys new`.
        #[arg(long, value_name = "FILE")]
        key: PathBuf,
        /// The key id the signature is filed under.
        #[arg(long, value_name = "ID")]
        key_id: String,
        /// Where to write the signed cart.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
}

pub fn run(command: Command) -> Result<Report, Failure> {
    match command {
        Command::Sign {
            cart,
            key,
            key_id,
            out,
        } => {
            let cart: NativeCart = files::read_json(&cart)?;
            let key = keys::load(&key)?;
            let signed = cart
                .sign(key_id.as_str(), &key)
                .map_err(|e| Failure::undecided(format!("cannot sign: {e}")))?;
            files::write_json(&out, &signed)?;
            let bytes = signed
                .canonical_bytes()
                .map_err(|e| Failure::undecided(format!("cannot hash the cart: {e}")))?;
            Ok(Report::new()
                .with("file", out.display().to_string())
                .with("merchant", signed.merchant.as_str())
                .with("total", signed.total.to_string())
                .with("key_id", key_id)
                .with("hash", Hash32::of(&bytes).to_string()))
        }
    }
}
