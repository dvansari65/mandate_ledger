//! The x402 protocol, version 2: the cart.
//!
//! An x402 purchase starts with a `402 Payment Required` carrying a
//! `PaymentRequired` object: what is being sold (`resource`) and the ways to
//! pay for it (`accepts`). The client picks one of those and, in its
//! `PaymentPayload`, echoes it as `accepted`. The two together are the cart:
//! what the agent is buying, from whom, for how much, in which token on
//! which chain. This module reads exactly that — the [`X402Cart`] document,
//! `{ x402Version, resource, accepted }` — and turns it into a
//! [`Cart`](ml_core::Cart) the engine can hold a mandate against.
//!
//! What the adapter checks, and refuses otherwise:
//!
//! - the document is x402 version 2 and the scheme is `exact`;
//! - the network is an `eip155` chain and the asset is one the
//!   [`AssetRegistry`] knows on that chain — an unknown token is never
//!   priced by guesswork;
//! - `payTo` and `asset` are EVM addresses, `amount` is a whole number of
//!   atomic units, `maxTimeoutSeconds` is a positive integer;
//! - the transfer method is the default, `eip3009`, and the payment flow
//!   the default, `authorization`; the EIP-712 domain in `extra` is the
//!   asset's own.
//!
//! The cart hash is over the canonical form of the whole document as
//! presented, unknown fields included, so the engine binds exactly what the
//! merchant offered and the client accepted. The claims are the payee as
//! the merchant, the amount as money in the asset's symbol, and nothing
//! else: x402 carries no category and no line items, and a claim the
//! protocol does not carry is not invented. The attestation is
//! agent-reported, because x402 does not sign its requirements.
//!
//! The reference for every field is the specification and the reference
//! implementation at <https://github.com/coinbase/x402> — `specs/x402-specification-v2.md`,
//! `specs/schemes/exact/scheme_exact_evm.md`, and the asset table in
//! `typescript/packages/mechanisms/evm/src/defaultAssets.ts`.

mod assets;
mod cart;
mod types;

pub use assets::{Asset, AssetRegistry, BASE, BASE_SEPOLIA};
pub use cart::{ExactTerms, X402CartAdapter};
pub use types::{Address, ChainId, PaymentRequirements, ResourceInfo, X402Cart};
