//! # mandate-ledger core
//!
//! An enforcement layer for agent-driven payments. It does not move money.
//! It decides whether each step of a payment is consistent, bounded, and
//! final — and records a hash-chained evidence trail of every decision.
//!
//! ```text
//!   mandate ──┐
//!             ├─► authorize ─► record_payment ─► record_settlement ─► record_delivery
//!   cart ─────┘        │              │                  │
//!                      └─ expire      └─ (rail proof)    └─ compensate
//! ```
//!
//! ## The five calls
//!
//! | Call | Input | Guarantees |
//! |---|---|---|
//! | [`Ledger::authorize`] | signed [`Mandate`] + normalized [`Cart`] + request key | signature, signer trust, revocation, scope, per-txn cap, velocity, total budget (atomic) |
//! | [`Ledger::record_payment`] | [`Authorized`] + rail proof | amount match, cart/context binding, single-use nonce |
//! | [`Ledger::record_settlement`] | [`Paid`] + finality evidence | finality per rail policy; releases budget on failure |
//! | [`Ledger::record_delivery`] | [`Settled`] + receipt | only reachable from `Settled` — enforced by types *and* store |
//! | [`Ledger::evidence`] | context id | self-verifying hash chain |
//!
//! ## Where the guarantees live
//!
//! 1. **Types** — `record_delivery` takes a [`Settled`]; there is no way to
//!    construct one except from the engine.
//! 2. **Engine** — every method re-reads the stored state before acting.
//! 3. **Store** — [`Store::append`] enforces [`PaymentState::can_transition_to`]
//!    and applies budget / velocity / nonce effects atomically.
//!
//! ## What this crate does not do
//!
//! Parse line items (adapters do that — the engine binds by hash), verify
//! agent identity, move money, or detect prompt injection. See
//! `docs/threat-model.md` in the repository.

#![forbid(unsafe_code)]

pub mod cart;
pub mod error;
pub mod event;
pub mod evidence;
pub mod hash;
pub mod ids;
pub mod ledger;
pub mod mandate;
pub mod money;
pub mod rail;
pub mod secret;
pub mod state;
pub mod store;
pub mod time;

pub use cart::{AdapterError, Attestation, AttestationLevel, Cart, CartAdapter, ScopeClaims};
pub use error::{Denied, DenyReason, LedgerError, StoreError};
pub use event::{CartSnapshot, DeliveryReceipt, Event, EventBody, Stage};
pub use evidence::{EvidenceBundle, EvidenceError, SignedEvidence};
pub use hash::{CanonicalizeError, Hash32, canonical_json};
pub use ids::{AgentId, Category, ContextId, IdError, MandateId, MerchantId, PrincipalId};
pub use ledger::Ledger;
pub use mandate::{
    AcceptAnySigner, Mandate, MandateBody, MandateError, MerchantPattern, Scope, SignerPolicy,
    TrustedSigners, Velocity,
};
pub use money::{Currency, Money, MoneyError};
pub use rail::{
    FinalityStatus, PaymentExpectation, Rail, RailError, SettlementExpectation, VerifiedProof,
};
pub use secret::Secret;
pub use state::{
    Authorized, Compensated, Delivered, Paid, PaymentState, Reached, Resumed, Settled, Settlement,
    SettlementFailed,
};
pub use store::{AppendOutcome, MemoryStore, Record, RecordFilter, Store};
pub use time::{Clock, FixedClock, SystemClock, Timestamp};

/// Re-exported so hosts don't need a direct `ed25519-dalek` dependency to sign mandates.
pub use ed25519_dalek::SigningKey;
/// Re-exported for the same reason.
pub use ed25519_dalek::VerifyingKey;
