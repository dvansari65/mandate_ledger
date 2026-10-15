//! A rail the operator drives. **Sandboxes and tests only.**
//!
//! It verifies proofs the way [`MockRail`](crate::MockRail) does — the proof
//! says whether it is valid, and the engine makes every comparison — and it
//! reports finality one of two ways. Told ([`Finality::reported`] is set),
//! it reports that. Asked (`reported` is `None`), it answers from the
//! payment reference itself:
//!
//! - `…-ok` — final.
//! - `…-fail` — failed: the rail declined or reverted it.
//! - `…-reorg` — seen, gaining a confirmation with every check, then dropped
//!   in a reorganization on the check that would have made it final.
//!
//! How many times a reference has been checked is the one thing the rail
//! remembers, and the host supplies the memory through [`ReorgChecks`]: a
//! database table for a tool that runs one process per command, or
//! [`MemoryChecks`] for a process that lives as long as the sandbox does.
//!
//! Any other reference has to be told, or the rail says it has no report —
//! which the engine treats as the rail being unavailable, not as a decision.

use crate::MockProof;
use ml_core::{
    FinalityStatus, PaymentExpectation, Rail, RailError, SettlementExpectation, VerifiedProof,
};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Mutex, PoisonError};

/// What the rail remembers between finality checks.
pub trait ReorgChecks: Send + Sync {
    /// Record one more check of `reference` on `rail`, and return how many
    /// there have been, this one included. An error means the rail cannot
    /// answer right now; the engine reports it as unavailable.
    fn count(&self, rail: &str, reference: &str) -> Result<u32, String>;
}

/// Checks remembered in memory, for a process that outlives the lifecycle.
///
/// Bounded: once `capacity` distinct references are remembered, a new one
/// is refused — the rail is unavailable for it — rather than let anyone who
/// can name a reference grow the process without limit.
pub struct MemoryChecks {
    checks: Mutex<HashMap<(String, String), u32>>,
    capacity: usize,
}

impl MemoryChecks {
    /// Memory for at most `capacity` distinct references.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            checks: Mutex::new(HashMap::new()),
            capacity,
        }
    }
}

impl ReorgChecks for MemoryChecks {
    fn count(&self, rail: &str, reference: &str) -> Result<u32, String> {
        let mut checks = self.checks.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (rail.to_owned(), reference.to_owned());
        if !checks.contains_key(&key) && checks.len() >= self.capacity {
            return Err(format!(
                "the sandbox rail remembers {} references already and will not remember `{reference}`",
                self.capacity
            ));
        }
        let count = checks.entry(key).or_insert(0);
        *count = count.saturating_add(1);
        Ok(*count)
    }
}

/// What a host hands the rail at settlement: which payment, and — if the
/// operator spoke for the rail — what to report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finality {
    /// The payment the evidence is about.
    pub reference: String,
    /// What the operator told the rail to report, or `None` to let the
    /// reference speak.
    pub reported: Option<Reported>,
}

/// What an operator can make the rail say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reported {
    /// This many confirmations.
    Confirmations(u32),
    /// The payment failed, for this reason.
    Failed(String),
}

/// The sandbox rail.
pub struct SandboxRail {
    id: String,
    finality: NonZeroU32,
    checks: Box<dyn ReorgChecks>,
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
    /// confirmations, remembering its checks in `checks`.
    pub fn new(
        id: impl Into<String>,
        finality: NonZeroU32,
        checks: impl ReorgChecks + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            finality,
            checks: Box::new(checks),
        }
    }

    /// What the rail reports about a payment: as told, or as the reference
    /// dictates. Only a reference it has no way to answer is an error.
    fn observe(&self, finality: &Finality) -> Result<Observation, RailError> {
        Ok(match &finality.reported {
            Some(Reported::Confirmations(n)) => Observation::Confirmations(*n),
            Some(Reported::Failed(reason)) => Observation::Failed(reason.clone()),
            None => match magic(&finality.reference) {
                Some(Magic::Ok) => Observation::Confirmations(self.finality.get()),
                Some(Magic::Fail) => Observation::Failed("declined by the rail".to_owned()),
                Some(Magic::Reorg) => {
                    let checks = self
                        .checks
                        .count(&self.id, &finality.reference)
                        .map_err(RailError::Unavailable)?;
                    if checks >= self.finality.get() {
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
                        "the sandbox rail has no report for `{}`: tell it what to report, \
                         or use a reference ending in -ok, -fail or -reorg",
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
            bound_merchant: proof.bound_merchant.clone(),
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
        if confirmations >= self.finality.get() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ml_core::ContextId;

    fn rail(finality: u32) -> SandboxRail {
        SandboxRail::new(
            "mock",
            NonZeroU32::new(finality).unwrap(),
            MemoryChecks::new(2),
        )
    }

    fn check(rail: &SandboxRail, reference: &str) -> Result<FinalityStatus, RailError> {
        let ctx = ContextId::new("ctx").unwrap();
        let expected = SettlementExpectation {
            ctx: &ctx,
            payment_reference: reference,
        };
        rail.check_finality(
            &Finality {
                reference: reference.to_owned(),
                reported: None,
            },
            &expected,
        )
    }

    #[test]
    fn a_reorg_reference_is_pending_then_dropped() {
        let rail = rail(2);
        assert!(matches!(
            check(&rail, "p-reorg"),
            Ok(FinalityStatus::Pending { .. })
        ));
        assert!(matches!(
            check(&rail, "p-reorg"),
            Ok(FinalityStatus::Failed { .. })
        ));
    }

    #[test]
    fn memory_is_bounded() {
        let rail = rail(1);
        assert!(check(&rail, "a-reorg").is_ok());
        assert!(check(&rail, "b-reorg").is_ok());
        assert!(matches!(
            check(&rail, "c-reorg"),
            Err(RailError::Unavailable(_))
        ));
        // A reference it already knows is still answered.
        assert!(check(&rail, "a-reorg").is_ok());
    }

    #[test]
    fn an_unknown_reference_is_unavailable_not_a_decision() {
        assert!(matches!(
            check(&rail(1), "p"),
            Err(RailError::Unavailable(_))
        ));
    }
}
