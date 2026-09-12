//! Self-verifying evidence bundles for disputes and audits.
//!
//! A bundle is the complete hash chain for one context. Anyone holding it
//! can recompute every hash and confirm nothing was inserted, removed, or
//! altered — without access to the store. A [`SignedEvidence`] adds the
//! host's Ed25519 signature so a third party can also confirm *who* exported it.

use crate::event::{CartSnapshot, Event, EventBody};
use crate::hash::{CanonicalizeError, Hash32, canonical_json};
use crate::ids::ContextId;
use crate::mandate::Mandate;
use crate::state::PaymentState;
use crate::time::Timestamp;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Bundle format version.
pub const EVIDENCE_VERSION: u16 = 1;

/// The full, ordered event chain for one context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceBundle {
    /// Format version.
    pub version: u16,
    /// The context.
    pub ctx: ContextId,
    /// When the bundle was produced.
    pub generated_at: Timestamp,
    /// Events, oldest first.
    pub events: Vec<Event>,
}

impl EvidenceBundle {
    pub(crate) const fn new(ctx: ContextId, generated_at: Timestamp, events: Vec<Event>) -> Self {
        Self {
            version: EVIDENCE_VERSION,
            ctx,
            generated_at,
            events,
        }
    }

    /// Recompute the hash chain and confirm it is intact.
    pub fn verify(&self) -> Result<(), EvidenceError> {
        if self.events.is_empty() {
            return Err(EvidenceError::Empty);
        }
        let mut prev = Hash32::ZERO;
        let mut last_seq = 0u64;
        for e in &self.events {
            if e.ctx != self.ctx {
                return Err(EvidenceError::WrongContext { seq: e.seq });
            }
            if e.seq <= last_seq {
                return Err(EvidenceError::SequenceNotIncreasing { seq: e.seq });
            }
            if e.prev_hash != prev {
                return Err(EvidenceError::BrokenChain { seq: e.seq });
            }
            if !e.verify_hash()? {
                return Err(EvidenceError::HashMismatch { seq: e.seq });
            }
            prev = e.hash;
            last_seq = e.seq;
        }
        Ok(())
    }

    /// The state after the last state-changing event.
    #[must_use]
    pub fn final_state(&self) -> Option<PaymentState> {
        self.events
            .iter()
            .rev()
            .find_map(|e| e.body.resulting_state())
    }

    /// The mandate the context was authorized under.
    #[must_use]
    pub fn mandate(&self) -> Option<&Mandate> {
        self.events.iter().find_map(|e| match &e.body {
            EventBody::Authorized { mandate, .. } => Some(mandate),
            _ => None,
        })
    }

    /// The cart as authorized.
    #[must_use]
    pub fn cart(&self) -> Option<&CartSnapshot> {
        self.events.iter().find_map(|e| match &e.body {
            EventBody::Authorized { cart, .. } => Some(cart),
            _ => None,
        })
    }

    /// Canonical bytes to sign.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, CanonicalizeError> {
        canonical_json(self)
    }

    /// Sign with the host's key.
    pub fn sign(self, key: &SigningKey) -> Result<SignedEvidence, CanonicalizeError> {
        let signature = key.sign(&self.signing_bytes()?);
        Ok(SignedEvidence {
            bundle: self,
            signer: key.verifying_key().to_bytes(),
            signature: signature.to_bytes(),
        })
    }
}

/// An [`EvidenceBundle`] with the exporting host's signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedEvidence {
    /// The bundle.
    pub bundle: EvidenceBundle,
    /// Ed25519 public key of the exporter.
    #[serde(with = "hex")]
    pub signer: [u8; 32],
    /// Signature over `bundle.signing_bytes()`.
    #[serde(with = "hex")]
    pub signature: [u8; 64],
}

impl SignedEvidence {
    /// Verify the chain and the exporter's signature.
    pub fn verify(&self) -> Result<(), EvidenceError> {
        self.bundle.verify()?;
        let key =
            VerifyingKey::from_bytes(&self.signer).map_err(|_| EvidenceError::SignatureInvalid)?;
        let sig = Signature::from_bytes(&self.signature);
        key.verify_strict(&self.bundle.signing_bytes()?, &sig)
            .map_err(|_| EvidenceError::SignatureInvalid)
    }
}

/// Why a bundle failed verification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceError {
    /// No events.
    #[error("bundle is empty")]
    Empty,
    /// An event belongs to a different context.
    #[error("event {seq} belongs to a different context")]
    WrongContext {
        /// Offending sequence number.
        seq: u64,
    },
    /// `prev_hash` does not match the preceding event.
    #[error("chain broken at event {seq}")]
    BrokenChain {
        /// Offending sequence number.
        seq: u64,
    },
    /// The event's own hash does not match its contents.
    #[error("hash mismatch at event {seq}")]
    HashMismatch {
        /// Offending sequence number.
        seq: u64,
    },
    /// Sequence numbers are not strictly increasing.
    #[error("sequence not increasing at event {seq}")]
    SequenceNotIncreasing {
        /// Offending sequence number.
        seq: u64,
    },
    /// The exporter's signature did not verify.
    #[error("exporter signature invalid")]
    SignatureInvalid,
    /// Canonicalization failed.
    #[error(transparent)]
    Canonicalize(#[from] CanonicalizeError),
}
