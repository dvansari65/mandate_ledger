//! The x402 protocol, version 2: the cart, the payment, and settlement.
//!
//! An x402 purchase starts with a `402 Payment Required` carrying a
//! `PaymentRequired` object: what is being sold (`resource`) and the ways to
//! pay for it (`accepts`). The client picks one of those and, in its
//! `PaymentPayload`, echoes it as `accepted`. The two together are the cart:
//! what the agent is buying, from whom, for how much, in which token on
//! which chain. This module reads exactly that — the [`X402Cart`] document,
//! `{ x402Version, resource, accepted, extensions? }`, which is the payment
//! payload without its signed `payload` — and turns it into a
//! [`Cart`](ml_core::Cart) the engine can hold a mandate against.
//!
//! What the adapter checks, and refuses otherwise:
//!
//! - the document is x402 version 2 and the scheme is `exact`;
//! - the network is an `eip155` chain and the asset is one the
//!   [`AssetRegistry`] knows on that chain — an unknown token is never
//!   priced by guesswork;
//! - `payTo` and `asset` are EVM addresses — checksummed when written in
//!   mixed case — and `payTo` is neither the zero address nor the token
//!   contract; `amount` is a whole number of atomic units written without
//!   leading zeros; `maxTimeoutSeconds` is a positive integer;
//! - the transfer method is the default, `eip3009`, and the payment flow
//!   the default, `authorization`; the EIP-712 domain in `extra` is the
//!   asset's own.
//!
//! The cart hash is over the canonical form of the whole document as
//! presented, unknown fields inside it included, so the engine binds
//! exactly what the merchant offered and the client accepted. The claims
//! are the payee as the merchant — a CAIP-10 account id, `eip155:8453:0x…`,
//! so the same address on another chain is another merchant and a mandate
//! names the chain it pays on — the amount as money in the asset's symbol,
//! and nothing else: x402 carries no category and no line items, and a
//! claim the protocol does not carry is not invented. The attestation is
//! agent-reported, because x402 does not sign its requirements; a mandate
//! that requires `merchant_signed` refuses every x402 cart.
//!
//! The reference for every field is the specification and the reference
//! implementation at <https://github.com/coinbase/x402> — `specs/x402-specification-v2.md`,
//! `specs/schemes/exact/scheme_exact_evm.md`, and the asset table in
//! `typescript/packages/mechanisms/evm/src/defaultAssets.ts`.

mod assets;
mod cart;
mod eip3009;
mod fields;
mod finality;
mod payment;
mod rail;
mod types;

pub use assets::{Asset, AssetRegistry, BASE, BASE_SEPOLIA, RegistryError};
pub use cart::{ExactTerms, X402CartAdapter};
pub use eip3009::{Authorization, binding_nonce, digest};
pub use finality::{Log, SettlementResponse, TransactionReceipt, X402Finality};
pub use payment::{AuthorizationBody, ExactEvmPayload, PaymentPayload};
pub use rail::{AuthorizationRef, FinalityPolicy, X402Rail};
pub use types::{Address, ChainId, PaymentRequirements, ResourceInfo, X402Cart, account_id};
