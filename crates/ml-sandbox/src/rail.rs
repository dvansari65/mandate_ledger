//! The sandbox rail: a rail the operator drives.
//!
//! It verifies proofs the way the mock rail does — the proof says whether it
//! is valid, and the engine makes every comparison — and it reports finality
//! one of two ways. Told (`ml settle --confirmations N` or `--failed REASON`),
//! it reports that. Asked (`ml settle CTX` alone), it answers from the
//! payment reference itself:
//!
//! - `…-ok` — final.
//! - `…-fail` — failed: the rail declined or reverted it.
//! - `…-reorg` — seen, gaining a confirmation with every check, then dropped
//!   in a reorganization on the check that would have made it final. The
//!   count survives across processes; it lives in the database with the rest
//!   of the sandbox.
//!
//! Any other reference has to be told, or the rail says it has no report —
//! which the engine treats as the rail being unavailable, not as a decision.

use crate::sandbox;
use ml_adapters::MockProof;
use ml_core::{
    FinalityStatus, PaymentExpectation, Rail, RailError, SettlementExpectation, VerifiedProof,
};
use ml_store_postgres::PgPool;

pub struct SandboxRail {
    id: String,
    finality: u32,
    pool: PgPool,
}

/// What `ml settle` hands the rail: which payment, and — if the operator
/// spoke for the rail — what to report.
pub struct Finality {
    pub reference: String,
    pub reported: Option<Reported>,
}

pub enum Reported {
    Confirmations(u32),
    Failed(String),
}

enum Magic {
    Ok,
    Fail,
    Reorg,
}

/// What the rail saw when asked about a payment.
enum Observation {
    Confirmations(u32),
    Failed(String),
}

fn magic(reference: &str) -> Option<Magic> {
    if reference.ends_with("-ok") {
        Some(Magic::Ok)
    } else if reference.ends_with("-fail") {
        Some(Magic::Fail)
    } else if reference.ends_with("-reorg") {
        Some(Magic::Reorg)
    } else {
        None
    }
}

impl SandboxRail {
    /// A rail named `id` that calls a payment final at `finality`
    /// confirmations, remembering its checks through `pool`.
    pub fn new(id: String, finality: u32, pool: PgPool) -> Self {
        Self { id, finality, pool }
    }

    /// What the rail reports about a payment: as told, or as the reference
    /// dictates. Only a reference it has no way to answer is an error.
    fn observe(&self, finality: &Finality) -> Result<Observation, RailError> {
        Ok(match &finality.reported {
            Some(Reported::Confirmations(n)) => Observation::Confirmations(*n),
            Some(Reported::Failed(reason)) => Observation::Failed(reason.clone()),
            None => match magic(&finality.reference) {
                Some(Magic::Ok) => Observation::Confirmations(self.finality),
                Some(Magic::Fail) => Observation::Failed("declined by the rail".to_owned()),
                Some(Magic::Reorg) => {
                    let checks = sandbox::count_check(&self.pool, &self.id, &finality.reference)
                        .map_err(RailError::Unavailable)?;
                    if checks >= self.finality {
                        Observation::Failed(format!(
                            "reorganized: dropped after {} confirmations",
                            checks.saturating_sub(1)
                        ))
                    } else {
                        Observation::Confirmations(checks)
                    }
                }
                None => {
                    return Err(RailError::Unavailable(format!(
                        "the sandbox rail has no report for `{}`: pass --confirmations or \
                         --failed, or use a reference ending in -ok, -fail or -reorg",
                        finality.reference
                    )));
                }
            },
        })
    }
}

impl Rail for SandboxRail {
    type Proof = MockProof;
    type Finality = Finality;

    fn id(&self) -> &str {
        &self.id
    }

    fn verify_proof(
        &self,
        proof: &MockProof,
        _expected: &PaymentExpectation<'_>,
    ) -> Result<VerifiedProof, RailError> {
        if !proof.valid {
            return Err(RailError::InvalidProof("signature check failed".to_owned()));
        }
        Ok(VerifiedProof {
            reference: proof.reference.clone(),
            nonce: proof.nonce.clone(),
            amount: proof.amount.clone(),
            bound_ctx: proof.bound_ctx.clone(),
            bound_cart: proof.bound_cart,
        })
    }

    fn check_finality(
        &self,
        finality: &Finality,
        expected: &SettlementExpectation<'_>,
    ) -> Result<FinalityStatus, RailError> {
        if finality.reference != expected.payment_reference {
            return Err(RailError::InvalidProof(format!(
                "evidence is for `{}`, payment is `{}`",
                finality.reference, expected.payment_reference
            )));
        }
        let confirmations = match self.observe(finality)? {
            Observation::Confirmations(confirmations) => confirmations,
            Observation::Failed(reason) => return Ok(FinalityStatus::Failed { reason }),
        };
        if confirmations >= self.finality {
            Ok(FinalityStatus::Final {
                reference: format!("{}@{confirmations}", finality.reference),
            })
        } else {
            Ok(FinalityStatus::Pending {
                reason: format!("need {} confirmations, have {confirmations}", self.finality),
            })
        }
    }
}
