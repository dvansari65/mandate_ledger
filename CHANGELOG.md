# Changelog

Notable changes to mandate-ledger, newest first, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format. `ml-core`,
`ml-adapters` and `ml-store-postgres` share one version and one entry; what
a version number promises is in [`docs/stability.md`](docs/stability.md).
A heading says *breaking* when the release may break the Rust API.

Nothing has been released yet. The history before the first release is in
git; the entry below describes what that release will contain.

## [Unreleased]

### Added

- `ml-core`: the engine. `Ledger` with the five calls — authorize, record
  payment, record settlement, record delivery, evidence — and typestate
  tokens, so a lifecycle out of order does not compile. Ed25519-signed
  mandates with a scope lattice, attenuation and a signer policy; carts as
  a hash plus the claims a mandate can be scoped on; deterministic context
  ids from the mandate, the cart and a request key; hash-chained events;
  self-verifying evidence bundles with an optional exporter signature; an
  in-memory store for tests, with pruning.
- 26 refusal codes, `DenyReason`, each traced to a row of the threat model.
  A refusal is a ledger event; a context records at most 32 of them, and
  `Denied.recorded` says whether this one was written.
- Payee binding: a proof that names a payee is checked against the
  authorized merchant (`MERCHANT_BINDING_MISMATCH`), and a replayed
  authorization must present the mandate the context was authorized under
  (`CONTEXT_MISMATCH`).
- `ml-adapters`: `NativeCartAdapter`, a canonical-JSON cart format with an
  optional merchant signature, and `MockRail` for tests.
- `ml-store-postgres`: a durable store. Budgets, revocations, spent nonces
  and the chain survive a restart; `migrate()` is idempotent and serialises
  concurrent boots. The log is readable across contexts — `events_after`,
  `last_seq`, `scan` with keyset paging.
- `ml`, the sandbox command: keys, mandates, carts, every lifecycle step,
  the log, the contexts, evidence export and offline verification, a rail
  the operator drives and a clock the sandbox owns.
- Nine scenario scripts in `scenarios/`, one per threat-model row, run in CI
  against PostgreSQL.
- `SECURITY.md`, this changelog, a stability policy, a release process and
  crates.io metadata; the library crates deny missing documentation.
- `X402CartAdapter`: an x402 v2 cart — the resource and the accepted
  payment requirement — priced through an `AssetRegistry` of known tokens
  (USDC on Base, and separately on Base Sepolia); the merchant is the
  payee as a CAIP-10 account id, so a mandate names the chain it pays on;
  the amount is exact in the token's decimals; addresses are checked
  against their EIP-55 checksum; and anything the adapter does not price
  is refused before the engine sees it.
- `X402Rail`: an x402 v2 payment verified offline — the EIP-3009
  signature recovered under the token contract's own rules, the window and
  the payee checked as the reference facilitator checks them — reporting
  the signed payee, amount and binding nonce for the engine to compare;
  settlement judged from a receipt that must show this authorization and
  this transfer, or from the facilitator's answer when the operator trusts
  it. The binding nonce, `keccak256("mandate-ledger:x402-nonce:v1:" ‖ ctx)`,
  is how a payer signs which purchase a payment is for.
- `SettlementExpectation` carries the authorized amount and merchant, so a
  rail can check that evidence shows this payment and not another.
- `ml-serve`, the engine as an HTTP service: the five calls, expiry and
  revocation, the log and the contexts, signed evidence; `200` allowed,
  `409` refused with the code, `503` could not decide; API keys compared
  in constant time; load shed past the pool's permits; structured logs;
  graceful shutdown; configuration from the environment; an OpenAPI
  document generated from the code. The sandbox rail lives in
  `ml-adapters`, so that `ml` and the service share it.

[Unreleased]: https://github.com/dvansari65/mandate_ledger/commits/main
