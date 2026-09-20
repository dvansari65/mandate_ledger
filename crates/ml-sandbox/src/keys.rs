//! Sandbox signing keys: an Ed25519 key pair in one JSON file.
//!
//! ```json
//! { "algorithm": "ed25519", "public": "<64 hex>", "secret": "<64 hex>" }
//! ```
//!
//! The secret is stored in the clear. That is acceptable for a sandbox and
//! for nothing else; a production wallet keeps its key in an HSM or a
//! signer service and hands the engine only signatures. The file is created
//! readable by its owner alone (on Unix) and is never overwritten.

use crate::Failure;
use crate::files;
use crate::report::Report;
use clap::Subcommand;
use ml_core::SigningKey;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum Command {
    /// Generate a key pair into a new file.
    New {
        /// Where to write it. An existing file is never overwritten.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    algorithm: String,
    #[serde(with = "hex")]
    public: [u8; 32],
    #[serde(with = "hex")]
    secret: [u8; 32],
}

const ALGORITHM: &str = "ed25519";

pub fn run(command: Command) -> Result<Report, Failure> {
    match command {
        Command::New { out } => {
            let key = generate()?;
            let file = KeyFile {
                algorithm: ALGORITHM.to_owned(),
                public: key.verifying_key().to_bytes(),
                secret: key.to_bytes(),
            };
            files::create_private_json(&out, &file)?;
            Ok(Report::new()
                .with("file", out.display().to_string())
                .with("public", hex::encode(file.public)))
        }
    }
}

/// Load a key file, checking that it is what it claims to be.
pub fn load(path: &Path) -> Result<SigningKey, Failure> {
    let file: KeyFile = files::read_json(path)?;
    if file.algorithm != ALGORITHM {
        return Err(Failure::undecided(format!(
            "{}: unsupported algorithm `{}`",
            path.display(),
            file.algorithm
        )));
    }
    let key = SigningKey::from_bytes(&file.secret);
    if key.verifying_key().to_bytes() != file.public {
        return Err(Failure::undecided(format!(
            "{}: the public key does not match the secret",
            path.display()
        )));
    }
    Ok(key)
}

/// An Ed25519 secret key is 32 uniformly random bytes — exactly what
/// `ed25519-dalek` draws from a CSPRNG, without taking on its RNG traits.
fn generate() -> Result<SigningKey, Failure> {
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret)
        .map_err(|e| Failure::undecided(format!("no randomness from the operating system: {e}")))?;
    Ok(SigningKey::from_bytes(&secret))
}
