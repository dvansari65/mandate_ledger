//! # mandate-ledger adapters
//!
//! Concrete plug-ins for the engine in `ml-core`:
//!
//! - [`native`] — a plain JSON cart format with optional merchant signature.
//!   Use it when you control both ends, or as the model for writing an
//!   adapter for a real protocol.
//! - [`mock`] — a rail that accepts what you tell it to. For tests and
//!   examples; never for production.
//! - [`sandbox`] — a rail the operator drives: told what to report, or
//!   answering from the payment reference. For sandboxes; never for
//!   production.
//!
//! Protocol adapters (x402, AP2, ACP, MPP) will live here as they land, one
//! module each, behind feature flags.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod mock;
pub mod native;
pub mod sandbox;

pub use mock::{MockFinality, MockProof, MockRail};
pub use native::{NativeAttestation, NativeCart, NativeCartAdapter};
pub use sandbox::SandboxRail;
