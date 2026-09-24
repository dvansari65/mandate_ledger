//! `ml verify`: check an evidence bundle from a file. No database, no
//! network — nothing but the file and, optionally, the key you expect it to
//! be signed with. This is the claim the project makes about evidence, made
//! runnable on a machine that has never seen the ledger.

use crate::Failure;
use crate::engine;
use crate::report::Report;
use crate::{files, keys};
use clap::Args;
use ml_core::{EvidenceBundle, EvidenceError, SignedEvidence};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct Cmd {
    /// The bundle, from `ml evidence`.
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// Require the bundle to be signed by this key. A `.pub` file is enough.
    #[arg(long, value_name = "KEYFILE")]
    signer: Option<PathBuf>,
}

/// Either shape `ml evidence` writes: a signed file has the bundle nested
/// under `bundle`, next to the exporter's key and signature; a bare file is
/// the bundle itself.
enum Evidence {
    Signed(SignedEvidence),
    Bare(EvidenceBundle),
}

/// Told apart by structure, so a malformed file gets the precise error —
/// "missing field `events`" — rather than "matched no variant".
fn read(path: &Path) -> Result<Evidence, Failure> {
    let value: Value = files::read_json(path)?;
    let parsed = if value.get("bundle").is_some() {
        serde_json::from_value(value).map(Evidence::Signed)
    } else {
        serde_json::from_value(value).map(Evidence::Bare)
    };
    parsed.map_err(|e| {
        Failure::undecided(format!("{} is not an evidence bundle: {e}", path.display()))
    })
}

pub fn run(cmd: &Cmd) -> Result<Report, Failure> {
    let expected = cmd.signer.as_deref().map(keys::public).transpose()?;
    let evidence = read(&cmd.file)?;
    let (bundle, signer) = match &evidence {
        Evidence::Signed(attested) => (&attested.bundle, Some(attested.signer)),
        Evidence::Bare(bundle) => (bundle, None),
    };
    let report = Report::new()
        .with("file", cmd.file.display().to_string())
        .with("context", bundle.ctx.as_str())
        .with("events", bundle.events.len())
        .with("final_state", bundle.final_state().map(engine::state_name))
        .with("signed", signer.is_some());

    // The chain, and the exporter's signature if there is one.
    let checked = match &evidence {
        Evidence::Signed(attested) => attested.verify(),
        Evidence::Bare(bundle) => bundle.verify(),
    };
    if let Err(e) = checked {
        return Ok(rejected(report, e.code(), e.seq(), e.to_string()));
    }
    // Then that it was the expected exporter who signed it.
    if let Some(want) = expected {
        let code = EvidenceError::SignatureInvalid.code();
        match signer {
            None => {
                return Ok(rejected(
                    report,
                    code,
                    None,
                    "not signed; --signer requires a signature",
                ));
            }
            Some(have) if have != want.to_bytes() => {
                let detail = format!(
                    "signed by {}, expected {}",
                    hex::encode(have),
                    hex::encode(want.to_bytes())
                );
                return Ok(rejected(report, code, None, detail));
            }
            Some(_) => {}
        }
    }
    Ok(report
        .with("signer", signer.map(hex::encode))
        .with("verified", true))
}

/// The bundle did not verify: exit 2, the code, the event it failed at.
fn rejected(report: Report, code: &str, seq: Option<u64>, detail: impl Into<String>) -> Report {
    report
        .refuse()
        .with("verified", false)
        .with("refused", code.to_owned())
        .with("event", seq)
        .with("detail", detail.into())
}
