//! # mandate-ledger as a service
//!
//! The five calls over HTTP, for a host that would rather not embed the
//! engine. One process, one ledger, one PostgreSQL database. The status
//! codes mirror the `ml` command's exit codes:
//!
//! | Status | Meaning |
//! |---|---|
//! | `200` | The step was allowed; the body states the token's facts. |
//! | `409` | The ledger evaluated the step and **refused** it. A decision, not a failure: the body names the code and the reason. Do not retry. |
//! | `503` | The ledger could not decide — the store, the rail or the service itself was unavailable. Read the context back, then retry: a step that timed out may still have been recorded. |
//! | `400`, `413`, `415`, `422` | The request is not something the ledger can be asked. |
//! | `401` | No valid API key. |
//!
//! Every response is JSON, and every response carries an `x-request-id`
//! header the service chose, which is the id in its logs.
//!
//! The crate is a library so that the service can be driven in-process by
//! tests; the `ml-serve` binary reads a [`Config`] from the environment
//! and calls [`run`].

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod app;
mod auth;
mod backend;
mod blocking;
pub mod config;
pub mod dto;
mod error;
mod handlers;
mod openapi;
mod request_id;
mod server;

pub use app::{App, AppOptions, Readiness};
pub use auth::{ApiKeys, MIN_KEY_LEN};
pub use backend::Backend;
pub use config::Config;
pub use openapi::spec;
pub use server::{Error, run};
