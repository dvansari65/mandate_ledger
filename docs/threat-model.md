# Threat model (draft)

Source of truth for scope. Every invariant in `ml-core` must trace back to a row here.
Properties P1–P18 are from "A Formal Analysis of Agent Payment Protocols" (arXiv 2609.00060).

## A — Prevented structurally

| # | Attack | Property | Mechanism |
|---|---|---|---|
| A1  | Double-charge on retry              | P3      | idempotency key = H(mandate_id, cart_hash, nonce); single-use ledger row |
| A2  | Nonce race at facilitator           | P3      | unique nonce table, CAS insert |
| A3  | Release-before-finality             | P5      | `Delivered` only from `Settled`; `Settled` only from finality evidence |
| A4  | No rollback after failed settlement | P6      | `Paid → SettlementFailed → Compensated` is a first-class path |
| A5  | Cart swap                           | P1, P2  | cart hash threaded through every stage; mismatch = deny |
| A6  | Budget overrun under concurrency    | P8      | reservation ledger, compare-and-swap |
| A7  | Out-of-scope purchase               | P14     | `cart.claims ⊆ mandate.scope` |
| A8  | Revoked / expired mandate           | P17     | registry + signed revocation list, offline verifiable |
| A9  | Unsigned terms / amount tampering   | P18     | adapter rejects unsigned terms; merchant key pinning |
| A10 | Redirect / facilitator impersonation| P18     | allowlist + key pinning |
| A11 | HTTP 402 confusion                  | —       | strict schema validation; malformed = no payment |
| A12 | Denial-of-settlement                | P13     | reserve settlement capacity before execution |
| A13 | Validity-window expiry              | —       | refuse if `expires_at - now < settlement_p99` |
| A14 | Cross-stage identity mismatch       | P7, P15 | single `InvocationContext` id |
| A15 | Secrets in logs                     | P16     | `Secret<T>` newtype, redacting Debug |

## B — Effect bounded / evidence produced, root cause not prevented

| # | Problem | What we do |
|---|---|---|
| B1 | "The agent did it" first-party dispute | signed, hash-chained evidence bundle |
| B2 | Refund abuse at machine speed          | refunds are transitions; scope + velocity apply |
| B3 | Prompt-injection overspend             | effect blocked by scope/budget; injection itself not detected |
| B4 | Card testing via agents                | admission control + velocity only |

## C — Out of scope (by design)

- Prompt-injection detection
- Counterfeit merchant storefronts (needs merchant identity registry, e.g. Visa TAP)
- Agent credential theft (needs key mgmt / TEE; see Tenuo)
- Merchant non-delivery after settlement (needs escrow)
- Chain-level asset theft, gas abuse

## Cart schema

Items are opaque. Core sees only `hash(canonical bytes)` + `ScopeClaims { merchant, total, category?, line_count? }`
+ `Attestation { MerchantSigned | AgentReported }`. Scope fields the rail cannot verify → `Deny(UNVERIFIABLE_SCOPE)`. Fail closed.
