<div align="center">

# mandate-ledger

**An enforcement layer for agent-driven payments.**

[![CI](https://github.com/dvansari65/mandate_ledger/actions/workflows/ci.yml/badge.svg)](https://github.com/dvansari65/mandate_ledger/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/rustc-1.85%2B-orange.svg)](#requirements)

</div>

It does not move money. It decides whether each step of a payment —
authorize, pay, settle, deliver — is consistent, bounded and final, and
records a hash-chained evidence trail of every decision, refusals included.

```text
  mandate ──┐
            ├──► authorize ──► record_payment ──► record_settlement ──► record_delivery
  cart ─────┘         │                                    │
                      ▼                                    ▼
                   expire                             compensate
```

Every call returns a token the next one requires, so an out-of-order
lifecycle does not compile. Every decision, including every refusal, is
appended to a chain anyone can verify without access to your database.

> [!WARNING]
> **Version 0.0.1 — not ready for production money.** The core engine and a
> durable PostgreSQL store are complete and tested. Protocol adapters
> (x402, AP2, ACP, MPP), TypeScript bindings and an external audit are not
> done. See [Status](#status).

## Contents

- [The problem](#the-problem)
- [What it guarantees](#what-it-guarantees)
- [Getting started](#getting-started)
- [The five calls](#the-five-calls)
- [Durable storage](#durable-storage)
- [Extending it](#extending-it)
- [Project layout](#project-layout)
- [Development](#development)
- [License](#license)

## The problem

The agent payment protocols — x402, AP2, ACP, MPP — each verify their own
step correctly. None verifies that the steps agree with one another.

A formal analysis of all four ([arXiv 2609.00060][paper]) found forty issues,
nearly all the same shape: *the check exists, but the caller can skip it.*
In practice that means goods delivered against a payment that later reverts,
a cart approved for one amount and paid for another, a retry charged twice, a
mandate that cannot be revoked once granted, and parallel workers that each
read the same remaining budget and all spend it.

Those failures are not specific to one protocol, and no single protocol owns
the question. This library is the layer that asks it.

## What it guarantees

| Guarantee | How it is enforced |
|---|---|
| Delivery only after settlement | `record_delivery` accepts only a `Settled` token, and the store refuses the transition independently |
| One payment nonce, one payment | the store claims nonces atomically on append, arbitrated by a unique index |
| The cart approved is the cart paid | the cart hash is bound at `authorize` and checked at `record_payment` |
| Budgets hold under concurrency | the reservation is applied inside the store's append, in one transaction |
| Mandates can be revoked | `revoke()` — every later `authorize` under that mandate is denied |
| Refusals are auditable | denials are ledger events, with stable machine-readable codes |
| Nothing is inferred | a scope constraint the cart cannot satisfy yields `UNVERIFIABLE_SCOPE`, never a pass |

[`docs/threat-model.md`](docs/threat-model.md) states precisely what is
prevented, what is only bounded, and what is out of scope.

## Getting started

### Requirements

- Rust 1.85 or newer (checked in CI)
- PostgreSQL 14 or newer, for durable storage — optional for tests and examples

### Installation

The crates are not yet published to crates.io. Depend on the repository:

```toml
[dependencies]
ml-core = { git = "https://github.com/dvansari65/mandate_ledger" }
ml-adapters = { git = "https://github.com/dvansari65/mandate_ledger" }
ml-store-postgres = { git = "https://github.com/dvansari65/mandate_ledger" }
```

Run the end-to-end example, which needs no database:

```bash
cargo run -p quickstart
```

### Usage

```rust
use ml_adapters::{MockRail, NativeCartAdapter};
use ml_core::{Ledger, MemoryStore, Settlement, SystemClock};

// Assemble the engine: where events live, what moves money, which keys may
// sign mandates, and the clock.
let ledger = Ledger::new(MemoryStore::new(), MockRail::new("mock", 1), signers, SystemClock);
let adapter = NativeCartAdapter::new().with_merchant_key("bb-2026", merchant_pubkey);

// Normalize the wire format into a hash plus the claims a mandate can be
// scoped on. Line items are never interpreted.
let cart = adapter.normalize(&raw_json)?;

let auth = ledger.authorize(&mandate, &cart, "order-1")?;
let paid = ledger.record_payment(&auth, &proof)?;

let settled = match ledger.record_settlement(&paid, &finality)? {
    Settlement::Settled(s) => s,
    Settlement::Pending { reason } => return Ok(retry_later(reason)),
    Settlement::Failed(f) => {
        undo_side_effects();
        ledger.compensate(&f, Some(&refund_id))?;
        return Ok(());
    }
};

// Unreachable without a `Settled`: passing `paid` here is a type error.
ledger.record_delivery(&settled, receipt)?;

// Self-verifying evidence, for a dispute months later.
let bundle = ledger.evidence(auth.ctx())?.expect("the context exists");
bundle.verify()?;
```

A lifecycle usually spans requests — authorize in an HTTP handler, settle in
a webhook minutes later. `ledger.resume(&ctx)` returns the token to carry on
with. [`docs/integration.md`](docs/integration.md) walks through a merchant,
a payment gateway and a wallet.

## The five calls

| Call | Input | Output |
|---|---|---|
| `authorize` | signed `Mandate`, normalized `Cart`, request key | `Authorized`, or `Denied { reason, detail }` |
| `record_payment` | `Authorized`, rail proof | `Paid`, or `Denied` |
| `record_settlement` | `Paid`, finality evidence | `Settled`, `Pending` or `Failed` |
| `record_delivery` | `Settled`, receipt | `Delivered` |
| `evidence` | context id | `EvidenceBundle` — the hash chain |

A denial is a decision: final, machine-readable, and never to be retried.
There are 24 stable codes — `SCOPE_MERCHANT_MISMATCH`, `NONCE_ALREADY_USED`
and the rest — enumerated by `ml_core::DenyReason`. Operational failures, such
as the store being unreachable, are separate error variants, and those *are*
worth retrying.

## Durable storage

`MemoryStore` holds everything in a `HashMap`, which is the right choice for
tests and the wrong one for anything else: a restart forgets the reserved
budget, the revoked mandates, the spent payment nonces and the evidence
chain. Every limit silently resets. An agent can spend its monthly cap a
second time, a revoked mandate starts working again, and a spent payment
proof is accepted afresh.

`ml-store-postgres` makes those four guarantees durable:

```rust
use ml_store_postgres::PostgresStore;

let store = PostgresStore::connect("postgres://localhost/mandate_ledger")?;
store.migrate()?;                    // idempotent, and safe on every boot
let ledger = Ledger::new(store, rail, signers, SystemClock);
```

`append` runs as a single transaction under an advisory lock on the context
and, for an authorization, on the mandate as well. The concurrency properties
are identical to the in-memory store, and are asserted against real
transactions in
[`crates/ml-store-postgres/tests/contract.rs`](crates/ml-store-postgres/tests/contract.rs):
sixteen threads against one budget yield exactly ten authorizations, and
eight threads presenting one nonce yield exactly one payment.

## Extending it

Four traits, each small:

| Trait | Responsibility | Provided |
|---|---|---|
| `CartAdapter` | wire format → `Cart { hash, claims, attestation }` | `NativeCartAdapter` — JSON with an optional merchant signature |
| `Rail` | verify a payment proof, report finality | `MockRail`; real rails are next |
| `Store` | records and hash-chained events, side effects applied atomically | `MemoryStore`, `PostgresStore` |
| `SignerPolicy` | which keys may sign mandates for which principal | `TrustedSigners`; `AcceptAnySigner` is for tests and says so |

A `CartAdapter` must fill only the claims its protocol actually carries.
Guessing one is worse than leaving it empty, because a claim the mandate
constrains and the cart cannot prove is refused by design.

## Project layout

```text
crates/ml-core             engine: types, mandates and scope, state machine,
                           store trait, ledger, evidence bundles
crates/ml-adapters         NativeCartAdapter, MockRail
crates/ml-store-postgres   durable PostgreSQL store
crates/ml-verify           lifecycle, denial, concurrency, tamper and property tests
examples/quickstart        runnable end to end, no database required
site/                      landing page (Next.js, TypeScript)
docs/threat-model.md       what is prevented, bounded, and out of scope
docs/integration.md        how a merchant, gateway or wallet wires this in
```

## Development

```bash
cargo test --workspace                    # 61 tests; Postgres suites skip without a database
cargo clippy --workspace --all-targets    # pedantic, zero warnings
cargo fmt --all --check
```

To include the PostgreSQL suites, point them at a database:

```bash
createdb mandate_ledger_test
ML_TEST_DATABASE_URL=postgres://localhost/mandate_ledger_test cargo test -p ml-store-postgres
```

CI additionally checks the crate on the minimum supported Rust version,
builds the documentation with warnings denied, audits dependencies for
advisories and licences with `cargo-deny` and `cargo-audit`, and runs the
PostgreSQL suites against a service container.

Every invariant in the core should trace back to a row in
[`docs/threat-model.md`](docs/threat-model.md). Changes that add an invariant
are easiest to review when they add the row too.

## Status

`0.0.1`. The core engine and the PostgreSQL store are complete and tested.
Still outstanding: protocol adapters for x402, AP2, ACP and MPP; verified
delegation chains and key rotation; partial captures and refunds; TypeScript
bindings; and an external audit. Do not put real money behind it yet.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

[paper]: https://arxiv.org/abs/2609.00060
