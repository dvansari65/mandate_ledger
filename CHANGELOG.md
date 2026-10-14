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

[Unreleased]: https://github.com/dvansari65/mandate_ledger/commits/main
