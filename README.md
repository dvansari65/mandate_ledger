# mandate-ledger

[![CI](https://github.com/dvansari65/mandate_ledger/actions/workflows/ci.yml/badge.svg)](https://github.com/dvansari65/mandate_ledger/actions/workflows/ci.yml)

**An enforcement layer for agent-driven payments.** It does not move money.
It decides whether each step of a payment — authorize → pay → settle →
deliver — is consistent, bounded, and final, and records a hash-chained
evidence trail of every decision, including refusals.

Rust core, protocol-agnostic. Free, Apache-2.0.

```
   mandate ──┐
             ├─► authorize ─► record_payment ─► record_settlement ─► record_delivery
   cart ─────┘        │              │                  │
                      └─ expire      └─ (rail proof)    └─ compensate
```

## Why

Agent payment protocols (x402, AP2, ACP, MPP) each verify their own step.
Nothing verifies that the steps agree with each other. A formal analysis
(arXiv 2609.00060) found 40 issues across the four, nearly all the same
shape: *the check exists, but the caller can skip it.* Delivered before
finality. Cart approved ≠ cart paid. Retry charged twice. Mandate can't be
revoked. Budget races under concurrency.

This library makes those paths **unrepresentable**:

| Guarantee | Enforced by |
|---|---|
| Delivery only after settlement | `record_delivery` accepts only a `Settled` token; store rejects the transition too |
| One nonce → one payment | store consumes nonces atomically on append |
| Cart approved = cart paid | cart hash bound at `authorize`, checked at `record_payment` |
| Budget holds under concurrency | reservation happens inside the store's append, under one lock/transaction |
| Mandates can be revoked | `revoke()`; future `authorize` denied |
| Every refusal is auditable | denials are ledger events too |
| Nothing is guessed | a scope constraint the cart can't satisfy → `UNVERIFIABLE_SCOPE`, not a pass |

See [`docs/threat-model.md`](docs/threat-model.md) for what is and is not covered.

## Quickstart

```bash
cargo run -p quickstart
```

```rust
use ml_core::*;
use ml_adapters::{MockRail, MockProof, MockFinality, NativeCartAdapter};

// Wire it: where events live, what moves money, who may sign mandates, the clock.
let ledger = Ledger::new(MemoryStore::new(), MockRail::new("mock", 1), signers, SystemClock);
let adapter = NativeCartAdapter::new().with_merchant_key("bb-2026", merchant_pubkey);

// Normalize whatever the wire sent into hash + claims. Items are opaque.
let cart = adapter.normalize(&raw_json)?;

// Each call returns a token the next call requires. Wrong order = compile error.
let auth     = ledger.authorize(&mandate, &cart, "order-1")?;
let paid     = ledger.record_payment(&auth, &proof)?;
let settled  = match ledger.record_settlement(&paid, &finality)? {
    Settlement::Settled(s) => s,
    Settlement::Pending { .. } => /* ask again later */ todo!(),
    Settlement::Failed(f) => { ledger.compensate(&f, None)?; return Ok(()) }
};
let delivered = ledger.record_delivery(&settled, receipt)?;

// Self-verifying evidence for disputes.
let bundle = ledger.evidence(auth.ctx())?.unwrap();
bundle.verify()?;
```

Across requests (HTTP handler now, webhook later), use `ledger.resume(&ctx)`
to get the token back. See [`docs/integration.md`](docs/integration.md).

## Inputs and outputs

| Call | You give | You get |
|---|---|---|
| `authorize` | signed `Mandate`, normalized `Cart`, request key | `Authorized` or `Denied { reason, detail }` |
| `record_payment` | `Authorized`, rail proof | `Paid` or `Denied` |
| `record_settlement` | `Paid`, finality evidence | `Settled` / `Pending` / `Failed` |
| `record_delivery` | `Settled`, receipt | `Delivered` |
| `evidence` | context id | `EvidenceBundle` (hash chain) |

Deny reasons are stable codes (`SCOPE_MERCHANT_MISMATCH`, `NONCE_ALREADY_USED`,
…) — see `ml_core::DenyReason`. Operational failures (store down, rail
unreachable) are separate error variants: retry those, never retry a denial.

## Plugging in

Three traits, all small:

- **`CartAdapter`** — wire format → `Cart { hash, claims, attestation }`. Ships: `NativeCartAdapter` (plain JSON, optional merchant signature).
- **`Rail`** — verify a proof, report finality. Ships: `MockRail`. Real rails (x402, PSP webhooks) are next.
- **`Store`** — records + hash-chained events; must apply side effects atomically on append. Ships: `MemoryStore`. SQL backends are next.

Plus **`SignerPolicy`** — which keys may sign mandates for which principal. Ships: `TrustedSigners` (static), `AcceptAnySigner` (tests only, insecure).

## Layout

```
crates/ml-core       engine: types, mandate/scope, state machine, store trait, ledger, evidence
crates/ml-adapters   NativeCartAdapter, MockRail
crates/ml-verify     lifecycle, denial, concurrency, tamper, and property tests
examples/quickstart  runnable end-to-end
site/                landing page (Next.js + TypeScript) — `pnpm --dir site dev`
docs/threat-model.md what is prevented, bounded, and out of scope
docs/integration.md  how a merchant, PSP, or wallet wires this in
```

## Verify it yourself

```bash
cargo test --workspace
cargo clippy --workspace --all-targets   # pedantic, zero warnings
```

## Status

`0.0.1` — core engine complete and tested. Not yet: protocol adapters
(x402, AP2, ACP, MPP), SQL store, TypeScript bindings, external audit.
Do not put money behind it yet.
