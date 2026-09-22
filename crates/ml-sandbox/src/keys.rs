//! Sandbox signing keys: an Ed25519 key pair in one JSON file, and its
//! public half in another.
//!
//! ```text
//! user.key      { "algorithm": "ed25519", "public": "<64 hex>", "secret": "<64 hex>" }
//! user.key.pub  { "algorithm": "ed25519", "public": "<64 hex>" }
//! ```
//!
//! Anything that only needs to *trust* a key — `--trust`, `--merchant-key` —
//! reads the public half and accepts either file, so a private key never has
//! to leave the machine that signs with it. The private file is created
//! readable by its owner alone (on Unix) and is never overwritten. Its secret
//! is stored in the clear: acceptable for a sandbox and for nothing else.

use crate::Failure;
use crate::files;
use crate::report::Report;
use clap::Subcommand;
use ml_core::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum Command {
    /// Generate a key pair: the private key in FILE, the public key in FILE.pub.
    New {
        /// Where to write the private key. An existing file is never overwritten.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
}

const ALGORITHM: &str = "ed25519";

#[derive(Serialize, Deserialize)]
struct KeyFile {
    algorithm: String,
    #[serde(with = "hex")]
    public: [u8; 32],
    #[serde(with = "hex")]
    secret: [u8; 32],
}

/// The public half. A private key file parses as this too; its secret is
/// simply never read.
#[derive(Serialize, Deserialize)]
struct PublicKeyFile {
    algorithm: String,
    #[serde(with = "hex")]
    public: [u8; 32],
}

pub fn run(command: Command) -> Result<Report, Failure> {
    match command {
        Command::New { out } => {
            let key = generate()?;
            let public = key.verifying_key().to_bytes();
            files::create_private_json(
                &out,
                &KeyFile {
                    algorithm: ALGORITHM.to_owned(),
                    public,
                    secret: key.to_bytes(),
                },
            )?;
            let public_file = public_path(&out);
            files::write_json(
                &public_file,
                &PublicKeyFile {
                    algorithm: ALGORITHM.to_owned(),
                    public,
                },
            )?;
            Ok(Report::new()
                .with("file", out.display().to_string())
                .with("public_file", public_file.display().to_string())
                .with("public", hex::encode(public)))
        }
    }
}

/// `FILE.pub`, next to `FILE`.
fn public_path(private: &Path) -> PathBuf {
    let mut name: OsString = private.as_os_str().to_owned();
    name.push(".pub");
    PathBuf::from(name)
}

/// Load a private key for signing, checking the file is what it claims.
pub fn load(path: &Path) -> Result<SigningKey, Failure> {
    let file: KeyFile = files::read_json(path)?;
    check_algorithm(path, &file.algorithm)?;
    let key = SigningKey::from_bytes(&file.secret);
    if key.verifying_key().to_bytes() != file.public {
        return Err(Failure::undecided(format!(
            "{}: the public key does not match the secret",
            path.display()
        )));
    }
    Ok(key)
}

/// Load a public key for trusting, from either kind of file.
pub fn public(path: &Path) -> Result<VerifyingKey, Failure> {
    let file: PublicKeyFile = files::read_json(path)?;
    check_algorithm(path, &file.algorithm)?;
    VerifyingKey::from_bytes(&file.public).map_err(|_| {
        Failure::undecided(format!(
            "{}: not a valid Ed25519 public key",
            path.display()
        ))
    })
}

fn check_algorithm(path: &Path, algorithm: &str) -> Result<(), Failure> {
    if algorithm == ALGORITHM {
        Ok(())
    } else {
        Err(Failure::undecided(format!(
            "{}: unsupported algorithm `{algorithm}`",
            path.display()
        )))
    }
}

/// An Ed25519 secret key is 32 uniformly random bytes — exactly what
/// `ed25519-dalek` draws from a CSPRNG, without taking on its RNG traits.
fn generate() -> Result<SigningKey, Failure> {
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret)
        .map_err(|e| Failure::undecided(format!("no randomness from the operating system: {e}")))?;
    Ok(SigningKey::from_bytes(&secret))
}
