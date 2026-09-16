//! # mandate-ledger — PostgreSQL store
//!
//! A durable [`ml_core::Store`] so the ledger's guarantees survive a restart.
//! With the in-memory store, a process restart forgets the budget already
//! reserved, which mandates were revoked, which payment nonces were spent and
//! the whole evidence chain — so every limit silently resets. This crate is
//! what makes those four guarantees real.
//!
//! ```no_run
//! use ml_core::Store;
//! use ml_store_postgres::PostgresStore;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let store = PostgresStore::connect("postgres://localhost/mandate_ledger")?;
//! store.migrate()?;                       // idempotent; safe on every boot
//! // let ledger = Ledger::new(store, rail, signers, SystemClock);
//! # Ok(()) }
//! ```
//!
//! ## Atomicity
//!
//! [`ml_core::Store::append`] must apply an event *and* its side effects
//! indivisibly. Here that is one transaction holding an advisory lock on the
//! context — and, for an authorization, on the mandate as well, so two
//! different contexts cannot both pass the same budget check. Locks are
//! always taken context-first, so no deadlock cycle exists.
//!
//! The concurrency tests in `ml-verify` are run against this store unchanged:
//! sixteen threads against one budget still yield exactly ten authorizations,
//! and eight threads presenting one nonce still yield exactly one payment.
//!
//! ## Not yet
//!
//! Plaintext connections only via [`PostgresStore::connect`] — supply your own
//! pool with [`PostgresStore::from_pool`] for TLS. `ml_authorizations` grows
//! one row per authorization and has no retention job yet.

#![forbid(unsafe_code)]

mod convert;
mod store;

pub use store::{PgPool, PostgresStore};
